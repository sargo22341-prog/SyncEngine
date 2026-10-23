//! Tests de comparaison : inventaires produits par le scanner sur stockage mémoire.

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

use std::time::Duration;

use connector_api::{CaseSensitivity, StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, path, ts};
use sync_compare::{
    CompareError, CompareOptions, ComparisonOutcome, Difference, EquivalenceBasis, OutcomeCategory,
    Side, Uncertainty, compare, compare_entries,
};
use sync_core::{
    CancellationToken, EntryKind, FileEntry, FileMetadata, HashAlgorithm, Timestamp,
    TimestampPrecision,
};
use sync_filter::{Filter, FilterRules};
use sync_scanner::{IncompleteReason, Inventory, ScanOptions, UnknownReason, scan};

fn inventory_with(
    connector: &MemoryConnector,
    filter: &Filter,
    options: &ScanOptions,
) -> Inventory {
    scan(connector, filter, options, &CancellationToken::new()).unwrap()
}

fn inventory(connector: &MemoryConnector) -> Inventory {
    inventory_with(connector, &Filter::include_all(), &ScanOptions::new())
}

fn outcome_of(report: &sync_compare::ComparisonReport, at: &str) -> ComparisonOutcome {
    report.get(&path(at)).unwrap().outcome().clone()
}

fn file(
    at: &str,
    size: Option<u64>,
    modified: Option<(Timestamp, TimestampPrecision)>,
) -> FileEntry {
    let mut metadata = FileMetadata::unknown();
    if let Some(size) = size {
        metadata = metadata.with_size(size);
    }
    if let Some((ts, precision)) = modified {
        metadata = metadata.with_modified(ts, precision);
    }
    FileEntry::new(path(at), EntryKind::File, metadata)
}

#[test]
fn identical_trees_are_equivalent_by_metadata() {
    let make = || {
        MemoryConnector::new("m")
            .file("a.txt", b"alpha", ts(100))
            .file("d/b.txt", b"bravo", ts(200))
            .dir("vide")
    };
    let report = compare(
        &inventory(&make()),
        &inventory(&make()),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(report.rows().len(), 4);
    for row in report.rows() {
        assert!(
            matches!(row.outcome(), ComparisonOutcome::Equivalent(_)),
            "{}: {:?}",
            row.path(),
            row.outcome()
        );
    }
    assert_eq!(
        outcome_of(&report, "a.txt"),
        ComparisonOutcome::Equivalent(EquivalenceBasis::SizeAndModified)
    );
    assert_eq!(
        outcome_of(&report, "vide"),
        ComparisonOutcome::Equivalent(EquivalenceBasis::Directory)
    );
    let summary = report.summary();
    assert_eq!((summary.equivalent, summary.content_verified), (4, 0));
    assert!(report.left_complete() && report.right_complete());
}

#[test]
fn presence_on_one_side_with_complete_listing() {
    let left = MemoryConnector::new("l")
        .file("nouveau.txt", b"n", ts(1))
        .dir("d");
    let right = MemoryConnector::new("r")
        .file("ancien.txt", b"o", ts(1))
        .dir("d")
        .file("d/x", b"x", ts(1));
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "nouveau.txt"),
        ComparisonOutcome::LeftOnly
    );
    assert_eq!(
        outcome_of(&report, "ancien.txt"),
        ComparisonOutcome::RightOnly
    );
    assert_eq!(outcome_of(&report, "d/x"), ComparisonOutcome::RightOnly);
    assert_eq!(report.rows_in(OutcomeCategory::RightOnly).count(), 2);
    let row = report.get(&path("nouveau.txt")).unwrap();
    assert!(row.left().is_some() && row.right().is_none());
}

