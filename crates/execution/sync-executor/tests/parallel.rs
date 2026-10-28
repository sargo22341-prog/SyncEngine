//! Parallel execution keeps the exact plan, per-file checks and durable ordering.
mod common;
use common::*;
use connector_api::{StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, ts};
use std::num::NonZeroUsize;
use sync_core::{CancellationToken, ExecutionId, ExecutionStatus};
use sync_executor::{ExecutionRequest, Executor, OutcomeStatus, PlanExecutor, Refusal, StopCause};
use sync_journal::{JournalRecord, MemoryJournal};

fn parallel() -> NonZeroUsize {
    NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN)
}
fn source() -> MemoryConnector {
    let mut source = MemoryConnector::new("source").with_transfer_concurrency(parallel());
    for n in 0..24 {
        source = source.file(&format!("d{}/f{n}", n % 4), &[n as u8; 8192], ts(10));
    }
    source
}
fn destination() -> MemoryConnector {
    MemoryConnector::new("destination")
        .writable()
        .with_read_back()
        .with_transfer_concurrency(parallel())
}
fn no_partial(destination: &MemoryConnector) {
    assert!(destination.paths().iter().all(|p| !p.contains(".partial")));
}

#[test]
fn a_worker_panic_is_fatal_and_cannot_authorize_mirror_deletions()
-> Result<(), Box<dyn std::error::Error>> {
    use connector_api::{
        ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector, StorageError,
    };
    use sync_core::{ConnectorId, FileEntry, ResourcePath};
    struct Broken(MemoryConnector);
    struct Reader;
    impl ContentReader for Reader {
        fn read(&mut self, _: &mut [u8]) -> Result<usize, StorageError> {
            panic!("deliberate worker failure");
        }
    }
    impl StorageConnector for Broken {
        fn transfer_concurrency(&self) -> NonZeroUsize {
            parallel()
        }
        fn connector_id(&self) -> &ConnectorId {
            self.0.connector_id()
        }
        fn capabilities(&self) -> &StorageCapabilities {
            self.0.capabilities()
        }
        fn list(
            &self,
            p: &ResourcePath,
            t: Option<&PageToken>,
            c: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            self.0.list(p, t, c)
        }
        fn stat(&self, p: &ResourcePath, c: &CancellationToken) -> Result<FileEntry, StorageError> {
            self.0.stat(p, c)
        }
        fn open_read(
            &self,
            _: &ResourcePath,
            _: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            Ok(Box::new(Reader))
        }
    }
    // Given a mirror with an obsolete destination file, When a copy worker
    // panics after staging starts, Then report failure and keep that file.
    let source = Broken(MemoryConnector::new("source").file("new", b"data", ts(10)));
    let destination = destination().file("old", b"preserve", ts(1));
    let plan = mirror_plan(&source, &destination);
    let mut journal = MemoryJournal::new();
    let report = execute(&plan, &source, &destination, confirmed(), &mut journal)?;
    assert_eq!(report.stop(), Some(StopCause::WorkerFailure));
    assert_eq!(report.totals().failed, 1);
    assert_eq!(destination.content("old"), Some(b"preserve".to_vec()));
    assert!(destination.content("new").is_none());
    no_partial(&destination);
    assert!(!journal.replay().recovery().is_interrupted());
    Ok(())
}

#[test]
fn parallel_matches_serial_contents_outcome_order_and_journal_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let source = source();
    let destination = destination();
    let plan = backup_plan(&source, &destination);
    let mut journal = MemoryJournal::new();
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed().with_copy_workers(parallel()),
        &mut journal,
    )?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(report.totals().done, 28);
    assert_eq!(report.totals().bytes, 24 * 8192);
    assert!(
        report
            .outcomes()
            .iter()
            .zip(plan.operations())
            .all(|(r, p)| r.operation() == p.id())
    );
    for n in 0..24 {
        assert_eq!(
            destination.content(&format!("d{}/f{n}", n % 4)),
            Some(vec![n as u8; 8192])
        );
    }
    assert!(!journal.replay().recovery().is_interrupted());
    no_partial(&destination);
    Ok(())
}

