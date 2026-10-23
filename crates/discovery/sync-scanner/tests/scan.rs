//! Tests du scanner sur stockage mémoire avec pannes injectées.

use connector_api::{
    ListingIssue, ListingIssueKind, NameIssue, StorageCapabilities, StorageErrorKind,
    StorageFeature, StorageOperation,
};
use connector_memory::{MemoryConnector, path, ts};
use sync_core::{
    CancellationToken, ContentHash, EntryKind, FileEntry, FileMetadata, HashAlgorithm,
};
use sync_filter::{Exclusion, Filter, FilterRules};
use sync_scanner::{
    DirectoryState, IncompleteReason, Presence, ScanError, ScanIssueKind, ScanOptions,
    UnknownReason, scan,
};

fn scan_all(connector: &MemoryConnector) -> Result<sync_scanner::Inventory, ScanError> {
    scan(
        connector,
        &Filter::include_all(),
        &ScanOptions::new(),
        &CancellationToken::new(),
    )
}

#[test]
fn exhausted_entry_budget_never_becomes_an_empty_complete_inventory() {
    let connector = sample();
    let result = scan(
        &connector,
        &Filter::include_all(),
        &ScanOptions::new().with_entry_limit(std::num::NonZeroUsize::MIN),
        &CancellationToken::new(),
    );
    assert!(matches!(result, Err(ScanError::RootUnavailable(_))));
}

fn sample() -> MemoryConnector {
    MemoryConnector::new("mem")
        .file("a.txt", b"alpha", ts(100))
        .file("docs/b.md", b"bravo", ts(200))
        .file("docs/sub/c.bin", &[0u8; 1000], ts(300))
        .dir("vide")
}

#[test]
fn empty_root_is_complete_and_empty() {
    let inventory = scan_all(&MemoryConnector::new("mem")).unwrap();
    assert!(inventory.is_empty());
    assert!(inventory.is_complete());
    // Vide ET complet : l'absence est établie.
    assert_eq!(inventory.presence(&path("x")), Presence::Absent);
}

#[test]
fn unknown_or_eventual_consistency_never_proves_absence() -> Result<(), Box<dyn std::error::Error>>
{
    use connector_api::{ListingConsistency, StorageConnector};
    for consistency in [ListingConsistency::Unknown, ListingConsistency::Eventual] {
        let connector = MemoryConnector::new("network-like").file("present", b"data", ts(1));
        let mut caps = connector.capabilities().clone();
        caps.listing_consistency = consistency;
        let inventory = scan_all(&connector.with_capabilities(caps))?;
        assert!(!inventory.is_complete());
        assert!(matches!(
            inventory.presence(&path("present")),
            Presence::Present(_)
        ));
        assert_eq!(
            inventory.presence(&path("missing")),
            Presence::Unknown(UnknownReason::IncompleteDirectory(
                IncompleteReason::UnreliableConsistency
            ))
        );
    }
    Ok(())
}

#[test]
fn inventories_nested_tree_and_empty_directory() {
    let inventory = scan_all(&sample()).unwrap();
    let paths: Vec<&str> = inventory.entries().map(|e| e.path().as_str()).collect();
    assert_eq!(
        paths,
        [
            "a.txt",
            "docs",
            "docs/b.md",
            "docs/sub",
            "docs/sub/c.bin",
            "vide"
        ]
    );
    assert!(inventory.is_complete());
    let stats = inventory.stats();
    assert_eq!((stats.files, stats.directories), (3, 3));
    assert_eq!(stats.known_bytes, 1010);
    assert_eq!(
        inventory.get(&path("docs/b.md")).unwrap().metadata().size(),
        Some(5)
    );
    assert_eq!(inventory.presence(&path("vide/rien")), Presence::Absent);
    assert_eq!(inventory.presence(&path("docs/sub/zz")), Presence::Absent);
    // Sous un fichier, rien ne peut exister.
    assert_eq!(inventory.presence(&path("a.txt/x")), Presence::Absent);
    // Sous un répertoire absent d'un répertoire complet : absent aussi.
    assert_eq!(inventory.presence(&path("nouveau/x/y")), Presence::Absent);
}

#[test]
fn root_failure_is_an_error_not_an_empty_inventory() {
    for kind in [
        StorageErrorKind::Disconnected,
        StorageErrorKind::PermissionDenied,
        StorageErrorKind::NotFound,
    ] {
        let connector = sample().fail("", StorageOperation::List, kind);
        match scan_all(&connector) {
            Err(ScanError::RootUnavailable(error)) => assert_eq!(error.kind(), kind),
            other => panic!("attendu RootUnavailable, obtenu {other:?}"),
        }
    }
}