#[test]
fn empty_side_is_only_a_fact_when_scan_succeeded() {
    let left = MemoryConnector::new("l").file("a", b"a", ts(1));
    let empty = MemoryConnector::new("r");
    let report = compare(
        &inventory(&left),
        &inventory(&empty),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(outcome_of(&report, "a"), ComparisonOutcome::LeftOnly);
}

#[test]
fn inaccessible_source_directory_yields_uncertainty_not_absence() {
    // Given : la source (gauche) a un répertoire illisible, la destination
    //         (droite) en contient une copie.
    // When  : on compare.
    // Then  : aucun fichier de la destination n'est « droite seulement » :
    //         une stratégie miroir ne pourra en déduire aucune suppression.
    let source = MemoryConnector::new("src")
        .file("docs/a", b"a", ts(1))
        .file("docs/sub/b", b"b", ts(1))
        .fail(
            "docs",
            StorageOperation::List,
            StorageErrorKind::PermissionDenied,
        );
    let destination = MemoryConnector::new("dst")
        .file("docs/a", b"a", ts(1))
        .file("docs/sub/b", b"b", ts(1))
        .file("docs/orphelin", b"o", ts(1));
    let report = compare(
        &inventory(&source),
        &inventory(&destination),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(report.summary().right_only, 0);
    for at in ["docs/a", "docs/sub", "docs/sub/b", "docs/orphelin"] {
        assert_eq!(
            outcome_of(&report, at),
            ComparisonOutcome::Uncertain(vec![Uncertainty::PresenceUnknown {
                side: Side::Left,
                reason: UnknownReason::IncompleteDirectory(IncompleteReason::ListFailed(
                    StorageErrorKind::PermissionDenied
                )),
            }]),
            "{at}"
        );
    }
    assert_eq!(
        outcome_of(&report, "docs"),
        ComparisonOutcome::Equivalent(EquivalenceBasis::Directory)
    );
    assert!(!report.left_complete());
}

#[test]
fn disconnected_destination_subtree_yields_uncertainty() {
    let source = MemoryConnector::new("src").file("p/a", b"a", ts(1));
    let destination = MemoryConnector::new("dst").dir("p").fail(
        "p",
        StorageOperation::List,
        StorageErrorKind::Disconnected,
    );
    let report = compare(
        &inventory(&source),
        &inventory(&destination),
        &CompareOptions::new(),
    )
    .unwrap();
    assert!(outcome_of(&report, "p/a").is_uncertain());
    assert_eq!(report.summary().left_only, 0);
}

#[test]
fn size_and_date_differences() {
    let left = MemoryConnector::new("l")
        .file("taille", b"court", ts(10))
        .file("date", b"meme", ts(20))
        .file("les-deux", b"aaaa", ts(30));
    let right = MemoryConnector::new("r")
        .file("taille", b"plus long", ts(10))
        .file("date", b"meme", ts(25))
        .file("les-deux", b"bbbbbb", ts(5));
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "taille"),
        ComparisonOutcome::Different(vec![Difference::Size { left: 5, right: 9 }])
    );
    assert_eq!(
        outcome_of(&report, "date"),
        ComparisonOutcome::Different(vec![Difference::Modified { newer: Side::Right }])
    );
    assert_eq!(
        outcome_of(&report, "les-deux"),
        ComparisonOutcome::Different(vec![
            Difference::Size { left: 4, right: 6 },
            Difference::Modified { newer: Side::Left }
        ])
    );
    assert_eq!(report.summary().different, 3);
}

#[test]
fn kind_mismatch_and_descendants() {
    let left = MemoryConnector::new("l").file("a/x", b"x", ts(1));
    let right = MemoryConnector::new("r").file("a", b"fichier", ts(1));
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "a"),
        ComparisonOutcome::Different(vec![Difference::Kind {
            left: EntryKind::Directory,
            right: EntryKind::File
        }])
    );
    // Sous un fichier, l'absence est établie.
    assert_eq!(outcome_of(&report, "a/x"), ComparisonOutcome::LeftOnly);
}

