//! Independent cursors, shared budgets, cancellation and failure boundaries.
use connector_api::{StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, path, ts};
use std::num::NonZeroUsize;
use sync_core::{CancellationToken, HashAlgorithm};
use sync_filter::{Filter, FilterRules};
use sync_scanner::{Presence, ScanError, ScanOptions, scan, scan_observed};

fn workers() -> NonZeroUsize {
    NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN)
}
fn tree() -> MemoryConnector {
    let mut tree = MemoryConnector::new("parallel")
        .with_page_size(2)
        .with_listing_concurrency(workers());
    for name in ["a", "b", "c", "d"] {
        for n in 0..5 {
            tree = tree.file(&format!("{name}/f{n}"), b"fixture", ts(10));
        }
    }
    tree.dir("empty")
        .file("ignored/secret", b"excluded", ts(1))
        .symlink("link")
}

#[test]
fn parallel_listing_matches_sequential_facts_filters_hashes_and_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let source = tree().fail("c", StorageOperation::List, StorageErrorKind::Disconnected);
    let filter = Filter::new(FilterRules::new().exclude_directory("ignored"))?;
    let cancel = CancellationToken::new();
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let serial = scan(
        &source,
        &filter,
        &options.clone().with_listing_workers(NonZeroUsize::MIN),
        &cancel,
    )?;
    let parallel = scan(&source, &filter, &options, &cancel)?;
    assert_eq!(
        serial.entries().collect::<Vec<_>>(),
        parallel.entries().collect::<Vec<_>>()
    );
    assert_eq!(
        serial.directories().collect::<Vec<_>>(),
        parallel.directories().collect::<Vec<_>>()
    );
    assert_eq!(
        serial.excluded().collect::<Vec<_>>(),
        parallel.excluded().collect::<Vec<_>>()
    );
    assert_eq!(serial.issues(), parallel.issues());
    assert_eq!(serial.stats(), parallel.stats());
    assert!(!parallel.is_complete());
    assert!(matches!(
        parallel.presence(&path("c/missing")),
        Presence::Unknown(_)
    ));
    assert_eq!(
        parallel.presence(&path("ignored/secret")),
        Presence::Excluded
    );
    assert_eq!(source.mutations(), 0);
    Ok(())
}

#[test]
fn parallel_shared_budget_never_allows_a_complete_over_limit_scan()
-> Result<(), Box<dyn std::error::Error>> {
    let source = tree();
    let inventory = scan(
        &source,
        &Filter::include_all(),
        &ScanOptions::new().with_entry_limit(NonZeroUsize::new(15).ok_or("limit")?),
        &CancellationToken::new(),
    )?;
    assert!(!inventory.is_complete());
    assert!(inventory.len() <= 15);
    assert_eq!(source.mutations(), 0);
    Ok(())
}

#[test]
fn parallel_cancel_drains_workers_and_keeps_all_unvisited_directories_unknown()
-> Result<(), Box<dyn std::error::Error>> {
    let source = tree();
    let cancel = CancellationToken::new();
    let result = scan_observed(
        &source,
        &Filter::include_all(),
        &ScanOptions::new(),
        &cancel,
        &|stats, _| {
            if stats.files > 0 {
                cancel.cancel();
            }
        },
    );
    let Err(ScanError::Cancelled { partial }) = result else {
        return Err("cancellation was lost".into());
    };
    assert!(!partial.is_complete());
    for (directory, state) in partial.directories() {
        if !state.is_complete() {
            assert!(matches!(
                partial.presence(&directory.join("missing")?),
                Presence::Unknown(_)
            ));
        }
    }
    assert_eq!(source.mutations(), 0);
    Ok(())
}

#[test]
fn parallel_root_failure_is_fatal_and_empty_root_is_complete()
-> Result<(), Box<dyn std::error::Error>> {
    let source = tree().fail("", StorageOperation::List, StorageErrorKind::Disconnected);
    assert!(matches!(
        scan(
            &source,
            &Filter::include_all(),
            &ScanOptions::new(),
            &CancellationToken::new()
        ),
        Err(ScanError::RootUnavailable(_))
    ));
    let source = MemoryConnector::new("empty").with_listing_concurrency(workers());
    let inventory = scan(
        &source,
        &Filter::include_all(),
        &ScanOptions::new(),
        &CancellationToken::new(),
    )?;
    assert!(inventory.is_complete() && inventory.is_empty());
    Ok(())
}

