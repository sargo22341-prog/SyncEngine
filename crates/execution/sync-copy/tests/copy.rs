//! Transferts en flux sur stockage mémoire avec injection de pannes.

use connector_api::{CommitMode, StorageErrorKind, StorageOperation, WriteOptions};
use connector_memory::{MemoryConnector, path, ts};
use sync_copy::{CopyRequest, CopyStage, stage_copy};
use sync_core::{CancellationToken, ContentHash, HashAlgorithm};

fn big() -> Vec<u8> {
    (0..10_000u32).map(|i| (i % 251) as u8).collect()
}

fn source() -> MemoryConnector {
    MemoryConnector::new("source")
        .file("gros.bin", &big(), ts(100))
        .file("petit.txt", b"abc", ts(50))
}

fn no_staging_left(destination: &MemoryConnector) {
    assert!(
        destination.paths().iter().all(|p| !p.contains(".partial")),
        "{:?}",
        destination.paths()
    );
}

#[test]
fn streams_in_blocks_hashes_and_commits() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let at = path("gros.bin");
    let request = CopyRequest::new(&source, &at, &destination, &at)
        .with_buffer_size(1024)
        .with_options(WriteOptions::new().with_modified(Some(ts(100))));
    let mut blocks = Vec::new();
    let staged = stage_copy(&request, &mut |n| blocks.push(n), &CancellationToken::new()).unwrap();
    assert_eq!(staged.bytes(), 10_000);
    assert_eq!(blocks.len(), 10);
    assert!(blocks.iter().all(|&n| n <= 1024));
    let expected = ContentHash::new(
        HashAlgorithm::Blake3,
        blake3::hash(&big()).as_bytes().to_vec(),
    )
    .unwrap();
    assert_eq!(staged.hash(), &expected);
    // Pas encore visible à la cible.
    assert_eq!(destination.content("gros.bin"), None);
    let staging = staged.staging_path().unwrap().clone();
    assert!(destination.content(staging.as_str()).is_some());

    let entry = staged.commit(CommitMode::CreateNew).unwrap();
    assert_eq!(entry.metadata().size(), Some(10_000));
    assert_eq!(entry.metadata().modified(), Some(ts(100)));
    assert_eq!(destination.content("gros.bin").unwrap(), big());
    no_staging_left(&destination);
}

#[test]
fn abort_leaves_nothing() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let at = path("petit.txt");
    let request = CopyRequest::new(&source, &at, &destination, &at);
    let staged = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap();
    staged.abort().unwrap();
    assert!(destination.paths().is_empty());
}

#[test]
fn read_failure_midway_abandons_the_staging_file() {
    let source = source().fail_read_after("gros.bin", 4096, StorageErrorKind::Disconnected);
    let destination = MemoryConnector::new("destination").writable();
    let at = path("gros.bin");
    let request = CopyRequest::new(&source, &at, &destination, &at).with_buffer_size(1024);
    let error = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.stage(), CopyStage::Read);
    assert_eq!(error.kind(), StorageErrorKind::Disconnected);
    assert!(error.staging_path().is_some());
    assert!(error.cleanup_error().is_none());
    assert!(destination.paths().is_empty());
}

#[test]
fn disk_full_is_reported_and_cleaned() {
    let source = source();
    let destination = MemoryConnector::new("destination")
        .writable()
        .with_space_limit(5_000);
    let at = path("gros.bin");
    let request = CopyRequest::new(&source, &at, &destination, &at).with_buffer_size(1024);
    let error = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.stage(), CopyStage::Write);
    assert_eq!(error.kind(), StorageErrorKind::InsufficientSpace);
    assert!(destination.paths().is_empty());
}

#[test]
fn cancellation_between_blocks() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let at = path("gros.bin");
    let request = CopyRequest::new(&source, &at, &destination, &at).with_buffer_size(1024);
    let cancel = CancellationToken::new();
    let mut seen = 0u64;
    let error = stage_copy(
        &request,
        &mut |n| {
            seen += n;
            if seen >= 3072 {
                cancel.cancel();
            }
        },
        &cancel,
    )
    .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Cancelled);
    assert_eq!(seen, 3072);
    assert!(destination.paths().is_empty());
}