#[test]
fn coarse_timestamp_precision_is_respected() {
    let precise = Timestamp::new(1_000, 300_000_000).unwrap();
    let fat = Timestamp::new(1_002, 0).unwrap();
    let left = file(
        "f",
        Some(10),
        Some((precise, TimestampPrecision::Nanosecond)),
    );
    let right = file("f", Some(10), Some((fat, TimestampPrecision::TwoSeconds)));
    // 1,7 s d'écart < 2 s de granularité FAT : même date.
    assert_eq!(
        compare_entries(&left, &right, &CompareOptions::new()),
        ComparisonOutcome::Equivalent(EquivalenceBasis::SizeAndModified)
    );
    // Avec la même précision fine des deux côtés, l'écart est une différence.
    let right_fine = file("f", Some(10), Some((fat, TimestampPrecision::Nanosecond)));
    assert_eq!(
        compare_entries(&left, &right_fine, &CompareOptions::new()),
        ComparisonOutcome::Different(vec![Difference::Modified { newer: Side::Right }])
    );
    // Une tolérance explicite l'absorbe.
    let tolerant = CompareOptions::new().with_modified_tolerance(Duration::from_secs(2));
    assert_eq!(
        compare_entries(&left, &right_fine, &tolerant),
        ComparisonOutcome::Equivalent(EquivalenceBasis::SizeAndModified)
    );
}

#[test]
fn unreliable_timestamps_make_comparison_uncertain() {
    let t = Timestamp::new(50, 0).unwrap();
    let known = file("f", Some(1), Some((t, TimestampPrecision::Second)));
    let unknown_precision = file("f", Some(1), Some((t, TimestampPrecision::Unknown)));
    let no_date = file("f", Some(1), None);
    let options = CompareOptions::new();
    assert_eq!(
        compare_entries(&known, &unknown_precision, &options),
        ComparisonOutcome::Uncertain(vec![Uncertainty::PrecisionUnknown])
    );
    assert_eq!(
        compare_entries(&known, &no_date, &options),
        ComparisonOutcome::Uncertain(vec![Uncertainty::ModifiedUnknown])
    );
    // Des tailles différentes suffisent, même sans date fiable.
    let bigger = file("f", Some(2), None);
    assert_eq!(
        compare_entries(&known, &bigger, &options),
        ComparisonOutcome::Different(vec![Difference::Size { left: 1, right: 2 }])
    );
    let no_size = file("f", None, Some((t, TimestampPrecision::Second)));
    assert_eq!(
        compare_entries(&known, &no_size, &options),
        ComparisonOutcome::Uncertain(vec![Uncertainty::SizeUnknown])
    );
}

#[test]
fn content_hash_decides_when_available() {
    let blake = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let all = Filter::include_all();
    // Même contenu, dates différentes : équivalent par contenu.
    let left = MemoryConnector::new("l")
        .file("same", b"contenu", ts(1))
        .file("diff", b"AAAA", ts(7));
    // Même taille, même date, contenu différent : seul le hash le voit.
    let right = MemoryConnector::new("r")
        .file("same", b"contenu", ts(999))
        .file("diff", b"BBBB", ts(7));
    let report = compare(
        &inventory_with(&left, &all, &blake),
        &inventory_with(&right, &all, &blake),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "same"),
        ComparisonOutcome::Equivalent(EquivalenceBasis::ContentHash)
    );
    assert_eq!(
        outcome_of(&report, "diff"),
        ComparisonOutcome::Different(vec![Difference::ContentHash])
    );
    assert_eq!(report.summary().content_verified, 1);

    // Sans empreinte, le même cas passe pour équivalent (fondement explicite).
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "diff"),
        ComparisonOutcome::Equivalent(EquivalenceBasis::SizeAndModified)
    );
    assert!(!EquivalenceBasis::SizeAndModified.is_content_verified());
}

#[test]
fn symlinks_are_not_compared() {
    let left = MemoryConnector::new("l").symlink("lien");
    let right = MemoryConnector::new("r").symlink("lien");
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "lien"),
        ComparisonOutcome::Uncertain(vec![Uncertainty::NotComparable {
            kind: EntryKind::Symlink
        }])
    );
}

