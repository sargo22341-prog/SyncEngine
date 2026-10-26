//! Règle commune et garde-fous, sur des inventaires en mémoire.
//!
//! Le mode de test `Deleting` supprime les entrées présentes seulement à la
//! destination quand la politique l'autorise : il isole la règle commune des
//! stratégies réelles (testées dans `sync-backup` et `sync-mirror`).

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "aides de test : un échec est un échec de test"
)]

use connector_api::{CaseSensitivity, StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, ts};
use sync_compare::{CompareOptions, ComparisonReport, compare};
use sync_core::{CancellationToken, EntryKind, HashAlgorithm, PlanId, ResourcePath, Timestamp};
use sync_decisions::{
    Decision, OneWayMode, PlanContext, PlanningError, PlanningInput, PolicyViolation, plan_one_way,
};
use sync_filter::Filter;
use sync_plan::{
    Action, DeletionGuard, DeletionPolicy, DeletionRefusal, EntrySnapshot, ExecutionPlan,
    OverwritePolicy, PlanError, PlanPolicy, PlanSide, Reason, SyncMode, UncertainCause,
};
use sync_scanner::{ScanOptions, scan};

struct Deleting;

impl OneWayMode for Deleting {
    fn mode(&self) -> SyncMode {
        SyncMode::Mirror
    }

    fn check_policy(&self, _policy: &PlanPolicy) -> Result<(), PolicyViolation> {
        Ok(())
    }

    fn destination_only(&self, destination: &EntrySnapshot, policy: &PlanPolicy) -> Decision {
        match (destination.kind(), policy.deletion.guard()) {
            (EntryKind::File | EntryKind::Directory, Some(_)) => {
                Decision::new(Action::Delete, Reason::AbsentFromSource)
            }
            (EntryKind::File | EntryKind::Directory, None) => {
                Decision::new(Action::Keep, Reason::DeletionDisabled)
            }
            (kind, _) => Decision::new(Action::Skip, Reason::UnsupportedEntryKind { kind }),
        }
    }
}

/// Mode défaillant : supprime même sans politique (le plan doit le refuser).
struct Reckless;

impl OneWayMode for Reckless {
    fn mode(&self) -> SyncMode {
        SyncMode::Mirror
    }

    fn check_policy(&self, _policy: &PlanPolicy) -> Result<(), PolicyViolation> {
        Ok(())
    }

    fn destination_only(&self, _destination: &EntrySnapshot, _policy: &PlanPolicy) -> Decision {
        Decision::new(Action::Delete, Reason::AbsentFromSource)
    }
}

fn report_with(
    source: &MemoryConnector,
    destination: &MemoryConnector,
    hash: bool,
) -> ComparisonReport {
    let options = if hash {
        ScanOptions::new().with_hash(HashAlgorithm::Blake3)
    } else {
        ScanOptions::new()
    };
    let filter = Filter::include_all();
    let cancel = CancellationToken::new();
    let left = scan(source, &filter, &options, &cancel).unwrap();
    let right = scan(destination, &filter, &options, &cancel).unwrap();
    compare(&left, &right, &CompareOptions::new()).unwrap()
}

fn report(source: &MemoryConnector, destination: &MemoryConnector) -> ComparisonReport {
    report_with(source, destination, false)
}

fn context() -> PlanContext {
    PlanContext {
        plan_id: PlanId::new("plan-1").unwrap(),
        profile: None,
        created_at: Timestamp::new(1_800_000_000, 0).unwrap(),
        source_location: "source".to_owned(),
        destination_location: "destination".to_owned(),
    }
}

fn deleting_policy(guard: DeletionGuard) -> PlanPolicy {
    PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::enabled(guard))
}

fn unguarded() -> PlanPolicy {
    deleting_policy(DeletionGuard::new().with_max_percent(None).unwrap())
}

fn plan(report: &ComparisonReport, policy: &PlanPolicy) -> ExecutionPlan {
    let context = context();
    plan_one_way(&Deleting, &PlanningInput::new(report, &context), policy).unwrap()
}

fn decision(plan: &ExecutionPlan, at: &str) -> (Action, Reason) {
    let op = plan
        .get(&ResourcePath::parse(at).unwrap())
        .unwrap_or_else(|| panic!("aucune opération pour {at}"));
    (op.action(), op.reason().clone())
}

