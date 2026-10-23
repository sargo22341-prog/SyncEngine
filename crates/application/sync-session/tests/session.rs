//! Bidirectional lifecycle and fault-injection acceptance tests.
#![expect(
    clippy::unwrap_used,
    reason = "isolated test fixtures fail immediately"
)]

use connector_api::{StorageConnector, StorageErrorKind};
use connector_memory::{MemoryConnector, path, ts};
use sync_bidirectional::binding;
use sync_core::{
    CancellationToken, ExecutionId, ExecutionStatus, HashAlgorithm, PlanId, ProfileId, Timestamp,
};
use sync_decisions::PlanContext;
use sync_executor::{ExecutionOptions, ExecutionRequest, Executor, PlanExecutor};
use sync_filter::Filter;
use sync_history::{History, RetentionPolicy, RunState};
use sync_journal::{JournalError, JournalRecord, JournalSink, MemoryJournal, OutcomeCode};
use sync_plan::{
    DeletionGuard, DeletionPolicy, ExecutionPlan, OverwritePolicy, PlanPolicy, PlanSide,
};
use sync_scanner::{ScanOptions, scan};
use sync_session::{SessionReport, SessionRequest, execute, preview};

fn context() -> PlanContext {
    PlanContext {
        plan_id: PlanId::new("plan").unwrap(),
        profile: Some(ProfileId::new("profile").unwrap()),
        created_at: Timestamp::now().unwrap(),
        source_location: "left-root".into(),
        destination_location: "right-root".into(),
    }
}
fn policy() -> PlanPolicy {
    PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(
            DeletionGuard::new()
                .with_empty_source_allowed(true)
                .with_max_percent(None)
                .unwrap(),
        ),
    )
}
fn plan(h: &History, l: &dyn StorageConnector, r: &dyn StorageConnector) -> ExecutionPlan {
    preview(
        h,
        l,
        r,
        &context(),
        &Filter::include_all(),
        &policy(),
        &CancellationToken::new(),
    )
    .unwrap()
}
fn run(
    h: &mut History,
    l: &dyn StorageConnector,
    r: &dyn StorageConnector,
    id: &str,
    p: &ExecutionPlan,
) -> SessionReport {
    execute(
        h,
        SessionRequest {
            plan: p,
            source: l,
            destination: r,
            execution: ExecutionId::new(id).unwrap(),
            journal: &mut MemoryJournal::new(),
            cancel: &CancellationToken::new(),
            options: ExecutionOptions::new().with_deletions_confirmed(true),
            versions: None,
        },
        RetentionPolicy::default(),
    )
    .unwrap()
}

#[test]
fn full_cycle_new_modify_delete_both_sides_and_resynchronize() {
    let mut h = History::in_memory().unwrap();
    let l = MemoryConnector::new("l")
        .writable()
        .file("l/new", b"left", ts(1));
    let r = MemoryConnector::new("r")
        .writable()
        .file("r/new", b"right", ts(1));
    let p = plan(&h, &l, &r);
    assert!(run(&mut h, &l, &r, "initial", &p).baseline_updated);
    assert_eq!(l.content("r/new"), r.content("r/new"));
    assert_eq!(l.content("l/new"), r.content("l/new"));
    l.put_file("l/new", b"LEFT", ts(1));
    r.put_file("r/new", b"RIGHT", ts(1));
    let p = plan(&h, &l, &r);
    assert_eq!(p.summary().replace, 2);
    assert!(run(&mut h, &l, &r, "modified", &p).baseline_updated);
    assert_eq!(l.content("r/new"), Some(b"RIGHT".to_vec()));
    assert_eq!(r.content("l/new"), Some(b"LEFT".to_vec()));
    l.remove("l");
    r.remove("r");
    let p = plan(&h, &l, &r);
    assert_eq!(p.summary().delete, 4);
    let report = run(&mut h, &l, &r, "deleted", &p);
    assert!(report.baseline_updated);
    assert_eq!(report.execution.status(), ExecutionStatus::Succeeded);
    let p = plan(&h, &l, &r);
    assert_eq!(p.summary().mutations(), 0);
    assert_eq!(
        p.baseline()
            .unwrap()
            .entries()
            .values()
            .filter(|e| e.is_none())
            .count(),
        4
    );
    assert!(run(&mut h, &l, &r, "again", &p).baseline_updated);
}

