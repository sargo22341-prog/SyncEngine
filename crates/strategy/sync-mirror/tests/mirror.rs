//! Stratégie miroir : sémantique, garde-fous et instantanés JSON du plan.
//!
//! Instantanés : `tests/snapshots/*.json`. Après un changement **voulu** du
//! format ou des décisions, les régénérer avec
//! `SYNCENGINE_UPDATE_SNAPSHOTS=1 cargo test -p sync-mirror`, puis relire le
//! diff avant de valider.

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "aides de test : un échec est un échec de test"
)]

use std::path::PathBuf;

use connector_api::{StorageErrorKind, StorageOperation};
use connector_memory::{MemoryConnector, ts};
use sync_compare::{CompareOptions, ComparisonReport, compare};
use sync_core::{CancellationToken, PlanId, ResourcePath, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningError, PlanningInput, PolicyViolation};
use sync_filter::Filter;
use sync_mirror::MirrorStrategy;
use sync_plan::{
    Action, DeletionGuard, DeletionPolicy, DeletionRefusal, ExecutionPlan, OverwritePolicy,
    PlanPolicy, Reason, SyncMode,
};
use sync_scanner::{ScanError, ScanOptions, scan};

fn report(source: &MemoryConnector, destination: &MemoryConnector) -> ComparisonReport {
    let filter = Filter::include_all();
    let cancel = CancellationToken::new();
    let left = scan(source, &filter, &ScanOptions::new(), &cancel).unwrap();
    let right = scan(destination, &filter, &ScanOptions::new(), &cancel).unwrap();
    compare(&left, &right, &CompareOptions::new()).unwrap()
}

fn context() -> PlanContext {
    PlanContext {
        plan_id: PlanId::new("plan-miroir").unwrap(),
        profile: None,
        created_at: Timestamp::new(1_800_000_000, 0).unwrap(),
        source_location: "memoire:source".to_owned(),
        destination_location: "memoire:destination".to_owned(),
    }
}

fn build(report: &ComparisonReport, policy: &PlanPolicy) -> Result<ExecutionPlan, PlanningError> {
    let context = context();
    MirrorStrategy::new().build_plan(&PlanningInput::new(report, &context), policy)
}

fn with_deletions() -> PlanPolicy {
    PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new()),
    )
}

fn source() -> MemoryConnector {
    MemoryConnector::new("source")
        .file("commun.txt", b"identique", ts(1_000))
        .file("docs/rapport.md", b"version longue", ts(2_000))
        .file("recent.txt", b"source", ts(1_000))
        .file("nouveau/a.txt", b"a", ts(1_500))
}

fn destination() -> MemoryConnector {
    MemoryConnector::new("destination")
        .file("commun.txt", b"identique", ts(1_000))
        .file("docs/rapport.md", b"courte", ts(1_000))
        .file("docs/notes.txt", b"n", ts(3_000))
        .file("recent.txt", b"modifie a la destination", ts(5_000))
        .file("vieux/x.txt", b"x", ts(500))
}

fn decision(plan: &ExecutionPlan, at: &str) -> (Action, Reason) {
    let op = plan.get(&ResourcePath::parse(at).unwrap()).unwrap();
    (op.action(), op.reason().clone())
}

fn assert_snapshot(name: &str, actual: &str) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "snapshots", name]
        .iter()
        .collect();
    if std::env::var_os("SYNCENGINE_UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "instantané {} illisible ({error}) ; le créer avec SYNCENGINE_UPDATE_SNAPSHOTS=1",
            path.display()
        )
    });
    // Tolère une conversion CRLF à l'extraction Git sous Windows.
    assert_eq!(
        expected.replace("\r\n", "\n"),
        actual,
        "instantané {name} différent"
    );
}

#[test]
fn mirror_plan_identifies_every_action() {
    // Critère d'acceptation : copy / replace / keep / delete / conflict
    // identifiés (le conflit apparaît avec la politique prudente).
    let report = report(&source(), &destination());
    let plan = build(&report, &with_deletions()).unwrap();
    assert_eq!(plan.mode(), SyncMode::Mirror);
    assert_eq!(
        decision(&plan, "nouveau"),
        (Action::Copy, Reason::MissingAtDestination)
    );
    assert_eq!(
        decision(&plan, "nouveau/a.txt"),
        (Action::Copy, Reason::MissingAtDestination)
    );
    assert_eq!(
        decision(&plan, "docs/rapport.md"),
        (Action::Replace, Reason::SourceNewer)
    );
    assert_eq!(
        decision(&plan, "recent.txt"),
        (Action::Replace, Reason::DestinationNewer)
    );
    assert_eq!(
        decision(&plan, "commun.txt"),
        (Action::Keep, Reason::Equivalent { verified: false })
    );
    for at in ["docs/notes.txt", "vieux", "vieux/x.txt"] {
        assert_eq!(
            decision(&plan, at),
            (Action::Delete, Reason::AbsentFromSource),
            "{at}"
        );
    }
    let summary = plan.summary();
    assert_eq!(
        (
            summary.copy,
            summary.replace,
            summary.keep,
            summary.delete,
            summary.conflict,
            summary.skip
        ),
        (2, 2, 2, 3, 0, 0)
    );
    assert_snapshot("mirror_with_deletions.json", &plan.to_json().unwrap());

    let cautious = PlanPolicy::new(
        OverwritePolicy::NewerSourceOnly,
        DeletionPolicy::enabled(DeletionGuard::new()),
    );
    let plan = build(&report, &cautious).unwrap();
    assert_eq!(
        decision(&plan, "recent.txt"),
        (Action::Conflict, Reason::DestinationNewer)
    );
}