#[test]
fn source_only_entries_are_copied_with_their_directories() {
    let source = MemoryConnector::new("src")
        .file("docs/a.txt", b"abc", ts(10))
        .symlink("lien");
    let destination = MemoryConnector::new("dst");
    let plan = plan(&report(&source, &destination), &unguarded());
    assert_eq!(
        decision(&plan, "docs"),
        (Action::Copy, Reason::MissingAtDestination)
    );
    let copy = plan
        .get(&ResourcePath::parse("docs/a.txt").unwrap())
        .unwrap();
    assert_eq!(copy.action(), Action::Copy);
    assert_eq!(copy.bytes(), Some(3));
    assert_eq!(copy.depends_on().len(), 1);
    assert_eq!(
        decision(&plan, "lien"),
        (
            Action::Skip,
            Reason::UnsupportedEntryKind {
                kind: EntryKind::Symlink
            }
        )
    );
    assert_eq!(plan.source().connector().as_str(), "src");
    assert_eq!(plan.destination().location(), "destination");
}

#[test]
fn unreadable_source_directory_never_leads_to_deletion() {
    // Given : un sous-dossier source illisible, la destination contient des
    //         fichiers dans ce dossier et ailleurs, suppressions autorisées.
    // When  : le plan est construit.
    // Then  : les fichiers sous le dossier illisible sont sautés (présence
    //         inconnue à la source) et AUCUNE suppression n'est retenue :
    //         l'inventaire source est incomplet.
    let source = MemoryConnector::new("src")
        .file("garde.txt", b"g", ts(1))
        .file("prive/s.txt", b"s", ts(1))
        .fail(
            "prive",
            StorageOperation::List,
            StorageErrorKind::PermissionDenied,
        );
    let destination = MemoryConnector::new("dst")
        .file("garde.txt", b"g", ts(1))
        .file("prive/orphelin.txt", b"o", ts(1))
        .file("ancien.txt", b"a", ts(1));
    let plan = plan(&report(&source, &destination), &unguarded());

    assert_eq!(
        decision(&plan, "prive/orphelin.txt"),
        (
            Action::Skip,
            Reason::Uncertain {
                causes: vec![UncertainCause::PresenceUnknown {
                    side: PlanSide::Source
                }]
            }
        )
    );
    assert_eq!(
        decision(&plan, "ancien.txt"),
        (
            Action::Skip,
            Reason::DeletionRefused {
                refusal: DeletionRefusal::SourceIncomplete
            }
        )
    );
    assert_eq!(plan.summary().delete, 0);
    assert!(!plan.source().is_complete());
    assert!(plan.needs_attention());
}

#[test]
fn incomplete_destination_refuses_deletions() {
    let source = MemoryConnector::new("src").file("a", b"a", ts(1));
    let destination = MemoryConnector::new("dst")
        .file("a", b"a", ts(1))
        .file("b", b"b", ts(1))
        .file("sous/c", b"c", ts(1))
        .fail(
            "sous",
            StorageOperation::List,
            StorageErrorKind::Disconnected,
        );
    let plan = plan(&report(&source, &destination), &unguarded());
    assert_eq!(
        decision(&plan, "b"),
        (
            Action::Skip,
            Reason::DeletionRefused {
                refusal: DeletionRefusal::DestinationIncomplete
            }
        )
    );
    assert_eq!(plan.summary().delete, 0);
}

#[test]
fn empty_source_refuses_deletions_unless_allowed() {
    // Given : une source vide (montage absent ?) et une destination remplie.
    let source = MemoryConnector::new("src");
    let destination = MemoryConnector::new("dst")
        .file("a", b"a", ts(1))
        .file("b", b"b", ts(1));
    let report = report(&source, &destination);
    let plan = plan(&report, &unguarded());
    assert_eq!(plan.summary().delete, 0);
    assert_eq!(plan.summary().deletions_refused, 2);
    assert_eq!(
        decision(&plan, "a").1,
        Reason::DeletionRefused {
            refusal: DeletionRefusal::SourceEmpty
        }
    );

    let allowed = deleting_policy(
        DeletionGuard::new()
            .with_max_percent(None)
            .unwrap()
            .with_empty_source_allowed(true),
    );
    let plan = plan_one_way(
        &Deleting,
        &PlanningInput::new(&report, &context()),
        &allowed,
    )
    .unwrap();
    assert_eq!(plan.summary().delete, 2);
}