#[test]
fn access_denied_subdirectory_is_incomplete_and_scan_continues() {
    let connector = sample().fail(
        "docs",
        StorageOperation::List,
        StorageErrorKind::PermissionDenied,
    );
    let inventory = scan_all(&connector).unwrap();
    assert!(!inventory.is_complete());
    let incomplete: Vec<_> = inventory.incomplete_directories().collect();
    assert_eq!(
        incomplete,
        [(
            &path("docs"),
            IncompleteReason::ListFailed(StorageErrorKind::PermissionDenied)
        )]
    );
    // Le répertoire lui-même est vu, son contenu est inconnu.
    assert!(inventory.get(&path("docs")).is_some());
    assert!(inventory.get(&path("docs/b.md")).is_none());
    assert_eq!(
        inventory.presence(&path("docs/b.md")),
        Presence::Unknown(UnknownReason::IncompleteDirectory(
            IncompleteReason::ListFailed(StorageErrorKind::PermissionDenied)
        ))
    );
    assert_eq!(
        inventory.presence(&path("docs/sub/c.bin")),
        Presence::Unknown(UnknownReason::IncompleteDirectory(
            IncompleteReason::ListFailed(StorageErrorKind::PermissionDenied)
        ))
    );
    // Le reste est inventorié normalement.
    assert!(inventory.get(&path("a.txt")).is_some());
    assert_eq!(inventory.presence(&path("vide/x")), Presence::Absent);
    assert!(inventory.issues().iter().any(|issue| {
        issue.path() == &path("docs")
            && *issue.kind() == ScanIssueKind::ListFailed(StorageErrorKind::PermissionDenied)
    }));
}

#[test]
fn disconnection_mid_scan_never_looks_like_absence() {
    let connector = sample().fail(
        "docs/sub",
        StorageOperation::List,
        StorageErrorKind::Disconnected,
    );
    let inventory = scan_all(&connector).unwrap();
    assert!(matches!(
        inventory.presence(&path("docs/sub/c.bin")),
        Presence::Unknown(_)
    ));
    assert!(inventory.get(&path("docs/b.md")).is_some());
}

#[test]
fn pagination_is_followed_to_the_end() {
    let mut connector = MemoryConnector::new("mem").with_page_size(2);
    for i in 0..7 {
        connector = connector.file(&format!("f{i}"), b"x", ts(1));
    }
    let inventory = scan_all(&connector).unwrap();
    assert_eq!(inventory.len(), 7);
    assert!(inventory.is_complete());
    // 4 pages pour la racine.
    assert_eq!(connector.list_calls(), 4);
}