#[test]
fn parallel_fatal_failures_stop_all_workers_and_block_mirror_deletions()
-> Result<(), Box<dyn std::error::Error>> {
    // Given a destructive mirror and concurrent copy failures, When storage or
    // the journal fails, Then no deletion may occur and staging is cleaned up.
    for mode in 0..3 {
        let mut source = source();
        let mut destination = destination().file("obsolete", b"preserve", ts(1));
        if mode == 0 {
            source = source.fail_read_after("d0/f0", 0, StorageErrorKind::Disconnected);
        }
        if mode == 1 {
            destination = destination.with_space_limit(0);
        }
        let plan = mirror_plan(&source, &destination);
        let mut journal = if mode == 2 {
            MemoryJournal::failing_after(2)
        } else {
            MemoryJournal::new()
        };
        let parent = CancellationToken::new();
        let report = Executor.execute(
            ExecutionRequest::new(
                &plan,
                &source,
                &destination,
                ExecutionId::new(format!("failure-{mode}"))?,
                &mut journal,
                &parent,
            )
            .with_options(confirmed())
            .at(now()),
        )?;
        assert_eq!(report.status(), ExecutionStatus::Failed);
        assert!(matches!(
            report.stop(),
            Some(
                StopCause::SourceUnavailable
                    | StopCause::InsufficientSpace
                    | StopCause::JournalFailure
            )
        ));
        assert_eq!(destination.content("obsolete"), Some(b"preserve".to_vec()));
        assert!(
            !parent.is_cancelled(),
            "internal abort must not cancel caller"
        );
        no_partial(&destination);
    }
    Ok(())
}

#[test]
fn directory_failure_refuses_its_children_and_source_changes_are_not_forced()
-> Result<(), Box<dyn std::error::Error>> {
    let source = source();
    let destination = destination().fail(
        "d0",
        StorageOperation::CreateDirectory,
        StorageErrorKind::PermissionDenied,
    );
    let plan = backup_plan(&source, &destination);
    source.put_file("d1/f1", b"changed after preview", ts(20));
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )?;
    assert!(matches!(
        status(&report, "d0/f0"),
        OutcomeStatus::Refused {
            refusal: Refusal::DependencyNotDone { .. }
        }
    ));
    assert!(matches!(
        status(&report, "d1/f1"),
        OutcomeStatus::Refused {
            refusal: Refusal::SourceChanged { .. }
        }
    ));
    assert!(destination.content("d0/f0").is_none());
    assert!(destination.content("d1/f1").is_none());
    assert!(destination.content("d2/f2").is_some());
    no_partial(&destination);
    Ok(())
}

#[test]
fn independent_files_overlap_but_never_exceed_the_endpoint_bound()
-> Result<(), Box<dyn std::error::Error>> {
    use connector_api::{
        ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector, StorageError,
    };
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    use sync_core::{ConnectorId, FileEntry, ResourcePath};
    struct Gated {
        inner: MemoryConnector,
        gate: Arc<Barrier>,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }
    struct Reader {
        inner: Box<dyn ContentReader>,
        gate: Arc<Barrier>,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        first: bool,
    }
    impl ContentReader for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> Result<usize, StorageError> {
            if self.first {
                self.first = false;
                self.peak.fetch_max(
                    self.active.fetch_add(1, Ordering::SeqCst) + 1,
                    Ordering::SeqCst,
                );
                self.gate.wait();
                self.active.fetch_sub(1, Ordering::SeqCst);
            }
            self.inner.read(bytes)
        }
    }
    impl StorageConnector for Gated {
        fn transfer_concurrency(&self) -> NonZeroUsize {
            parallel()
        }
        fn connector_id(&self) -> &ConnectorId {
            self.inner.connector_id()
        }
        fn capabilities(&self) -> &StorageCapabilities {
            self.inner.capabilities()
        }
        fn list(
            &self,
            p: &ResourcePath,
            t: Option<&PageToken>,
            c: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            self.inner.list(p, t, c)
        }
        fn stat(&self, p: &ResourcePath, c: &CancellationToken) -> Result<FileEntry, StorageError> {
            self.inner.stat(p, c)
        }
        fn open_read(
            &self,
            p: &ResourcePath,
            c: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            Ok(Box::new(Reader {
                inner: self.inner.open_read(p, c)?,
                gate: self.gate.clone(),
                active: self.active.clone(),
                peak: self.peak.clone(),
                first: true,
            }))
        }
    }
    let source = Gated {
        inner: MemoryConnector::new("source")
            .file("a", b"a", ts(10))
            .file("b", b"b", ts(10))
            .file("c", b"c", ts(10))
            .file("d", b"d", ts(10)),
        gate: Arc::new(Barrier::new(4)),
        active: Arc::default(),
        peak: Arc::default(),
    };
    let destination = destination()
        .file("a", b"old", ts(1))
        .file("b", b"old", ts(1))
        .file("c", b"old", ts(1))
        .file("d", b"old", ts(1));
    let plan = mirror_plan(&source, &destination);
    let versions = MemoryConnector::new("versions")
        .writable()
        .with_transfer_concurrency(parallel());
    let store = sync_versioning::VersionStore::new(&versions, connector_memory::path("run"))?;
    let options = confirmed().with_copy_workers(NonZeroUsize::new(16).ok_or("workers")?);
    let report = Executor.execute(
        ExecutionRequest::new(
            &plan,
            &source,
            &destination,
            ExecutionId::new("parallel-version-overlap")?,
            &mut MemoryJournal::new(),
            &CancellationToken::new(),
        )
        .with_versions(&store)
        .with_options(options)
        .at(now()),
    )?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(source.peak.load(Ordering::SeqCst), 4);
    for file in ["a", "b", "c", "d"] {
        assert_eq!(
            versions.content(&format!("run/{file}")),
            Some(b"old".to_vec())
        );
    }
    Ok(())
}