#[test]
fn mass_deletion_guard_is_all_or_nothing() {
    let source = MemoryConnector::new("src").file("garde", b"g", ts(1));
    let destination = (1..=12).fold(
        MemoryConnector::new("dst").file("garde", b"g", ts(1)),
        |connector, n| connector.file(&format!("x{n:02}"), b"x", ts(1)),
    );
    let report = report(&source, &destination);
    // 12 suppressions sur 13 entrées = 92 % > 50 % par défaut.
    let plan = plan(&report, &deleting_policy(DeletionGuard::new()));
    assert_eq!(plan.summary().delete, 0);
    assert_eq!(plan.summary().deletions_refused, 12);
    assert_eq!(
        decision(&plan, "x01").1,
        Reason::DeletionRefused {
            refusal: DeletionRefusal::TooLarge {
                count: 12,
                total: 13,
                max_percent: 50
            }
        }
    );
    let generous = deleting_policy(DeletionGuard::new().with_max_percent(Some(95)).unwrap());
    let plan = plan_one_way(
        &Deleting,
        &PlanningInput::new(&report, &context()),
        &generous,
    )
    .unwrap();
    assert_eq!(plan.summary().delete, 12);
    let capped = deleting_policy(
        DeletionGuard::new()
            .with_max_percent(None)
            .unwrap()
            .with_max_count(Some(1)),
    );
    let plan = plan_one_way(&Deleting, &PlanningInput::new(&report, &context()), &capped).unwrap();
    assert_eq!(plan.summary().delete, 0);
    assert_eq!(plan.summary().deletions_refused, 12);
}

/// Given une source pleine et une destination ne contenant qu'un fichier
/// absent de la source, when le miroir est planifié avec les garde-fous par
/// défaut, then ce fichier est supprimé : 1 suppression sur 1 entrée ne
/// relève pas du plafond relatif (seuil de volume).
#[test]
fn single_obsolete_destination_file_is_deleted_by_default() {
    let source = (1..=30).fold(MemoryConnector::new("src"), |connector, n| {
        connector.file(&format!("photo{n:02}.jpg"), b"p", ts(1))
    });
    let destination = MemoryConnector::new("dst").file("ancien.zip", b"z", ts(1));
    let report = report(&source, &destination);
    let plan = plan(&report, &deleting_policy(DeletionGuard::new()));
    assert_eq!(plan.summary().delete, 1);
    assert_eq!(plan.summary().copy, 30);
    assert_eq!(plan.summary().deletions_refused, 0);
}

#[test]
fn directory_deletion_waits_for_its_content() {
    let source = MemoryConnector::new("src").file("garde", b"g", ts(1));
    let destination = MemoryConnector::new("dst")
        .file("garde", b"g", ts(1))
        .file("vieux/a", b"a", ts(1))
        .file("vieux/sous/b", b"b", ts(1))
        .symlink("autre/lien");
    let plan = plan(&report(&source, &destination), &unguarded());
    assert_eq!(decision(&plan, "vieux").0, Action::Delete);
    assert_eq!(decision(&plan, "vieux/sous").0, Action::Delete);
    let vieux = plan.get(&ResourcePath::parse("vieux").unwrap()).unwrap();
    assert_eq!(vieux.depends_on().len(), 2, "vieux/a et vieux/sous");
    // Un lien n'est jamais supprimé : son dossier ne peut pas l'être.
    assert_eq!(
        decision(&plan, "autre/lien").1,
        Reason::UnsupportedEntryKind {
            kind: EntryKind::Symlink
        }
    );
    assert_eq!(
        decision(&plan, "autre"),
        (Action::Skip, Reason::DirectoryNotEmptied)
    );
}

