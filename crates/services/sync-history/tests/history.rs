//! Phase 05 contract and safety tests.
#![expect(
    clippy::unwrap_used,
    reason = "isolated test fixtures fail immediately"
)]

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use sync_core::{
    ConnectorId, ExecutionId, ExecutionStatus, PlanId, ProfileId, ResourcePath, Timestamp,
};
use sync_history::{History, HistoryError, RetentionPolicy, RunState};
use sync_plan::{
    Baseline, DeletionPolicy, EndpointSnapshot, ExecutionPlan, OverwritePolicy, PlanHeader,
    PlanPolicy, StateBinding, SyncMode,
};

fn binding() -> StateBinding {
    StateBinding {
        profile: ProfileId::new("profile").unwrap(),
        source_connector: ConnectorId::new("l").unwrap(),
        destination_connector: ConnectorId::new("r").unwrap(),
        source: "left-root".into(),
        destination: "right-root".into(),
        filter: Default::default(),
    }
}
fn plan(base: Option<&Baseline>) -> ExecutionPlan {
    let b = binding();
    ExecutionPlan::new_bidirectional(
        PlanHeader {
            id: PlanId::new("plan").unwrap(),
            profile: Some(b.profile),
            mode: SyncMode::Bidirectional,
            created_at: Timestamp::UNIX_EPOCH,
            source: EndpointSnapshot::new(b.source_connector, b.source, true),
            destination: EndpointSnapshot::new(b.destination_connector, b.destination, true),
            filter: b.filter,
            policy: PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled),
        },
        vec![],
        base.cloned(),
    )
    .unwrap()
}
fn next(generation: u64) -> Baseline {
    Baseline::new(
        binding(),
        generation,
        BTreeMap::from([(ResourcePath::parse("gone").unwrap(), None)]),
    )
    .unwrap()
}
fn id(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}
fn publish(h: &mut History, run: &str, old: Option<&Baseline>, new: &Baseline) {
    let token = h.begin(id(run), &binding(), &plan(old)).unwrap();
    h.finish(
        token,
        ExecutionStatus::Succeeded,
        serde_json::json!({"ok":true}),
        Some(new),
        RetentionPolicy::default(),
    )
    .unwrap();
}

#[test]
fn migration_from_empty_database_reopen_and_backup_preserve_tombstones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut h = History::open(&path).unwrap();
    assert!(h.baseline(&binding()).unwrap().is_none());
    let b = next(1);
    publish(&mut h, "one", None, &b);
    let backup = dir.path().join("backup.sqlite");
    h.backup_to(&backup).unwrap();
    assert!(h.backup_to(&backup).is_err());
    drop(h);
    for file in [&path, &backup] {
        let h = History::open(file).unwrap();
        assert_eq!(h.baseline(&binding()).unwrap(), Some(b.clone()));
        let records = h.records(&binding().profile).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].state,
            RunState::Finished(ExecutionStatus::Succeeded)
        );
    }
}

#[test]
fn dropped_owner_blocks_new_run_until_explicit_interruption_diagnosis() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut h = History::open(&path).unwrap();
    let token = h.begin(id("crash"), &binding(), &plan(None)).unwrap();
    drop(token);
    drop(h);
    let mut h = History::open(&path).unwrap();
    assert!(matches!(
        h.begin(id("new"), &binding(), &plan(None)),
        Err(HistoryError::Running)
    ));
    assert!(h.baseline(&binding()).unwrap().is_none());
    h.mark_interrupted(&id("crash")).unwrap();
    publish(&mut h, "new", None, &next(1));
    assert_eq!(
        h.records(&binding().profile).unwrap()[0].state,
        RunState::Interrupted
    );
}

#[test]
fn preview_cannot_overwrite_a_new_generation_or_different_binding() {
    let mut h = History::in_memory().unwrap();
    publish(&mut h, "one", None, &next(1));
    assert!(matches!(
        h.begin(id("stale"), &binding(), &plan(None)),
        Err(HistoryError::Stale)
    ));
    let mut other = binding();
    other.source = "another".into();
    assert!(matches!(h.baseline(&other), Err(HistoryError::Stale)));
}