#[test]
fn deletions_require_an_explicit_policy() {
    let report = report(&source(), &destination());
    let plan = build(&report, &MirrorStrategy::default_policy()).unwrap();
    assert_eq!(plan.summary().delete, 0);
    for at in ["docs/notes.txt", "vieux", "vieux/x.txt"] {
        assert_eq!(
            decision(&plan, at),
            (Action::Keep, Reason::DeletionDisabled),
            "{at}"
        );
    }
}

#[test]
fn keep_destination_contradicts_a_mirror() {
    let report = report(&source(), &destination());
    let policy = PlanPolicy::new(OverwritePolicy::KeepDestination, DeletionPolicy::Disabled);
    assert_eq!(
        build(&report, &policy).unwrap_err(),
        PlanningError::InvalidPolicy(PolicyViolation::OverwriteNotSupported {
            mode: SyncMode::Mirror,
            overwrite: OverwritePolicy::KeepDestination
        })
    );
}

#[test]
fn partially_unreadable_source_produces_zero_deletion() {
    // Given : miroir avec suppressions autorisées ; le dossier source
    //         « docs » est illisible (accès refusé).
    // When  : le plan est construit.
    // Then  : zéro suppression ; « docs/notes.txt » est incertain, les
    //         autres suppressions sont refusées (source incomplète).
    let source = source().fail(
        "docs",
        StorageOperation::List,
        StorageErrorKind::PermissionDenied,
    );
    let report = report(&source, &destination());
    let plan = build(&report, &with_deletions()).unwrap();
    assert_eq!(plan.summary().delete, 0);
    assert_eq!(decision(&plan, "docs/notes.txt").0, Action::Skip);
    assert_eq!(
        decision(&plan, "vieux/x.txt"),
        (
            Action::Skip,
            Reason::DeletionRefused {
                refusal: DeletionRefusal::SourceIncomplete
            }
        )
    );
    assert!(plan.needs_attention());
    assert_snapshot("mirror_source_incomplete.json", &plan.to_json().unwrap());
}

#[test]
fn missing_source_yields_no_inventory_hence_no_plan() {
    // Given : la racine source est déconnectée (montage absent).
    // Then  : le scan échoue ; aucun rapport ni plan ne peut exister, donc
    //         aucune suppression. Une racine absente n'est jamais « vide ».
    let source = MemoryConnector::new("source").fail(
        "",
        StorageOperation::List,
        StorageErrorKind::Disconnected,
    );
    let result = scan(
        &source,
        &Filter::include_all(),
        &ScanOptions::new(),
        &CancellationToken::new(),
    );
    assert!(matches!(result, Err(ScanError::RootUnavailable(_))));
}

#[test]
fn empty_source_never_empties_the_destination() {
    let report = report(&MemoryConnector::new("source"), &destination());
    let unlimited = PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new().with_max_percent(None).unwrap()),
    );
    let plan = build(&report, &unlimited).unwrap();
    assert_eq!(plan.summary().delete, 0);
    assert_eq!(
        decision(&plan, "commun.txt").1,
        Reason::DeletionRefused {
            refusal: DeletionRefusal::SourceEmpty
        }
    );
}

#[test]
fn links_at_destination_are_never_deleted() {
    let source = MemoryConnector::new("source").file("a", b"a", ts(1));
    let destination = MemoryConnector::new("destination")
        .file("a", b"a", ts(1))
        .symlink("lien");
    let plan = build(&report(&source, &destination), &with_deletions()).unwrap();
    assert_eq!(
        decision(&plan, "lien"),
        (
            Action::Skip,
            Reason::UnsupportedEntryKind {
                kind: sync_core::EntryKind::Symlink
            }
        )
    );
}

#[test]
fn snapshot_reloads_as_the_same_plan() {
    let report = report(&source(), &destination());
    let plan = build(&report, &with_deletions()).unwrap();
    let reloaded = ExecutionPlan::from_json(&plan.to_json().unwrap()).unwrap();
    assert_eq!(reloaded, plan);
}
