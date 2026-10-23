//! Stratégie de sauvegarde : sémantique et instantané JSON du plan.
//!
//! Instantanés : `tests/snapshots/*.json`, régénérés par
//! `SYNCENGINE_UPDATE_SNAPSHOTS=1 cargo test -p sync-backup` après un
//! changement voulu (relire le diff).

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "aides de test : un échec est un échec de test"
)]

use std::path::PathBuf;

use connector_memory::{MemoryConnector, ts};
use sync_backup::BackupStrategy;
use sync_compare::{CompareOptions, ComparisonReport, compare};
use sync_core::{CancellationToken, PlanId, ResourcePath, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningError, PlanningInput, PolicyViolation};
use sync_filter::{Filter, FilterRules};
use sync_plan::{
    Action, DeletionGuard, DeletionPolicy, ExecutionPlan, OverwritePolicy, PlanPolicy, Reason,
    SyncMode,
};
use sync_scanner::{ScanOptions, scan};

fn report_filtered(
    source: &MemoryConnector,
    destination: &MemoryConnector,
    rules: FilterRules,
) -> ComparisonReport {
    let filter = Filter::new(rules).unwrap();
    let cancel = CancellationToken::new();
    let left = scan(source, &filter, &ScanOptions::new(), &cancel).unwrap();
    let right = scan(destination, &filter, &ScanOptions::new(), &cancel).unwrap();
    compare(&left, &right, &CompareOptions::new()).unwrap()
}

fn report(source: &MemoryConnector, destination: &MemoryConnector) -> ComparisonReport {
    report_filtered(source, destination, FilterRules::new())
}

fn context() -> PlanContext {
    PlanContext {
        plan_id: PlanId::new("plan-sauvegarde").unwrap(),
        profile: Some(sync_core::ProfileId::new("quotidien").unwrap()),
        created_at: Timestamp::new(1_800_000_000, 0).unwrap(),
        source_location: "memoire:source".to_owned(),
        destination_location: "memoire:destination".to_owned(),
    }
}

fn build(report: &ComparisonReport, policy: &PlanPolicy) -> Result<ExecutionPlan, PlanningError> {
    let context = context();
    BackupStrategy::new().build_plan(&PlanningInput::new(report, &context), policy)
}

fn source() -> MemoryConnector {
    MemoryConnector::new("source")
        .file("commun.txt", b"identique", ts(1_000))
        .file("docs/rapport.md", b"version longue", ts(2_000))
        .file("recent.txt", b"source", ts(1_000))
        .file("nouveau/a.txt", b"a", ts(1_500))
        .file("cache/tmp.tmp", b"t", ts(1))
}

fn destination() -> MemoryConnector {
    MemoryConnector::new("destination")
        .file("commun.txt", b"identique", ts(1_000))
        .file("docs/rapport.md", b"courte", ts(1_000))
        .file("docs/notes.txt", b"n", ts(3_000))
        .file("recent.txt", b"modifie a la destination", ts(5_000))
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
    assert_eq!(
        expected.replace("\r\n", "\n"),
        actual,
        "instantané {name} différent"
    );
}

#[test]
fn backup_plan_copies_replaces_keeps_and_reports_conflicts() {
    let rules = FilterRules::new().exclude_file("*.tmp");
    let report = report_filtered(&source(), &destination(), rules.clone());
    let plan = build(&report, &BackupStrategy::default_policy()).unwrap();
    assert_eq!(plan.mode(), SyncMode::Backup);
    assert_eq!(
        plan.filter_rules(),
        &rules,
        "filtre enregistré dans le plan"
    );
    assert_eq!(plan.profile().map(|p| p.as_str()), Some("quotidien"));
    assert_eq!(
        decision(&plan, "nouveau/a.txt"),
        (Action::Copy, Reason::MissingAtDestination)
    );
    assert_eq!(
        decision(&plan, "docs/rapport.md"),
        (Action::Replace, Reason::SourceNewer)
    );
    // La destination a été modifiée après la source : conflit, rien n'est écrit.
    assert_eq!(
        decision(&plan, "recent.txt"),
        (Action::Conflict, Reason::DestinationNewer)
    );
    // Présent seulement à la destination : conservé, jamais supprimé.
    assert_eq!(
        decision(&plan, "docs/notes.txt"),
        (Action::Keep, Reason::DestinationOnly)
    );
    assert!(
        plan.get(&ResourcePath::parse("cache/tmp.tmp").unwrap())
            .is_none()
    );
    assert_eq!(plan.summary().delete, 0);
    assert!(plan.needs_attention(), "un conflit demande l'attention");
    assert_snapshot("backup_default.json", &plan.to_json().unwrap());
}

#[test]
fn backup_refuses_a_deletion_policy() {
    let report = report(&source(), &destination());
    let policy = PlanPolicy::new(
        OverwritePolicy::NewerSourceOnly,
        DeletionPolicy::enabled(DeletionGuard::new()),
    );
    assert_eq!(
        build(&report, &policy).unwrap_err(),
        PlanningError::InvalidPolicy(PolicyViolation::DeletionNotSupported {
            mode: SyncMode::Backup
        })
    );
}

#[test]
fn backup_with_source_wins_overwrites_newer_destination() {
    let report = report(&source(), &destination());
    let policy = PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled);
    let plan = build(&report, &policy).unwrap();
    assert_eq!(
        decision(&plan, "recent.txt"),
        (Action::Replace, Reason::DestinationNewer)
    );
    let policy = PlanPolicy::new(OverwritePolicy::KeepDestination, DeletionPolicy::Disabled);
    let plan = build(&report, &policy).unwrap();
    assert_eq!(
        decision(&plan, "docs/rapport.md"),
        (Action::Keep, Reason::OverwriteDisabled)
    );
}

#[test]
fn empty_source_against_full_destination_changes_nothing() {
    let report = report(&MemoryConnector::new("source"), &destination());
    let plan = build(&report, &BackupStrategy::default_policy()).unwrap();
    assert_eq!(plan.summary().mutations(), 0);
    assert_eq!(plan.summary().keep, plan.operations().len() as u64);
}