#[test]
fn filter_excludes_files_and_does_not_descend_into_excluded_directories() {
    let connector = sample()
        .file("docs/tmp/cache.dat", b"zz", ts(1))
        .file("x.tmp", b"t", ts(1));
    let rules = FilterRules::new()
        .exclude_file("*.tmp")
        .exclude_directory("tmp");
    let filter = Filter::new(rules.clone()).unwrap();
    let inventory = scan(
        &connector,
        &filter,
        &ScanOptions::new(),
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(inventory.get(&path("x.tmp")).is_none());
    assert!(inventory.get(&path("docs/tmp")).is_none());
    assert!(inventory.get(&path("docs/tmp/cache.dat")).is_none());
    assert_eq!(inventory.presence(&path("x.tmp")), Presence::Excluded);
    assert_eq!(
        inventory.presence(&path("docs/tmp/cache.dat")),
        Presence::Excluded
    );
    assert_eq!(inventory.filter_rules(), &rules);
    let excluded: Vec<_> = inventory
        .excluded()
        .map(|(p, k, _)| (p.as_str(), k))
        .collect();
    assert_eq!(
        excluded,
        [
            ("docs/tmp", EntryKind::Directory),
            ("x.tmp", EntryKind::File)
        ]
    );
    assert!(
        inventory
            .excluded()
            .any(|(_, _, e)| e == Exclusion::ExcludedDirectory { rule: 0 })
    );
    // Le contenu du répertoire exclu n'est pas listé.
    assert_eq!(connector.list_calls(), 4);
    assert!(inventory.is_complete());
}

#[test]
fn symlinks_are_inventoried_but_not_followed() {
    let connector = sample().symlink("lien");
    let inventory = scan_all(&connector).unwrap();
    assert_eq!(
        inventory.get(&path("lien")).map(FileEntry::kind),
        Some(EntryKind::Symlink)
    );
    assert_eq!(
        inventory.presence(&path("lien/cible")),
        Presence::Unknown(UnknownReason::BehindUnfollowedEntry)
    );
    assert_eq!(inventory.stats().others, 1);
}

#[test]
fn unrepresentable_names_make_directory_incomplete() {
    let connector = sample().listing_issue(
        "docs",
        ListingIssue::new(
            "nom\u{fffd}",
            ListingIssueKind::UnrepresentableName(NameIssue::NonUtf8),
        ),
    );
    let inventory = scan_all(&connector).unwrap();
    assert_eq!(
        inventory
            .directories()
            .find(|(p, _)| *p == &path("docs"))
            .map(|(_, s)| s),
        Some(DirectoryState::Incomplete(IncompleteReason::OmittedEntries))
    );
    assert!(matches!(
        inventory.presence(&path("docs/autre")),
        Presence::Unknown(_)
    ));
    // Les entrées représentables restent inventoriées.
    assert!(inventory.get(&path("docs/b.md")).is_some());
}

#[test]
fn metadata_issue_keeps_directory_complete() {
    let connector = sample().listing_issue(
        "",
        ListingIssue::new(
            "a.txt",
            ListingIssueKind::MetadataUnavailable(StorageErrorKind::PermissionDenied),
        ),
    );
    let inventory = scan_all(&connector).unwrap();
    assert!(inventory.is_complete());
    assert_eq!(inventory.issues().len(), 1);
}

#[test]
fn inconsistent_listing_is_detected() {
    let stray = FileEntry::new(path("ailleurs/x"), EntryKind::File, FileMetadata::unknown());
    let duplicate = FileEntry::new(path("a.txt"), EntryKind::File, FileMetadata::unknown());
    let connector = sample().extra_entry("", stray).extra_entry("", duplicate);
    let inventory = scan_all(&connector).unwrap();
    assert!(inventory.get(&path("ailleurs/x")).is_none());
    assert_eq!(
        inventory
            .directories()
            .find(|(p, _)| p.is_root())
            .map(|(_, s)| s),
        Some(DirectoryState::Incomplete(
            IncompleteReason::InconsistentListing
        ))
    );
    let kinds: Vec<_> = inventory
        .issues()
        .iter()
        .map(|i| i.kind().clone())
        .collect();
    assert!(kinds.contains(&ScanIssueKind::UnexpectedEntry(path("ailleurs/x"))));
    assert!(kinds.contains(&ScanIssueKind::DuplicateEntry));
}

#[test]
fn pre_cancelled_scan_returns_partial_inventory() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = scan(
        &sample(),
        &Filter::include_all(),
        &ScanOptions::new(),
        &cancel,
    );
    let Err(ScanError::Cancelled { partial }) = result else {
        panic!("attendu Cancelled");
    };
    assert!(partial.is_empty());
    assert!(!partial.is_complete());
    assert!(matches!(
        partial.presence(&path("a.txt")),
        Presence::Unknown(_)
    ));
}

#[test]
fn cancellation_mid_scan_keeps_what_was_seen() {
    let cancel = CancellationToken::new();
    // Annulation pendant le 2e listing : la racine est inventoriée, le
    // répertoire en cours et les suivants sont « non parcourus ».
    let connector = sample().cancel_on_list_call(2, cancel.clone());
    let result = scan(
        &connector,
        &Filter::include_all(),
        &ScanOptions::new(),
        &cancel,
    );
    let Err(ScanError::Cancelled { partial }) = result else {
        panic!("attendu Cancelled");
    };
    assert!(partial.get(&path("a.txt")).is_some());
    assert!(partial.get(&path("docs")).is_some());
    assert!(!partial.is_complete());
    assert_eq!(
        partial.presence(&path("docs/b.md")),
        Presence::Unknown(UnknownReason::IncompleteDirectory(
            IncompleteReason::NotScanned
        ))
    );
    assert_eq!(
        partial.presence(&path("vide/x")),
        Presence::Unknown(UnknownReason::IncompleteDirectory(
            IncompleteReason::NotScanned
        ))
    );
}

#[test]
fn computes_blake3_hashes_in_stream() {
    let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let connector = sample().file("gros.bin", &big, ts(5));
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let inventory = scan(
        &connector,
        &Filter::include_all(),
        &options,
        &CancellationToken::new(),
    )
    .unwrap();
    let expected = ContentHash::new(
        HashAlgorithm::Blake3,
        blake3::hash(&big).as_bytes().to_vec(),
    )
    .unwrap();
    assert_eq!(
        inventory.get(&path("gros.bin")).unwrap().metadata().hash(),
        Some(&expected)
    );
    assert_eq!(inventory.stats().hashed, 4);
    assert!(
        inventory
            .get(&path("docs"))
            .unwrap()
            .metadata()
            .hash()
            .is_none()
    );
    assert_eq!(inventory.hash_algorithm(), Some(HashAlgorithm::Blake3));
}