#[test]
fn deletion_records_follow_every_parallel_copy_completion() -> Result<(), Box<dyn std::error::Error>>
{
    let source = source();
    let destination = destination().file("old", b"old", ts(1));
    let plan = mirror_plan(&source, &destination);
    let mut journal = MemoryJournal::new();
    let report = execute(&plan, &source, &destination, confirmed(), &mut journal)?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    let mut deletion_started = false;
    for line in journal.lines() {
        match line.record() {
            JournalRecord::OperationStarted {
                action: sync_plan::Action::Delete,
                ..
            } => deletion_started = true,
            JournalRecord::OperationFinished {
                action: sync_plan::Action::Copy,
                ..
            } => assert!(!deletion_started),
            _ => {}
        }
    }
    assert!(deletion_started);
    assert!(destination.content("old").is_none());
    Ok(())
}

#[test]
fn cancellation_keeps_monotonic_progress_and_a_new_preview_can_resume()
-> Result<(), Box<dyn std::error::Error>> {
    let source = source();
    let destination = destination();
    let plan = backup_plan(&source, &destination);
    let cancel = CancellationToken::new();
    let recorder = Recorder::new(Some((7, cancel.clone())));
    let emitter = recorder.emitter();
    let report = Executor.execute(
        ExecutionRequest::new(
            &plan,
            &source,
            &destination,
            ExecutionId::new("cancel-parallel")?,
            &mut MemoryJournal::new(),
            &cancel,
        )
        .with_options(confirmed())
        .with_events(&emitter)
        .at(now()),
    )?;
    assert_eq!(report.status(), ExecutionStatus::Cancelled);
    let progress: Vec<_> = recorder
        .events()
        .iter()
        .filter_map(|e| match e.kind() {
            sync_events::EventKind::Progress(p) => Some(*p),
            _ => None,
        })
        .collect();
    assert!(
        progress
            .windows(2)
            .all(|p| p[0].bytes_done <= p[1].bytes_done && p[0].entries_done <= p[1].entries_done)
    );
    no_partial(&destination);
    let resumed = backup_plan(&source, &destination);
    let report = execute(
        &resumed,
        &source,
        &destination,
        confirmed(),
        &mut MemoryJournal::new(),
    )?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    for n in 0..24 {
        assert_eq!(
            destination.content(&format!("d{}/f{n}", n % 4)),
            Some(vec![n as u8; 8192])
        );
    }
    Ok(())
}

#[test]
fn versioned_replacements_keep_the_original_while_new_files_use_the_pool()
-> Result<(), Box<dyn std::error::Error>> {
    let source = source().file("existing", b"new content", ts(20));
    let destination = destination().file("existing", b"old", ts(1));
    let versions = MemoryConnector::new("versions").writable();
    let store = sync_versioning::VersionStore::new(&versions, p("run"))?;
    let plan = mirror_plan(&source, &destination);
    let report = execute_with_versions(
        &plan,
        &source,
        &destination,
        &store,
        &mut MemoryJournal::new(),
    )?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(
        destination.content("existing"),
        Some(b"new content".to_vec())
    );
    assert_eq!(versions.content("run/existing"), Some(b"old".to_vec()));
    assert!(is_done(&report, "d0/f0"));
    no_partial(&destination);
    Ok(())
}

#[test]
fn verification_and_archiving_are_visible_without_inflating_transfer_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let bytes = vec![3; 2 * 1024 * 1024];
    let old = vec![1; 1024 * 1024];
    let source = MemoryConnector::new("source")
        .with_transfer_concurrency(parallel())
        .file("d/a", &bytes, ts(20))
        .file("d/b", &bytes, ts(20));
    let destination = destination()
        .file("d/a", &old, ts(1))
        .file("d/b", &old, ts(1));
    let versions = MemoryConnector::new("versions")
        .writable()
        .with_transfer_concurrency(parallel());
    let store = sync_versioning::VersionStore::new(&versions, p("run"))?;
    let plan = mirror_plan(&source, &destination);
    let recorder = Recorder::new(None);
    let emitter = recorder.emitter();
    let report = Executor.execute(
        ExecutionRequest::new(
            &plan,
            &source,
            &destination,
            ExecutionId::new("auxiliary-io")?,
            &mut MemoryJournal::new(),
            &CancellationToken::new(),
        )
        .with_options(confirmed())
        .with_versions(&store)
        .with_events(&emitter)
        .at(now()),
    )?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    let events = recorder.events();
    let (mut verified, mut versioned) = (0, 0);
    for event in &events {
        if let sync_events::EventKind::IoActivity {
            verification_bytes,
            version_bytes,
        } = event.kind()
        {
            verified += verification_bytes;
            versioned += version_bytes;
        }
    }
    assert_eq!(verified, report.totals().bytes);
    assert_eq!(versioned, 2 * old.len() as u64);
    let first_io=events.iter().position(|e|matches!(e.kind(),sync_events::EventKind::IoActivity{verification_bytes,..} if *verification_bytes>0)).ok_or("verification invisible")?;
    let first_done = events
        .iter()
        .position(|e| matches!(e.kind(),sync_events::EventKind::Progress(p) if p.entries_done>0))
        .ok_or("completion missing")?;
    assert!(first_io < first_done);
    no_partial(&destination);
    Ok(())
}

