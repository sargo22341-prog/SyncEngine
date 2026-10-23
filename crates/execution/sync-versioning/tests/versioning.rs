//! Magasin de versions sur stockage mémoire.

use connector_api::StorageErrorKind;
use connector_memory::{MemoryConnector, path, ts};
use sync_core::{CancellationToken, FileMetadata, ResourcePath};
use sync_versioning::{VersionStore, VersioningError};

fn destination() -> MemoryConnector {
    MemoryConnector::new("destination")
        .file("docs/a/rapport.txt", b"version 1", ts(10))
        .file("b.txt", b"b", ts(20))
}

#[test]
fn preserves_nested_files_with_their_date() {
    let destination = destination();
    let versions = MemoryConnector::new("versions").writable();
    let store = VersionStore::new(&versions, path("exec-1")).unwrap();
    let cancel = CancellationToken::new();
    let observed = destination.entry_at("docs/a/rapport.txt").unwrap();
    let kept = store
        .preserve(
            &destination,
            &path("docs/a/rapport.txt"),
            observed.metadata(),
            &cancel,
        )
        .unwrap();
    assert_eq!(kept.location().as_str(), "exec-1/docs/a/rapport.txt");
    assert_eq!(kept.bytes(), 9);
    let entry = versions.entry_at("exec-1/docs/a/rapport.txt").unwrap();
    assert_eq!(entry.metadata().modified(), Some(ts(10)));
    assert_eq!(
        versions.content("exec-1/docs/a/rapport.txt").unwrap(),
        b"version 1"
    );
    // Un second fichier réutilise les dossiers existants.
    let observed = destination.entry_at("b.txt").unwrap();
    store
        .preserve(&destination, &path("b.txt"), observed.metadata(), &cancel)
        .unwrap();
    assert_eq!(
        versions.paths(),
        [
            "exec-1",
            "exec-1/b.txt",
            "exec-1/docs",
            "exec-1/docs/a",
            "exec-1/docs/a/rapport.txt"
        ]
    );
    // La source de la version n'est jamais modifiée.
    assert_eq!(destination.mutations(), 0);
}

#[test]
fn an_existing_version_is_never_overwritten() {
    let destination = destination();
    let versions =
        MemoryConnector::new("versions")
            .writable()
            .file("exec-1/b.txt", b"deja la", ts(1));
    let store = VersionStore::new(&versions, path("exec-1")).unwrap();
    let observed = destination.entry_at("b.txt").unwrap();
    let error = store
        .preserve(
            &destination,
            &path("b.txt"),
            observed.metadata(),
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert!(matches!(&error, VersioningError::Store(e) if e.kind() == StorageErrorKind::Conflict));
    assert_eq!(versions.content("exec-1/b.txt").unwrap(), b"deja la");
    assert!(versions.paths().iter().all(|p| !p.contains(".partial")));
}

#[test]
fn a_changed_file_is_not_preserved() {
    let destination = destination();
    let versions = MemoryConnector::new("versions").writable();
    let store = VersionStore::new(&versions, path("exec-1")).unwrap();
    // Observation périmée : taille différente de l'actuelle.
    let stale = FileMetadata::unknown().with_size(3);
    let error = store
        .preserve(
            &destination,
            &path("b.txt"),
            &stale,
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert!(matches!(error, VersioningError::Changed(_)));
    assert_eq!(versions.paths(), ["exec-1"]);
}

#[test]
fn store_must_be_writable_and_not_the_root() {
    let read_only = MemoryConnector::new("versions");
    assert!(matches!(
        VersionStore::new(&read_only, path("x")),
        Err(VersioningError::Store(e)) if e.kind() == StorageErrorKind::NotSupported
    ));
    let writable = MemoryConnector::new("versions").writable();
    assert!(matches!(
        VersionStore::new(&writable, ResourcePath::root()),
        Err(VersioningError::RootFolder)
    ));
}