#[test]
fn unresolved_conflict_does_not_advance_reference_and_preserves_both() {
    let mut h = History::in_memory().unwrap();
    let l = MemoryConnector::new("l")
        .writable()
        .file("f", b"old", ts(1));
    let r = MemoryConnector::new("r")
        .writable()
        .file("f", b"old", ts(1));
    let p = plan(&h, &l, &r);
    run(&mut h, &l, &r, "initial", &p);
    l.put_file("f", b"AAA", ts(1));
    r.put_file("f", b"BBB", ts(99));
    let p = plan(&h, &l, &r);
    let before = p.baseline().unwrap().clone();
    let report = run(&mut h, &l, &r, "conflict", &p);
    assert!(!report.baseline_updated);
    assert_eq!(
        report.execution.status(),
        ExecutionStatus::SucceededWithWarnings
    );
    assert_eq!(l.content("f"), Some(b"AAA".to_vec()));
    assert_eq!(r.content("f"), Some(b"BBB".to_vec()));
    assert_eq!(h.baseline(before.binding()).unwrap(), Some(before));
}

struct CancelAfterMutation {
    journal: MemoryJournal,
    cancel: CancellationToken,
}

struct CancelBeforeRescan {
    journal: MemoryJournal,
    cancel: CancellationToken,
}
impl JournalSink for CancelBeforeRescan {
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError> {
        let finished = matches!(&record, JournalRecord::ExecutionFinished { .. });
        self.journal.append(record)?;
        if finished {
            self.cancel.cancel();
        }
        Ok(())
    }
}

#[test]
fn cancellation_before_final_rescan_is_explicit_and_does_not_publish() {
    let mut h = History::in_memory().unwrap();
    let l = MemoryConnector::new("l").writable().file("f", b"x", ts(1));
    let r = MemoryConnector::new("r").writable();
    let p = plan(&h, &l, &r);
    let cancel = CancellationToken::new();
    let mut journal = CancelBeforeRescan {
        journal: MemoryJournal::new(),
        cancel: cancel.clone(),
    };
    let result = execute(
        &mut h,
        SessionRequest {
            plan: &p,
            source: &l,
            destination: &r,
            execution: ExecutionId::new("rescan-cancel").unwrap(),
            journal: &mut journal,
            cancel: &cancel,
            options: ExecutionOptions::new(),
            versions: None,
        },
        RetentionPolicy::default(),
    )
    .unwrap();
    assert!(!result.baseline_updated);
    assert!(result.verification_error.is_some());
    assert_eq!(
        h.records(&context().profile.unwrap()).unwrap()[0].state,
        RunState::Finished(ExecutionStatus::Failed)
    );
    let retry = plan(&h, &l, &r);
    assert!(retry.baseline().is_none());
    assert!(run(&mut h, &l, &r, "retry", &retry).baseline_updated);
}
impl JournalSink for CancelAfterMutation {
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError> {
        let done = matches!(
            &record,
            JournalRecord::OperationFinished {
                outcome: OutcomeCode::Done,
                ..
            }
        );
        self.journal.append(record)?;
        if done {
            self.cancel.cancel();
        }
        Ok(())
    }
}

#[test]
fn cooperative_interruption_keeps_baseline_then_fresh_plan_resynchronizes() {
    let mut h = History::in_memory().unwrap();
    let l = MemoryConnector::new("l")
        .writable()
        .file("a", b"old", ts(1))
        .file("b", b"old", ts(1));
    let r = MemoryConnector::new("r")
        .writable()
        .file("a", b"old", ts(1))
        .file("b", b"old", ts(1));
    let p = plan(&h, &l, &r);
    run(&mut h, &l, &r, "initial", &p);
    l.put_file("a", b"new", ts(1));
    r.put_file("b", b"new", ts(1));
    let p = plan(&h, &l, &r);
    let old = p.baseline().unwrap().clone();
    let cancel = CancellationToken::new();
    let mut journal = CancelAfterMutation {
        journal: MemoryJournal::new(),
        cancel: cancel.clone(),
    };
    let report = execute(
        &mut h,
        SessionRequest {
            plan: &p,
            source: &l,
            destination: &r,
            execution: ExecutionId::new("cancel").unwrap(),
            journal: &mut journal,
            cancel: &cancel,
            options: ExecutionOptions::new(),
            versions: None,
        },
        RetentionPolicy::default(),
    )
    .unwrap();
    assert_eq!(report.execution.status(), ExecutionStatus::Cancelled);
    assert!(!report.baseline_updated);
    assert_eq!(h.baseline(old.binding()).unwrap(), Some(old));
    let p = plan(&h, &l, &r);
    assert_eq!(p.summary().replace, 1);
    assert!(run(&mut h, &l, &r, "retry", &p).baseline_updated);
    assert_eq!(l.content("a"), r.content("a"));
    assert_eq!(l.content("b"), r.content("b"));
}