#[test]
fn case_collisions_on_insensitive_side_are_uncertain() {
    let left = MemoryConnector::new("l")
        .file("README.md", b"r", ts(1))
        .file("Docs/a", b"a", ts(1));
    let right = MemoryConnector::new("r")
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("Readme.md", b"r", ts(1))
        .file("docs/a", b"a", ts(1));
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "README.md"),
        ComparisonOutcome::Uncertain(vec![Uncertainty::CaseCollision {
            with: vec![path("Readme.md")]
        }])
    );
    for at in ["Readme.md", "Docs", "docs", "Docs/a", "docs/a"] {
        assert!(outcome_of(&report, at).is_uncertain(), "{at}");
    }
    assert_eq!(report.summary().left_only + report.summary().right_only, 0);

    // Deux côtés sensibles à la casse : ce sont des fichiers distincts.
    let right_sensitive = MemoryConnector::new("r").file("Readme.md", b"r", ts(1));
    let left_only = MemoryConnector::new("l").file("README.md", b"r", ts(1));
    let report = compare(
        &inventory(&left_only),
        &inventory(&right_sensitive),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "README.md"),
        ComparisonOutcome::LeftOnly
    );
    assert_eq!(
        outcome_of(&report, "Readme.md"),
        ComparisonOutcome::RightOnly
    );
}

#[test]
fn unknown_case_rule_is_treated_as_insensitive() {
    let left = MemoryConnector::new("l")
        .file("a", b"1", ts(1))
        .file("A", b"2", ts(1));
    let right = MemoryConnector::new("r").with_case_sensitivity(CaseSensitivity::Unknown);
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert!(outcome_of(&report, "a").is_uncertain());
    assert!(outcome_of(&report, "A").is_uncertain());
}

#[test]
fn excluded_directory_colliding_with_included_file_is_uncertain() {
    let filter = Filter::new(FilterRules::new().exclude_directory("cache")).unwrap();
    let left = MemoryConnector::new("l").file("cache", b"un fichier", ts(1));
    let right = MemoryConnector::new("r").file("cache/x", b"x", ts(1));
    let report = compare(
        &inventory_with(&left, &filter, &ScanOptions::new()),
        &inventory_with(&right, &filter, &ScanOptions::new()),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(
        outcome_of(&report, "cache"),
        ComparisonOutcome::Uncertain(vec![Uncertainty::ExcludedOnSide { side: Side::Right }])
    );
    assert_eq!(report.rows().len(), 1);
}

#[test]
fn different_filters_are_refused() {
    let connector = MemoryConnector::new("m").file("a", b"a", ts(1));
    let a = inventory(&connector);
    let b = inventory_with(
        &connector,
        &Filter::new(FilterRules::new().exclude_file("*.tmp")).unwrap(),
        &ScanOptions::new(),
    );
    assert_eq!(
        compare(&a, &b, &CompareOptions::new()).unwrap_err(),
        CompareError::FilterMismatch
    );
}

#[test]
fn empty_directories_on_both_sides() {
    let left = MemoryConnector::new("l").dir("vide");
    let right = MemoryConnector::new("r").dir("vide");
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    assert_eq!(report.rows().len(), 1);
    assert_eq!(
        outcome_of(&report, "vide"),
        ComparisonOutcome::Equivalent(EquivalenceBasis::Directory)
    );
}

#[test]
fn rows_are_sorted_and_complete() {
    let left = MemoryConnector::new("l")
        .file("b", b"", ts(1))
        .file("a/c", b"", ts(1));
    let right = MemoryConnector::new("r").file("z", b"", ts(1));
    let report = compare(
        &inventory(&left),
        &inventory(&right),
        &CompareOptions::new(),
    )
    .unwrap();
    let paths: Vec<&str> = report.rows().iter().map(|r| r.path().as_str()).collect();
    assert_eq!(paths, ["a", "a/c", "b", "z"]);
}