#[test]
fn workers_really_overlap_within_the_advertised_bound() -> Result<(), Box<dyn std::error::Error>> {
    use connector_api::{
        ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector, StorageError,
    };
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    use sync_core::{ConnectorId, FileEntry, ResourcePath};
    struct Gated {
        inner: MemoryConnector,
        gate: Barrier,
        active: AtomicUsize,
        peak: AtomicUsize,
        panic_worker: bool,
    }
    impl StorageConnector for Gated {
        fn listing_concurrency(&self) -> NonZeroUsize {
            workers()
        }
        fn connector_id(&self) -> &ConnectorId {
            self.inner.connector_id()
        }
        fn capabilities(&self) -> &StorageCapabilities {
            self.inner.capabilities()
        }
        fn list(
            &self,
            directory: &ResourcePath,
            page: Option<&PageToken>,
            cancel: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            if directory.depth() == 1 && page.is_none() {
                if self.panic_worker {
                    panic!("injected worker failure");
                }
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                self.gate.wait();
                self.active.fetch_sub(1, Ordering::SeqCst);
            }
            self.inner.list(directory, page, cancel)
        }
        fn stat(&self, p: &ResourcePath, c: &CancellationToken) -> Result<FileEntry, StorageError> {
            self.inner.stat(p, c)
        }
        fn open_read(
            &self,
            p: &ResourcePath,
            c: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            self.inner.open_read(p, c)
        }
    }
    for panic_worker in [false, true] {
        let source = Gated {
            inner: MemoryConnector::new("gated")
                .dir("a")
                .dir("b")
                .dir("c")
                .dir("d"),
            gate: Barrier::new(4),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            panic_worker,
        };
        let result = scan(
            &source,
            &Filter::include_all(),
            &ScanOptions::new(),
            &CancellationToken::new(),
        );
        if panic_worker {
            assert!(
                matches!(result, Err(ScanError::WorkerFailed { partial }) if !partial.is_complete())
            );
        } else {
            assert!(result?.is_complete());
            assert_eq!(source.peak.load(Ordering::SeqCst), 4);
        }
    }
    Ok(())
}

#[test]
#[ignore = "performance: two inventories of 1,001,000 synthetic entries, no filesystem"]
fn million_entries_match_sequential_and_parallel() -> Result<(), Box<dyn std::error::Error>> {
    use connector_api::{
        ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector, StorageError,
    };
    use std::io::Write;
    use sync_core::{ConnectorId, EntryKind, FileEntry, FileMetadata, ResourcePath};
    struct Generated(MemoryConnector);
    impl StorageConnector for Generated {
        fn listing_concurrency(&self) -> NonZeroUsize {
            workers()
        }
        fn connector_id(&self) -> &ConnectorId {
            self.0.connector_id()
        }
        fn capabilities(&self) -> &StorageCapabilities {
            self.0.capabilities()
        }
        fn list(
            &self,
            directory: &ResourcePath,
            page: Option<&PageToken>,
            _: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            let start = page
                .and_then(|p| p.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            let end = (start + 256).min(1000);
            let entries = (start..end)
                .map(|n| {
                    if directory.is_root() {
                        FileEntry::new(
                            path(&format!("d{n:04}")),
                            EntryKind::Directory,
                            FileMetadata::unknown(),
                        )
                    } else {
                        FileEntry::new(
                            path(&format!("{directory}/f{n:04}")),
                            EntryKind::File,
                            FileMetadata::unknown().with_size(8),
                        )
                    }
                })
                .collect();
            Ok(ListPage::new(
                entries,
                vec![],
                (end < 1000).then(|| PageToken::new(end.to_string())),
            ))
        }
        fn stat(&self, _: &ResourcePath, _: &CancellationToken) -> Result<FileEntry, StorageError> {
            Err(StorageError::not_supported(StorageOperation::Stat))
        }
        fn open_read(
            &self,
            _: &ResourcePath,
            _: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            Err(StorageError::not_supported(StorageOperation::Read))
        }
    }
    let source = Generated(MemoryConnector::new("generated"));
    let start = std::time::Instant::now();
    let serial = scan(
        &source,
        &Filter::include_all(),
        &ScanOptions::new().with_listing_workers(NonZeroUsize::MIN),
        &CancellationToken::new(),
    )?;
    let serial_time = start.elapsed();
    let start = std::time::Instant::now();
    let parallel = scan(
        &source,
        &Filter::include_all(),
        &ScanOptions::new(),
        &CancellationToken::new(),
    )?;
    let parallel_time = start.elapsed();
    assert_eq!(serial.len(), 1_001_000);
    assert!(serial.is_complete() && parallel.is_complete());
    assert!(serial.entries().eq(parallel.entries()));
    assert!(serial.directories().eq(parallel.directories()));
    assert_eq!(serial.stats(), parallel.stats());
    assert_eq!(serial.issues(), parallel.issues());
    writeln!(
        std::io::stdout().lock(),
        "entries={} serial={serial_time:.3?} parallel={parallel_time:.3?}",
        serial.len()
    )?;
    Ok(())
}