#[test]
fn process_loss_after_mutations_before_state_commit_is_recovered_by_rescan() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("state.sqlite");
    let mut h = History::open(&database).unwrap();
    let l = MemoryConnector::new("l")
        .writable()
        .file("f", b"old", ts(1));
    let r = MemoryConnector::new("r")
        .writable()
        .file("f", b"old", ts(1));
    let p = plan(&h, &l, &r);
    run(&mut h, &l, &r, "initial", &p);
    r.put_file("f", b"new", ts(1));
    let p = plan(&h, &l, &r);
    let old = p.baseline().unwrap().clone();
    let run_id = ExecutionId::new("crash").unwrap();
    let token = h.begin(run_id.clone(), old.binding(), &p).unwrap();
    let result = Executor
        .execute(ExecutionRequest::new(
            &p,
            &l,
            &r,
            run_id.clone(),
            &mut MemoryJournal::new(),
            &CancellationToken::new(),
        ))
        .unwrap();
    assert_eq!(result.status(), ExecutionStatus::Succeeded);
    drop(token);
    drop(h);
    let mut h = History::open(&database).unwrap();
    assert_eq!(h.baseline(old.binding()).unwrap(), Some(old));
    assert_eq!(
        h.records(&context().profile.unwrap()).unwrap()[1].state,
        RunState::Running
    );
    h.mark_interrupted(&run_id).unwrap();
    let p = plan(&h, &l, &r);
    assert_eq!(p.summary().mutations(), 0);
    assert!(run(&mut h, &l, &r, "rescan", &p).baseline_updated);
}

#[test]
fn destination_same_size_same_date_change_after_preview_refuses_overwrite_both_directions() {
    for reverse in [false, true] {
        let mut h = History::in_memory().unwrap();
        let l = MemoryConnector::new("l")
            .writable()
            .file("f", b"old", ts(1));
        let r = MemoryConnector::new("r")
            .writable()
            .file("f", b"old", ts(1));
        let p = plan(&h, &l, &r);
        run(&mut h, &l, &r, "initial", &p);
        (if reverse { &r } else { &l }).put_file("f", b"new", ts(1));
        let p = plan(&h, &l, &r);
        let receiver = if reverse { &l } else { &r };
        receiver.put_file("f", b"XXX", ts(1));
        let report = run(&mut h, &l, &r, "changed", &p);
        assert!(!report.baseline_updated);
        assert_eq!(report.execution.totals().refused, 1);
        assert_eq!(receiver.content("f"), Some(b"XXX".to_vec()));
    }
}

#[test]
fn same_size_same_date_change_after_preview_refuses_deletion() {
    for reverse in [false, true] {
        let mut h = History::in_memory().unwrap();
        let l = MemoryConnector::new("l")
            .writable()
            .file("f", b"old", ts(1));
        let r = MemoryConnector::new("r")
            .writable()
            .file("f", b"old", ts(1));
        let p = plan(&h, &l, &r);
        run(&mut h, &l, &r, "initial", &p);
        (if reverse { &r } else { &l }).remove("f");
        let p = plan(&h, &l, &r);
        let receiver = if reverse { &l } else { &r };
        receiver.put_file("f", b"NEW", ts(1));
        let report = run(&mut h, &l, &r, "delete", &p);
        assert_eq!(report.execution.totals().refused, 1);
        assert_eq!(receiver.content("f"), Some(b"NEW".to_vec()));
        assert!(!report.baseline_updated);
    }
}

#[test]
fn disconnected_root_means_no_deletion_and_no_new_baseline() {
    let mut h = History::in_memory().unwrap();
    let l = MemoryConnector::new("l")
        .writable()
        .file("f", b"old", ts(1));
    let r = MemoryConnector::new("r")
        .writable()
        .file("f", b"old", ts(1));
    let p = plan(&h, &l, &r);
    run(&mut h, &l, &r, "initial", &p);
    l.remove("f");
    let p = plan(&h, &l, &r);
    l.set_disconnected(true);
    assert!(
        execute(
            &mut h,
            SessionRequest {
                plan: &p,
                source: &l,
                destination: &r,
                execution: ExecutionId::new("offline").unwrap(),
                journal: &mut MemoryJournal::new(),
                cancel: &CancellationToken::new(),
                options: ExecutionOptions::new().with_deletions_confirmed(true),
                versions: None
            },
            RetentionPolicy::default()
        )
        .is_err()
    );
    assert_eq!(r.content("f"), Some(b"old".to_vec()));
    assert_eq!(
        h.baseline(p.baseline().unwrap().binding()).unwrap(),
        p.baseline().cloned()
    );
}