#[test]
fn kind_mismatch_is_a_conflict_and_blocks_descendants() {
    let source = MemoryConnector::new("src").file("x/y.txt", b"y", ts(1));
    let destination = MemoryConnector::new("dst").file("x", b"fichier", ts(1));
    let plan = plan(&report(&source, &destination), &unguarded());
    assert_eq!(
        decision(&plan, "x"),
        (
            Action::Conflict,
            Reason::KindMismatch {
                source: EntryKind::Directory,
                destination: EntryKind::File
            }
        )
    );
    assert_eq!(
        decision(&plan, "x/y.txt"),
        (Action::Skip, Reason::ParentUnavailable)
    );
}

#[test]
fn overwrite_follows_policy_and_dates() {
    let source = MemoryConnector::new("src")
        .file("plus-recent-source", b"nouveau", ts(200))
        .file("plus-recent-dest", b"ancien", ts(100))
        .file("meme-date", b"aaa", ts(100));
    let destination = MemoryConnector::new("dst")
        .file("plus-recent-source", b"ancien", ts(100))
        .file("plus-recent-dest", b"modifie", ts(300))
        .file("meme-date", b"bbbb", ts(100));
    let report = report(&source, &destination);
    let context = context();
    let input = PlanningInput::new(&report, &context);

    let cautious = PlanPolicy::new(OverwritePolicy::NewerSourceOnly, DeletionPolicy::Disabled);
    let plan = plan_one_way(&Deleting, &input, &cautious).unwrap();
    assert_eq!(
        decision(&plan, "plus-recent-source"),
        (Action::Replace, Reason::SourceNewer)
    );
    assert_eq!(
        decision(&plan, "plus-recent-dest"),
        (Action::Conflict, Reason::DestinationNewer)
    );
    assert_eq!(
        decision(&plan, "meme-date"),
        (Action::Conflict, Reason::AmbiguousChange)
    );

    let source_wins = PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled);
    let plan = plan_one_way(&Deleting, &input, &source_wins).unwrap();
    assert_eq!(
        decision(&plan, "plus-recent-dest"),
        (Action::Replace, Reason::DestinationNewer)
    );
    assert_eq!(
        decision(&plan, "meme-date"),
        (Action::Replace, Reason::ContentDiffers)
    );

    let keep = PlanPolicy::new(OverwritePolicy::KeepDestination, DeletionPolicy::Disabled);
    let plan = plan_one_way(&Deleting, &input, &keep).unwrap();
    assert_eq!(plan.summary().replace, 0);
    assert_eq!(plan.summary().keep, 3);
}

#[test]
fn equivalence_basis_is_kept_in_the_plan() {
    let source = MemoryConnector::new("src").file("a", b"meme", ts(5));
    let destination = MemoryConnector::new("dst").file("a", b"meme", ts(5));
    let plan_meta = plan(&report(&source, &destination), &unguarded());
    assert_eq!(
        decision(&plan_meta, "a"),
        (Action::Keep, Reason::Equivalent { verified: false })
    );
    let hashed = report_with(&source, &destination, true);
    let plan_hash = plan(&hashed, &unguarded());
    assert_eq!(
        decision(&plan_hash, "a"),
        (Action::Keep, Reason::Equivalent { verified: true })
    );
}

#[test]
fn case_collisions_are_skipped() {
    let source = MemoryConnector::new("src").file("Rapport.txt", b"s", ts(1));
    let destination = MemoryConnector::new("dst")
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("rapport.txt", b"d", ts(1));
    let plan = plan(&report(&source, &destination), &unguarded());
    for at in ["Rapport.txt", "rapport.txt"] {
        assert_eq!(
            decision(&plan, at),
            (
                Action::Skip,
                Reason::Uncertain {
                    causes: vec![UncertainCause::CaseCollision]
                }
            ),
            "{at}"
        );
    }
    assert_eq!(plan.summary().mutations(), 0);
}

#[test]
fn unknown_dates_never_overwrite() {
    let unknown = sync_core::FileMetadata::unknown().with_size(4);
    let source = MemoryConnector::new("src").file_with_metadata("a", b"aaaa", unknown.clone());
    let destination = MemoryConnector::new("dst").file_with_metadata("a", b"bbbb", unknown);
    let plan = plan(&report(&source, &destination), &unguarded());
    assert_eq!(
        decision(&plan, "a"),
        (
            Action::Skip,
            Reason::Uncertain {
                causes: vec![UncertainCause::ModifiedUnknown]
            }
        )
    );
}