#[test]
fn opening_failures_are_attributed() {
    let destination = MemoryConnector::new("destination").writable();
    let at = path("absent.txt");
    let source = source();
    let request = CopyRequest::new(&source, &at, &destination, &at);
    let error = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.stage(), CopyStage::OpenSource);
    assert_eq!(error.kind(), StorageErrorKind::NotFound);

    // Destination en lecture seule : refus explicite, aucune écriture.
    let read_only = MemoryConnector::new("destination");
    let at = path("petit.txt");
    let request = CopyRequest::new(&source, &at, &read_only, &at);
    let error = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.stage(), CopyStage::OpenDestination);
    assert_eq!(error.kind(), StorageErrorKind::NotSupported);

    let refusing = MemoryConnector::new("destination").writable().fail(
        "petit.txt",
        StorageOperation::Write,
        StorageErrorKind::PermissionDenied,
    );
    let request = CopyRequest::new(&source, &at, &refusing, &at);
    let error = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::PermissionDenied);
    assert!(refusing.paths().is_empty());
}

#[test]
fn empty_file_is_copied() {
    let source = MemoryConnector::new("source").file("vide", b"", ts(1));
    let destination = MemoryConnector::new("destination").writable();
    let at = path("vide");
    let request = CopyRequest::new(&source, &at, &destination, &at);
    let staged = stage_copy(&request, &mut |_| {}, &CancellationToken::new()).unwrap();
    assert_eq!(staged.bytes(), 0);
    staged.commit(CommitMode::CreateNew).unwrap();
    assert_eq!(destination.content("vide").unwrap(), b"");
}

#[test]
fn staging_is_observed_before_content_and_observer_failure_aborts() {
    let source = source();
    let destination = MemoryConnector::new("destination").writable();
    let at = path("petit.txt");
    let request = CopyRequest::new(&source, &at, &destination, &at);
    let mut observed = false;
    let error = sync_copy::stage_copy_observed(
        &request,
        &mut |_| {},
        &mut |staging| {
            let staging = staging.ok_or_else(|| {
                connector_api::StorageError::new(
                    StorageErrorKind::Permanent,
                    StorageOperation::Write,
                )
            })?;
            assert_eq!(destination.content(staging.as_str()), Some(Vec::new()));
            observed = true;
            Err(connector_api::StorageError::new(
                StorageErrorKind::PermissionDenied,
                StorageOperation::Write,
            ))
        },
        &CancellationToken::new(),
    )
    .unwrap_err();
    assert!(observed);
    assert_eq!(error.kind(), StorageErrorKind::PermissionDenied);
    assert!(destination.paths().is_empty());
}

#[test]
fn pause_mid_transfer_resumes_same_stream_or_cancels_without_publication() {
    use std::sync::mpsc;
    use std::time::Duration;

    for cancel in [false, true] {
        let source = source();
        let destination = MemoryConnector::new("destination").writable();
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let (paused_tx, paused_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let destination = &destination;
            let worker = scope.spawn(move || {
                let at = path("gros.bin");
                let request =
                    CopyRequest::new(&source, &at, destination, &at).with_buffer_size(1024);
                let mut blocks = 0;
                let staged = stage_copy(
                    &request,
                    &mut |_| {
                        blocks += 1;
                        if blocks == 1 {
                            worker_token.set_paused(true);
                            paused_tx.send(()).unwrap();
                        }
                    },
                    &worker_token,
                );
                let succeeded = match staged {
                    Ok(staged) => {
                        staged.commit(CommitMode::CreateNew).unwrap();
                        true
                    }
                    Err(e) => {
                        assert_eq!(e.kind(), StorageErrorKind::Cancelled);
                        false
                    }
                };
                done_tx.send((succeeded, blocks)).unwrap();
            });
            paused_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let blocked = done_rx.recv_timeout(Duration::from_millis(40)).is_err();
            let unpublished = destination.content("gros.bin").is_none();
            if cancel {
                token.cancel();
            } else {
                token.set_paused(false);
            }
            let (succeeded, blocks) = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            worker.join().unwrap();
            assert!(blocked && unpublished);
            assert_eq!(succeeded, !cancel);
            assert_eq!(blocks, if cancel { 1 } else { 10 });
        });
        assert_eq!(
            destination.content("gros.bin"),
            if cancel { None } else { Some(big()) }
        );
        no_staging_left(&destination);
    }
}