#[test]
fn disk_full_never_advances_state_or_runs_later_deletions() {
    let mut h = History::in_memory().unwrap();
    let l = MemoryConnector::new("l")
        .writable()
        .file("a", b"old", ts(1))
        .file("z", b"old", ts(1));
    let r = MemoryConnector::new("r")
        .writable()
        .file("a", b"old", ts(1))
        .file("z", b"old", ts(1))
        .fail_write_after("a", 1, StorageErrorKind::InsufficientSpace);
    let p = plan(&h, &l, &r);
    run(&mut h, &l, &r, "initial", &p);
    l.put_file("a", b"new", ts(1));
    l.remove("z");
    let p = plan(&h, &l, &r);
    let result = run(&mut h, &l, &r, "full", &p);
    assert!(!result.baseline_updated);
    assert_eq!(result.execution.status(), ExecutionStatus::Failed);
    assert_eq!(r.content("a"), Some(b"old".to_vec()));
    assert_eq!(r.content("z"), Some(b"old".to_vec()));
}

#[test]
fn local_sqlite_cycle_and_verified_versions_are_real_files() {
    use connector_local::{LocalConnector, LocalOptions};
    use sync_versioning::VersionStore;
    let temp = tempfile::tempdir().unwrap();
    let left = temp.path().join("left");
    let right = temp.path().join("right");
    let state = temp.path().join("state");
    for p in [&left, &right, &state] {
        std::fs::create_dir(p).unwrap();
    }
    std::fs::write(left.join("l"), b"left").unwrap();
    std::fs::write(right.join("r"), b"right").unwrap();
    let l = LocalConnector::new(&left, LocalOptions::new().with_mutations()).unwrap();
    let r = LocalConnector::new(&right, LocalOptions::new().with_mutations()).unwrap();
    let mut ctx = context();
    ctx.source_location = left.to_str().unwrap().into();
    ctx.destination_location = right.to_str().unwrap().into();
    let database = state.join("state.sqlite");
    let mut h = History::open(&database).unwrap();
    let cancel = CancellationToken::new();
    let p = preview(&h, &l, &r, &ctx, &Filter::include_all(), &policy(), &cancel).unwrap();
    assert!(run(&mut h, &l, &r, "initial", &p).baseline_updated);
    std::fs::write(left.join("l"), b"LEFT").unwrap();
    std::fs::write(right.join("r"), b"RIGHT").unwrap();
    ctx.created_at = Timestamp::now().unwrap();
    let p = preview(&h, &l, &r, &ctx, &Filter::include_all(), &policy(), &cancel).unwrap();
    let versions = LocalConnector::new(&state, LocalOptions::new().with_mutations()).unwrap();
    let store = VersionStore::new(&versions, path("versions")).unwrap();
    let mut journal = sync_journal::FileJournal::create(state.join("run.jsonl")).unwrap();
    let result = execute(
        &mut h,
        SessionRequest {
            plan: &p,
            source: &l,
            destination: &r,
            execution: ExecutionId::new("modified").unwrap(),
            journal: &mut journal,
            cancel: &cancel,
            options: ExecutionOptions::new(),
            versions: Some(&store),
        },
        RetentionPolicy::default(),
    )
    .unwrap();
    assert!(result.baseline_updated);
    assert_eq!(std::fs::read(left.join("r")).unwrap(), b"RIGHT");
    assert_eq!(std::fs::read(right.join("l")).unwrap(), b"LEFT");
    assert_eq!(std::fs::read(state.join("versions/l")).unwrap(), b"left");
    assert_eq!(std::fs::read(state.join("versions/r")).unwrap(), b"right");
    drop(h);
    let h = History::open(&database).unwrap();
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let binding = binding(
        &scan(&l, &Filter::include_all(), &options, &cancel).unwrap(),
        &scan(&r, &Filter::include_all(), &options, &cancel).unwrap(),
        &ctx,
    )
    .unwrap();
    assert_eq!(h.baseline(&binding).unwrap().unwrap().generation(), 2);
    let replay = sync_journal::read_journal(&state.join("run.jsonl")).unwrap();
    assert_eq!(replay.recovery().finished, Some(ExecutionStatus::Succeeded));
    assert!(
        result
            .execution
            .outcomes()
            .iter()
            .any(|o| o.target() == PlanSide::Source)
    );
}
