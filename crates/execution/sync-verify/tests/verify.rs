//! Relecture de contenu sur stockage mémoire.

#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

use connector_api::StorageErrorKind;
use connector_memory::{MemoryConnector, path, ts};
use sync_core::{CancellationToken, ContentHash, HashAlgorithm};
use sync_verify::{Mismatch, VerifyError, verify_content};

fn blake3_of(content: &[u8]) -> ContentHash {
    ContentHash::new(
        HashAlgorithm::Blake3,
        blake3::hash(content).as_bytes().to_vec(),
    )
    .unwrap()
}

#[test]
fn bounded_verification_ignores_only_a_tail_and_rejects_changed_or_short_prefixes() {
    let source = MemoryConnector::new("m")
        .file("growing", b"prefix plus a growing tail", ts(1))
        .file("changed", b"PREFIX plus a tail", ts(1))
        .file("short", b"pre", ts(1));
    let hash = blake3_of(b"prefix");
    let cancel = CancellationToken::new();
    sync_verify::verify_prefix(&source, &path("growing"), &hash, 6, 2, &cancel).unwrap();
    for name in ["changed", "short"] {
        assert!(matches!(
            sync_verify::verify_prefix(&source, &path(name), &hash, 6, 2, &cancel),
            Err(VerifyError::Mismatch(_))
        ));
    }
}

#[test]
fn identical_content_passes_and_differences_are_reported() {
    let content: Vec<u8> = (0..5000u32).map(|i| (i % 7) as u8).collect();
    let store = MemoryConnector::new("m").file("bon", &content, ts(1)).file(
        "court",
        &content[..4999],
        ts(1),
    );
    let cancel = CancellationToken::new();
    let hash = blake3_of(&content);
    verify_content(&store, &path("bon"), &hash, 5000, 512, &cancel).unwrap();

    let error = verify_content(&store, &path("court"), &hash, 5000, 512, &cancel).unwrap_err();
    assert!(matches!(
        error,
        VerifyError::Mismatch(Mismatch::Size {
            expected: 5000,
            found: Some(4999)
        })
    ));
    let error =
        verify_content(&store, &path("bon"), &blake3_of(b"x"), 5000, 512, &cancel).unwrap_err();
    assert!(matches!(
        error,
        VerifyError::Mismatch(Mismatch::Content { .. })
    ));
}

#[test]
fn read_errors_are_not_mismatches() {
    let store = MemoryConnector::new("m")
        .file("f", b"abcdef", ts(1))
        .fail_read_after("f", 3, StorageErrorKind::Disconnected);
    let error = verify_content(
        &store,
        &path("f"),
        &blake3_of(b"abcdef"),
        6,
        2,
        &CancellationToken::new(),
    )
    .unwrap_err();
    match error {
        VerifyError::Storage(error) => assert_eq!(error.kind(), StorageErrorKind::Disconnected),
        other => panic!("{other:?}"),
    }
    let md5 = ContentHash::new(HashAlgorithm::Md5, vec![0; 16]).unwrap();
    let error =
        verify_content(&store, &path("f"), &md5, 6, 2, &CancellationToken::new()).unwrap_err();
    assert!(matches!(error, VerifyError::Storage(e) if e.kind() == StorageErrorKind::NotSupported));
}