#[test]
fn failed_cancelled_and_warning_runs_do_not_publish_baselines() {
    for status in [
        ExecutionStatus::Failed,
        ExecutionStatus::Cancelled,
        ExecutionStatus::SucceededWithWarnings,
    ] {
        let mut h = History::in_memory().unwrap();
        let token = h.begin(id("one"), &binding(), &plan(None)).unwrap();
        assert!(
            h.finish(
                token,
                status,
                serde_json::json!({}),
                Some(&next(1)),
                RetentionPolicy::default()
            )
            .is_err()
        );
        assert!(h.baseline(&binding()).unwrap().is_none());
        assert_eq!(
            h.records(&binding().profile).unwrap()[0].state,
            RunState::Running
        );
    }
}

#[test]
fn history_retention_never_discards_reference_or_interrupted_runs() {
    let mut h = History::in_memory().unwrap();
    let token = h.begin(id("crash"), &binding(), &plan(None)).unwrap();
    drop(token);
    h.mark_interrupted(&id("crash")).unwrap();
    let retention = RetentionPolicy::new(NonZeroUsize::new(1).unwrap());
    for i in 1..=4 {
        let old = h.baseline(&binding()).unwrap();
        let token = h
            .begin(id(&format!("run-{i}")), &binding(), &plan(old.as_ref()))
            .unwrap();
        h.finish(
            token,
            ExecutionStatus::Succeeded,
            serde_json::json!({}),
            Some(&next(i)),
            retention,
        )
        .unwrap();
    }
    let records = h.records(&binding().profile).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].state, RunState::Interrupted);
    assert_eq!(h.baseline(&binding()).unwrap(), Some(next(4)));
}

#[test]
fn transaction_failure_rolls_back_result_and_reference_together() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut h = History::open(&path).unwrap();
    publish(&mut h, "one", None, &next(1));
    let token = h
        .begin(id("two"), &binding(), &plan(Some(&next(1))))
        .unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_publication BEFORE UPDATE ON profiles BEGIN SELECT RAISE(ABORT,'injected disk failure'); END;").unwrap();
    assert!(
        h.finish(
            token,
            ExecutionStatus::Succeeded,
            serde_json::json!({}),
            Some(&next(2)),
            RetentionPolicy::default()
        )
        .is_err()
    );
    assert_eq!(h.baseline(&binding()).unwrap(), Some(next(1)));
    assert_eq!(
        h.records(&binding().profile).unwrap()[1].state,
        RunState::Running
    );
}

#[test]
fn concurrent_connections_cannot_acquire_same_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut a = History::open(&path).unwrap();
    let mut b = History::open(&path).unwrap();
    let _owner = a.begin(id("one"), &binding(), &plan(None)).unwrap();
    assert!(matches!(
        b.begin(id("two"), &binding(), &plan(None)),
        Err(HistoryError::Running)
    ));
}

#[test]
fn unknown_version_foreign_database_and_corruption_are_not_reset() {
    let dir = tempfile::tempdir().unwrap();
    for (name, sql) in [
        ("future", "PRAGMA user_version=999;"),
        (
            "foreign",
            "CREATE TABLE user_data (secret TEXT); INSERT INTO user_data VALUES ('fixture');",
        ),
    ] {
        let path = dir.path().join(name);
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(sql).unwrap();
        drop(c);
        let before = std::fs::read(&path).unwrap();
        assert!(History::open(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    let path = dir.path().join("corrupt");
    std::fs::write(&path, b"not sqlite").unwrap();
    assert!(History::open(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"not sqlite");
}

#[test]
fn reference_cannot_lose_a_tombstone_during_publication() {
    let mut h = History::in_memory().unwrap();
    publish(&mut h, "one", None, &next(1));
    let token = h
        .begin(id("two"), &binding(), &plan(Some(&next(1))))
        .unwrap();
    let no_tombstone = Baseline::new(binding(), 2, BTreeMap::new()).unwrap();
    assert!(
        h.finish(
            token,
            ExecutionStatus::Succeeded,
            serde_json::json!({}),
            Some(&no_tombstone),
            RetentionPolicy::default()
        )
        .is_err()
    );
    assert_eq!(h.baseline(&binding()).unwrap(), Some(next(1)));
}

#[test]
fn different_profiles_cannot_run_against_the_same_physical_root() {
    let mut h = History::in_memory().unwrap();
    let _token = h.begin(id("first"), &binding(), &plan(None)).unwrap();
    let mut other = binding();
    other.profile = ProfileId::new("other").unwrap();
    let mut json = serde_json::to_value(plan(None)).unwrap();
    json["profile"] = "other".into();
    let other_plan = ExecutionPlan::from_json(&json.to_string()).unwrap();
    assert!(matches!(
        h.begin(id("second"), &other, &other_plan),
        Err(HistoryError::Running)
    ));
}