/// Given three large and four small files with four workers, When the plan
/// runs, Then only one large file is transferred at a time while the small
/// ones go on in the other workers.
#[test]
fn large_files_are_transferred_one_at_a_time_beside_small_ones()
-> Result<(), Box<dyn std::error::Error>> {
    use connector_api::{
        ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector, StorageError,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::{Duration, Instant};
    use sync_core::{ConnectorId, FileEntry, ResourcePath};
    use sync_executor::LARGE_TRANSFER_BYTES;
    #[derive(Default)]
    struct Counters {
        large_active: AtomicUsize,
        large_peak: AtomicUsize,
        small_done: AtomicUsize,
        overlapped: AtomicBool,
    }
    struct Observed {
        inner: MemoryConnector,
        counters: Arc<Counters>,
    }
    struct Reader {
        inner: Box<dyn ContentReader>,
        counters: Arc<Counters>,
        large: bool,
        started: bool,
    }
    impl ContentReader for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> Result<usize, StorageError> {
            let c = &self.counters;
            if !self.started {
                self.started = true;
                if self.large {
                    let active = c.large_active.fetch_add(1, Ordering::SeqCst) + 1;
                    c.large_peak.fetch_max(active, Ordering::SeqCst);
                    // The small files finish meanwhile (bounded wait).
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while c.small_done.load(Ordering::SeqCst) < 4 && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if c.small_done.load(Ordering::SeqCst) == 4 {
                        c.overlapped.store(true, Ordering::SeqCst);
                    }
                }
            }
            let read = self.inner.read(bytes)?;
            if read == 0 {
                if self.large {
                    c.large_active.fetch_sub(1, Ordering::SeqCst);
                } else {
                    c.small_done.fetch_add(1, Ordering::SeqCst);
                }
            }
            Ok(read)
        }
    }
    impl StorageConnector for Observed {
        fn transfer_concurrency(&self) -> NonZeroUsize {
            parallel()
        }
        fn connector_id(&self) -> &ConnectorId {
            self.inner.connector_id()
        }
        fn capabilities(&self) -> &StorageCapabilities {
            self.inner.capabilities()
        }
        fn list(
            &self,
            p: &ResourcePath,
            t: Option<&PageToken>,
            c: &CancellationToken,
        ) -> Result<ListPage, StorageError> {
            self.inner.list(p, t, c)
        }
        fn stat(&self, p: &ResourcePath, c: &CancellationToken) -> Result<FileEntry, StorageError> {
            self.inner.stat(p, c)
        }
        fn open_read(
            &self,
            p: &ResourcePath,
            c: &CancellationToken,
        ) -> Result<Box<dyn ContentReader>, StorageError> {
            Ok(Box::new(Reader {
                inner: self.inner.open_read(p, c)?,
                counters: self.counters.clone(),
                large: p.as_str().starts_with("big"),
                started: false,
            }))
        }
    }
    let big = vec![1_u8; usize::try_from(LARGE_TRANSFER_BYTES)?];
    let mut inner = MemoryConnector::new("source");
    for n in 0..3 {
        inner = inner.file(&format!("big{n}"), &big, ts(10));
    }
    for n in 0..4 {
        inner = inner.file(&format!("small{n}"), &[n; 64], ts(10));
    }
    let source = Observed {
        inner,
        counters: Arc::default(),
    };
    let destination = destination();
    let plan = backup_plan(&source, &destination);
    let report = execute(
        &plan,
        &source,
        &destination,
        confirmed().with_copy_workers(parallel()),
        &mut MemoryJournal::new(),
    )?;
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert_eq!(report.totals().done, 7);
    assert_eq!(source.counters.large_peak.load(Ordering::SeqCst), 1);
    assert!(source.counters.overlapped.load(Ordering::SeqCst));
    for n in 0..3 {
        assert_eq!(destination.content(&format!("big{n}")), Some(big.clone()));
    }
    no_partial(&destination);
    Ok(())
}