#[test]
fn mirror_never_deletes_below_a_case_ambiguous_source_directory() {
    // Given: two source directories collapse onto the Windows destination.
    let source = MemoryConnector::new("src")
        .with_case_sensitivity(CaseSensitivity::Unknown)
        .dir("People/B")
        .dir("People/b");
    let destination = MemoryConnector::new("dst")
        .with_case_sensitivity(CaseSensitivity::InsensitivePreserving)
        .file("People/B/bbno$/folder.jpg", b"must survive", ts(1))
        .file("unrelated-obsolete", b"delete allowed", ts(1));
    // When: the mirror allows deletion and the scans are complete.
    let plan = plan(&report(&source, &destination), &unguarded());
    // Then: all ambiguous descendants are skipped, never deleted.
    for at in ["People/B/bbno$", "People/B/bbno$/folder.jpg"] {
        assert_eq!(
            decision(&plan, at),
            (
                Action::Skip,
                Reason::Uncertain {
                    causes: vec![UncertainCause::CaseCollision],
                }
            )
        );
    }
    assert_eq!(
        decision(&plan, "unrelated-obsolete"),
        (Action::Delete, Reason::AbsentFromSource)
    );
}

#[test]
fn planning_is_deterministic() {
    let source = MemoryConnector::new("src")
        .file("a/b", b"1", ts(2))
        .file("c", b"22", ts(3));
    let destination = MemoryConnector::new("dst")
        .file("c", b"2", ts(1))
        .file("d", b"4", ts(1));
    let first = plan(&report(&source, &destination), &unguarded());
    let second = plan(&report(&source, &destination), &unguarded());
    assert_eq!(first, second);
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
}

#[test]
fn a_reckless_mode_cannot_produce_a_deleting_plan() {
    // Given : un mode défectueux qui supprime malgré une politique
    //         n'autorisant pas les suppressions.
    // Then  : aucun plan n'est rendu ; l'erreur est explicite.
    let source = MemoryConnector::new("src").file("a", b"a", ts(1));
    let destination = MemoryConnector::new("dst")
        .file("a", b"a", ts(1))
        .file("b", b"b", ts(1));
    let report = report(&source, &destination);
    let policy = PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled);
    let error =
        plan_one_way(&Reckless, &PlanningInput::new(&report, &context()), &policy).unwrap_err();
    assert!(
        matches!(
            error,
            PlanningError::InconsistentPlan(PlanError::DeletionNotAllowed { .. })
        ),
        "{error}"
    );
}

#[test]
fn interleaved_sibling_names_do_not_hide_kept_content() {
    // Given : « vieux.bak » et « vieux-1 » se classent ENTRE « vieux » et
    //         « vieux/… » (« . » et « - » précèdent « / ») ; « vieux/lien »
    //         est un lien (jamais supprimé).
    // Then  : « vieux » n'est pas supprimé, et sa dépendance vers
    //         « vieux/a » est retrouvée malgré l'intercalation.
    let source = MemoryConnector::new("src")
        .file("garde", b"g", ts(1))
        .file("vieux.bak", b"b", ts(1))
        .file("vieux-1", b"1", ts(1));
    let destination = MemoryConnector::new("dst")
        .file("garde", b"g", ts(1))
        .file("vieux.bak", b"b", ts(1))
        .file("vieux-1", b"1", ts(1))
        .file("vieux/a", b"a", ts(1))
        .symlink("vieux/lien")
        .file("autre/x", b"x", ts(1));
    let plan = plan(&report(&source, &destination), &unguarded());
    assert_eq!(
        decision(&plan, "vieux"),
        (Action::Skip, Reason::DirectoryNotEmptied)
    );
    assert_eq!(decision(&plan, "vieux/a").0, Action::Delete);
    assert_eq!(decision(&plan, "autre").0, Action::Delete);
    let autre = plan.get(&ResourcePath::parse("autre").unwrap()).unwrap();
    assert_eq!(
        autre.depends_on(),
        [plan
            .get(&ResourcePath::parse("autre/x").unwrap())
            .unwrap()
            .id()
            .clone()]
    );
}