#[test]
fn hash_failures_are_reported_without_hash() {
    let connector = sample()
        .fail(
            "a.txt",
            StorageOperation::Read,
            StorageErrorKind::PermissionDenied,
        )
        // Déconnexion au milieu du transfert : lecture partielle.
        .fail_read_after("docs/sub/c.bin", 100, StorageErrorKind::Disconnected);
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let inventory = scan(
        &connector,
        &Filter::include_all(),
        &options,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(
        inventory
            .get(&path("a.txt"))
            .unwrap()
            .metadata()
            .hash()
            .is_none()
    );
    assert!(
        inventory
            .get(&path("docs/sub/c.bin"))
            .unwrap()
            .metadata()
            .hash()
            .is_none()
    );
    assert!(
        inventory
            .get(&path("docs/b.md"))
            .unwrap()
            .metadata()
            .hash()
            .is_some()
    );
    let kinds: Vec<_> = inventory
        .issues()
        .iter()
        .map(|i| (i.path().as_str(), i.kind().clone()))
        .collect();
    assert!(kinds.contains(&(
        "a.txt",
        ScanIssueKind::HashFailed(StorageErrorKind::PermissionDenied)
    )));
    assert!(kinds.contains(&(
        "docs/sub/c.bin",
        ScanIssueKind::HashFailed(StorageErrorKind::Disconnected)
    )));
    // Une empreinte manquante ne rend pas le listing incomplet.
    assert!(inventory.is_complete());
}

#[test]
fn size_mismatch_during_hash_discards_hash() {
    let stale = FileMetadata::unknown()
        .with_size(3)
        .with_modified(ts(1), sync_core::TimestampPrecision::Second);
    let connector = MemoryConnector::new("mem").file_with_metadata("f", b"plus long", stale);
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let inventory = scan(
        &connector,
        &Filter::include_all(),
        &options,
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(
        inventory
            .get(&path("f"))
            .unwrap()
            .metadata()
            .hash()
            .is_none()
    );
    assert_eq!(
        *inventory.issues()[0].kind(),
        ScanIssueKind::ChangedDuringHash
    );
}

#[test]
fn hashing_requires_read_capability_or_native_hash() {
    let caps = StorageCapabilities::none().with_features([StorageFeature::List]);
    let connector = sample().with_capabilities(caps);
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    assert!(matches!(
        scan(
            &connector,
            &Filter::include_all(),
            &options,
            &CancellationToken::new()
        ),
        Err(ScanError::HashingNotSupported {
            algorithm: HashAlgorithm::Blake3
        })
    ));
    let options = ScanOptions::new().with_hash(HashAlgorithm::Sha256);
    assert!(matches!(
        scan(
            &sample(),
            &Filter::include_all(),
            &options,
            &CancellationToken::new()
        ),
        Err(ScanError::HashingNotSupported {
            algorithm: HashAlgorithm::Sha256
        })
    ));
}

#[test]
fn native_hashes_are_reused_without_reading() {
    let native = ContentHash::new(HashAlgorithm::Sha256, vec![9; 32]).unwrap();
    let metadata = FileMetadata::unknown()
        .with_size(1)
        .with_hash(native.clone());
    let mut caps = StorageCapabilities::none().with_features([StorageFeature::List]);
    caps.native_hashes.insert(HashAlgorithm::Sha256);
    let connector = MemoryConnector::new("mem")
        .file_with_metadata("f", b"x", metadata)
        .with_capabilities(caps);
    let options = ScanOptions::new().with_hash(HashAlgorithm::Sha256);
    let inventory = scan(
        &connector,
        &Filter::include_all(),
        &options,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        inventory.get(&path("f")).unwrap().metadata().hash(),
        Some(&native)
    );
    assert_eq!(connector.read_opens(), 0);
}

#[test]
fn scanner_never_requests_mutations() {
    // Le stockage ne déclare que la lecture : le scan aboutit, ce qui montre
    // qu'aucune capacité d'écriture n'est nécessaire.
    let caps = StorageCapabilities::none().with_features([
        StorageFeature::List,
        StorageFeature::Stat,
        StorageFeature::Read,
    ]);
    assert!(caps.is_read_only());
    let connector = sample().with_capabilities(caps);
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    assert!(
        scan(
            &connector,
            &Filter::include_all(),
            &options,
            &CancellationToken::new()
        )
        .is_ok()
    );
}
