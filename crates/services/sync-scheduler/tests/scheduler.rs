//! Durable claims, mock application failures and controllable clocks.
use platform_api::SessionEvent;
use std::cell::Cell;
use sync_core::{CancellationToken, ProfileId};
use sync_scheduler::*;
fn schedule(temp: &tempfile::TempDir) -> Result<Schedule, Box<dyn std::error::Error>> {
    let profile = sync_profiles::Profile {
        id: ProfileId::new("p")?,
        name: "test".into(),
        group: String::new(),
        mode: serde_json::from_str("\"backup\"")?,
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        filter: Default::default(),
        remote_source: None,
        remote_destination: None,
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
        trash: sync_profiles::TrashPolicy::legacy(),
    };
    Ok(Schedule {
        approved: profile,
        trigger: Trigger::Interval { seconds: 60 },
        created_at: 100,
        enabled: true,
        allow_deletions: false,
        missed: MissedJobs::RunOnce,
        show_window: false,
    })
}
struct Mock {
    calls: Cell<usize>,
    status: JobStatus,
}

#[test]
fn same_logon_profiles_queue_sequentially_and_cancellation_keeps_pending_work()
-> Result<(), Box<dyn std::error::Error>> {
    use std::cell::RefCell;
    struct Queued {
        path: std::path::PathBuf,
        calls: RefCell<Vec<String>>,
        queues: RefCell<Vec<(Option<String>, Vec<String>)>>,
        cancel_first: bool,
    }
    impl ScheduledApplication for Queued {
        fn run(&self, schedule: &Schedule, cancel: &CancellationToken) -> Completion {
            let db = rusqlite::Connection::open(&self.path).unwrap();
            let count: i64 = db
                .query_row(
                    "SELECT count(*) FROM schedules WHERE running_slot IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "only the current profile may be claimed");
            self.calls
                .borrow_mut()
                .push(schedule.approved.id.to_string());
            if self.cancel_first {
                cancel.cancel();
            }
            Completion {
                status: JobStatus::Failed,
                code: "isolated business failure".into(),
            }
        }
        fn queue_changed(&self, current: Option<&QueuedProfile>, pending: &[QueuedProfile]) {
            self.queues.borrow_mut().push((
                current.map(|p| p.id.to_string()),
                pending.iter().map(|p| p.id.to_string()).collect(),
            ));
        }
    }
    for cancel_first in [false, true] {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("scheduler.sqlite");
        let mut db = Scheduler::open(&path)?;
        for id in ["a", "b", "c"] {
            let mut s = schedule(&temp)?;
            s.approved.id = ProfileId::new(id)?;
            s.approved.name = format!("Profile {id}");
            s.trigger = Trigger::Logon;
            db.save(&s)?;
        }
        db.observe_session("isolated-logon", 200)?;
        let app = Queued {
            path,
            calls: RefCell::new(Vec::new()),
            queues: RefCell::new(Vec::new()),
            cancel_first,
        };
        db.dispatch(200, Wake::Clock, &app, &CancellationToken::new())?;
        assert_eq!(
            app.queues.borrow()[0],
            (Some("a".into()), vec!["b".into(), "c".into()])
        );
        if cancel_first {
            assert_eq!(*app.calls.borrow(), vec!["a"]);
            assert_eq!(
                app.queues.borrow().last(),
                Some(&(None, vec!["b".into(), "c".into()]))
            );
            assert!(
                db.list()?
                    .iter()
                    .skip(1)
                    .all(|row| row.last_slot.is_none() && row.running_slot.is_none())
            );
            let resumed = Queued {
                cancel_first: false,
                ..app
            };
            db.dispatch(200, Wake::Clock, &resumed, &CancellationToken::new())?;
            assert_eq!(*resumed.calls.borrow(), vec!["a", "b", "c"]);
        } else {
            assert_eq!(*app.calls.borrow(), vec!["a", "b", "c"]);
            assert_eq!(app.queues.borrow()[1], (Some("b".into()), vec!["c".into()]));
            assert_eq!(app.queues.borrow().last(), Some(&(None, vec![])));
        }
        assert!(db.list()?.iter().all(|row| row.running_slot.is_none()));
    }
    Ok(())
}
impl ScheduledApplication for Mock {
    fn run(&self, _: &Schedule, _: &CancellationToken) -> Completion {
        self.calls.set(self.calls.get() + 1);
        Completion {
            status: self.status,
            code: "mock".into(),
        }
    }
}
#[test]
fn missed_jobs_coalesce_restart_and_clock_rollback_do_not_replay()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("schedule.sqlite");
    let mut db = Scheduler::open(&path)?;
    let s = schedule(&temp)?;
    db.save(&s)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let cancel = CancellationToken::new();
    assert!(db.dispatch(99, Wake::Clock, &app, &cancel)?.is_empty());
    assert_eq!(db.dispatch(1000, Wake::Clock, &app, &cancel)?.len(), 1);
    assert_eq!(db.list()?[0].last_slot, Some(1000));
    drop(db);
    let mut db = Scheduler::open(&path)?;
    assert!(db.dispatch(1000, Wake::Clock, &app, &cancel)?.is_empty());
    assert!(db.dispatch(500, Wake::Clock, &app, &cancel)?.is_empty());
    db.dispatch(1060, Wake::Clock, &app, &cancel)?;
    assert_eq!(app.calls.get(), 2);
    Ok(())
}
#[test]
fn skip_daily_utc_and_session_capabilities_are_explicit() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempfile::tempdir()?;
    let mut db = Scheduler::open(temp.path().join("db"))?;
    let mut s = schedule(&temp)?;
    s.created_at = 0;
    s.trigger = Trigger::DailyUtc {
        hour: 2,
        minute: 30,
    };
    s.missed = MissedJobs::Skip;
    db.save(&s)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Attention,
    };
    let cancel = CancellationToken::new();
    assert!(
        db.dispatch(9000 + 61, Wake::Clock, &app, &cancel)?
            .is_empty()
    );
    assert_eq!(
        db.dispatch(9000 + 86400, Wake::Clock, &app, &cancel)?[0]
            .completion
            .status,
        JobStatus::Attention
    );
    let mut s = schedule(&temp)?;
    s.approved.id = ProfileId::new("session")?;
    s.trigger = Trigger::Logon;
    db.save(&s)?;
    assert!(
        db.dispatch(
            200,
            Wake::Session(SessionEvent::UserManagerStart),
            &app,
            &cancel
        )?
        .is_empty()
    );
    assert_eq!(
        db.dispatch(200, Wake::Session(SessionEvent::Logon), &app, &cancel)?
            .len(),
        1
    );
    assert!(
        db.dispatch(201, Wake::Session(SessionEvent::Logon), &app, &cancel)?
            .is_empty()
    );
    Ok(())
}
#[test]
fn deferred_retries_failure_does_not_and_cancel_prevents_claims()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mut db = Scheduler::open(temp.path().join("db"))?;
    db.save(&schedule(&temp)?)?;
    let busy = Mock {
        calls: Cell::new(0),
        status: JobStatus::Deferred,
    };
    let cancel = CancellationToken::new();
    db.dispatch(100, Wake::Clock, &busy, &cancel)?;
    db.dispatch(100, Wake::Clock, &busy, &cancel)?;
    assert_eq!(busy.calls.get(), 2);
    let failed = Mock {
        calls: Cell::new(0),
        status: JobStatus::Failed,
    };
    db.dispatch(100, Wake::Clock, &failed, &cancel)?;
    assert!(db.dispatch(100, Wake::Clock, &failed, &cancel)?.is_empty());
    cancel.cancel();
    assert!(db.dispatch(160, Wake::Clock, &failed, &cancel)?.is_empty());
    assert_eq!(failed.calls.get(), 1);
    Ok(())
}
#[test]
fn durable_claim_survives_process_loss_and_requires_diagnosis()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Scheduler::open(&path)?;
    let s = schedule(&temp)?;
    db.save(&s)?;
    drop(db);
    // Given durable intent committed, When owner disappears, Then no re-execution.
    rusqlite::Connection::open(&path)?.execute("UPDATE schedules SET running_slot=100", [])?;
    let mut db = Scheduler::open(&path)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    assert_eq!(
        db.dispatch(160, Wake::Clock, &app, &CancellationToken::new())?[0]
            .completion
            .status,
        JobStatus::Attention
    );
    assert_eq!(app.calls.get(), 0);
    assert!(db.save(&s).is_err());
    assert!(db.remove(&s.approved.id).is_err());
    db.acknowledge(&s.approved.id)?;
    assert_eq!(
        db.list()?[0].result.as_ref().ok_or("receipt")?.status,
        JobStatus::Failed
    );
    db.dispatch(160, Wake::Clock, &app, &CancellationToken::new())?;
    assert_eq!(app.calls.get(), 1);
    Ok(())
}
struct Concurrent {
    path: std::path::PathBuf,
    observed: Cell<bool>,
}
impl ScheduledApplication for Concurrent {
    fn run(&self, _: &Schedule, _: &CancellationToken) -> Completion {
        let result = (|| -> Result<bool, SchedulerError> {
            let mut other = Scheduler::open(&self.path)?;
            let app = Mock {
                calls: Cell::new(0),
                status: JobStatus::Succeeded,
            };
            other.dispatch(100, Wake::Clock, &app, &CancellationToken::new())?;
            Ok(app.calls.get() == 0 && other.list()?[0].running_slot == Some(100))
        })();
        self.observed.set(result.unwrap_or(false));
        Completion {
            status: JobStatus::Succeeded,
            code: "mock".into(),
        }
    }
}
#[test]
fn claim_is_visible_before_application_call_and_blocks_second_connection()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let mut db = Scheduler::open(&path)?;
    db.save(&schedule(&temp)?)?;
    let app = Concurrent {
        path,
        observed: Cell::new(false),
    };
    db.dispatch(100, Wake::Clock, &app, &CancellationToken::new())?;
    assert!(app.observed.get());
    Ok(())
}
#[test]
fn invalid_future_foreign_and_corrupt_stores_are_not_reset()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let db = Scheduler::open(&path)?;
    let mut s = schedule(&temp)?;
    s.trigger = Trigger::Interval { seconds: 0 };
    assert!(db.save(&s).is_err());
    s.trigger = Trigger::DailyUtc {
        hour: 24,
        minute: 0,
    };
    assert!(db.save(&s).is_err());
    drop(db);
    let sql = rusqlite::Connection::open(&path)?;
    sql.execute_batch("PRAGMA user_version=99")?;
    drop(sql);
    assert!(Scheduler::open(&path).is_err());
    let foreign = temp.path().join("foreign");
    let sql = rusqlite::Connection::open(&foreign)?;
    sql.execute_batch("CREATE TABLE keep(data)")?;
    drop(sql);
    assert!(Scheduler::open(foreign).is_err());
    let corrupt = temp.path().join("corrupt");
    std::fs::write(&corrupt, b"not sqlite")?;
    assert!(Scheduler::open(&corrupt).is_err());
    assert_eq!(std::fs::read(corrupt)?, b"not sqlite");
    Ok(())
}

#[test]
fn failed_receipt_write_keeps_claim_and_prevents_unsafe_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let mut db = Scheduler::open(&path)?;
    db.save(&schedule(&temp)?)?;
    rusqlite::Connection::open(&path)?.execute_batch("CREATE TRIGGER receipt_failure BEFORE UPDATE ON schedules WHEN NEW.running_slot IS NULL BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;")?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    assert!(
        db.dispatch(100, Wake::Clock, &app, &CancellationToken::new())
            .is_err()
    );
    assert_eq!(app.calls.get(), 1);
    assert_eq!(db.list()?[0].running_slot, Some(100));
    assert_eq!(
        db.dispatch(160, Wake::Clock, &app, &CancellationToken::new())?[0]
            .completion
            .status,
        JobStatus::Attention
    );
    assert_eq!(app.calls.get(), 1);
    Ok(())
}
#[test]
fn disable_and_remove_do_not_invoke_application() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mut db = Scheduler::open(temp.path().join("db"))?;
    let mut s = schedule(&temp)?;
    s.enabled = false;
    db.save(&s)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    assert!(
        db.dispatch(100, Wake::Clock, &app, &CancellationToken::new())?
            .is_empty()
    );
    db.remove(&s.approved.id)?;
    assert!(db.list()?.is_empty());
    assert_eq!(app.calls.get(), 0);
    Ok(())
}

#[test]
fn delayed_logon_survives_restart_and_never_runs_early_or_replays()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let mut db = Scheduler::open(&path)?;
    let mut s = schedule(&temp)?;
    s.trigger = Trigger::LogonDelayed { seconds: 90 };
    db.save(&s)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let cancel = CancellationToken::new();
    // Given no native logon, clock polls alone cannot invent a session event.
    assert!(db.dispatch(200, Wake::Clock, &app, &cancel)?.is_empty());
    assert!(
        db.dispatch(200, Wake::Session(SessionEvent::Logon), &app, &cancel)?
            .is_empty()
    );
    // Duplicate native event in the same minute must not postpone the deadline.
    assert!(
        db.dispatch(205, Wake::Session(SessionEvent::Logon), &app, &cancel)?
            .is_empty()
    );
    drop(db);
    let mut db = Scheduler::open(&path)?;
    assert!(db.dispatch(289, Wake::Clock, &app, &cancel)?.is_empty());
    assert_eq!(db.dispatch(290, Wake::Clock, &app, &cancel)?.len(), 1);
    assert!(db.dispatch(300, Wake::Clock, &app, &cancel)?.is_empty());
    assert!(db.dispatch(280, Wake::Clock, &app, &cancel)?.is_empty());
    assert_eq!(app.calls.get(), 1);
    Ok(())
}
#[test]
fn migration_v1_preserves_authorization_and_completion() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let s = schedule(&temp)?;
    let old = rusqlite::Connection::open(&path)?;
    old.execute_batch(include_str!("../migrations/001_initial.sql"))?;
    old.execute(
        "INSERT INTO schedules(profile,config,last_slot) VALUES(?1,?2,123)",
        rusqlite::params![s.approved.id.as_str(), serde_json::to_string(&s)?],
    )?;
    drop(old);
    let db = Scheduler::open(&path)?;
    let rows = db.list()?;
    assert_eq!(rows[0].schedule, s);
    assert_eq!(rows[0].last_slot, Some(123));
    assert_eq!(
        rusqlite::Connection::open(&path)?
            .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
        4
    );
    Ok(())
}
/// Given a v2 store whose configuration predates `show_window`,
/// when it is opened and dispatched, then the schedule still runs (the stored
/// text is normalized, otherwise the compare-and-claim would never match).
#[test]
fn migration_v2_normalizes_config_so_claims_still_match() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let s = schedule(&temp)?;
    let mut legacy = serde_json::to_value(&s)?;
    legacy
        .as_object_mut()
        .ok_or("object expected")?
        .remove("show_window");
    let old = rusqlite::Connection::open(&path)?;
    old.execute_batch(include_str!("../migrations/001_initial.sql"))?;
    old.execute_batch(include_str!("../migrations/002_session_wakes.sql"))?;
    old.execute(
        "INSERT INTO schedules(profile,config) VALUES(?1,?2)",
        rusqlite::params![s.approved.id.as_str(), legacy.to_string()],
    )?;
    drop(old);
    let mut db = Scheduler::open(&path)?;
    assert_eq!(db.list()?[0].schedule, s);
    assert!(!db.list()?[0].schedule.show_window);
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let receipts = db.dispatch(1000, Wake::Clock, &app, &CancellationToken::new())?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(app.calls.get(), 1);
    Ok(())
}
/// The presentation flag round-trips and does not change dispatch.
#[test]
fn show_window_is_persisted() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mut db = Scheduler::open(temp.path().join("db"))?;
    let s = Schedule {
        show_window: true,
        ..schedule(&temp)?
    };
    db.save(&s)?;
    assert!(db.list()?[0].schedule.show_window);
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    assert_eq!(
        db.dispatch(1000, Wake::Clock, &app, &CancellationToken::new())?
            .len(),
        1
    );
    Ok(())
}

#[test]
fn resident_session_restart_delay_and_new_session_are_durable()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let mut db = Scheduler::open(&path)?;
    let mut s = schedule(&temp)?;
    s.trigger = Trigger::LogonDelayed { seconds: 90 };
    db.save(&s)?;
    db.observe_session("boot-a:session-1:2000000000", 200)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let cancel = CancellationToken::new();
    assert!(db.dispatch(289, Wake::Clock, &app, &cancel)?.is_empty());
    drop(db);
    let mut db = Scheduler::open(&path)?;
    db.observe_session("boot-a:session-1:2000000000", 200)?;
    assert_eq!(db.dispatch(290, Wake::Clock, &app, &cancel)?.len(), 1);
    // A legacy logon notification or restart must not delay/replay this session.
    assert!(
        db.dispatch(400, Wake::Session(SessionEvent::Logon), &app, &cancel)?
            .is_empty()
    );
    db.observe_session("boot-b:session-1:5000000000", 500)?;
    assert!(db.dispatch(589, Wake::Clock, &app, &cancel)?.is_empty());
    assert_eq!(db.dispatch(590, Wake::Clock, &app, &cancel)?.len(), 1);
    // An older concurrent desktop session cannot replace the newest observation.
    db.observe_session("boot-a:session-1:2000000000", 200)?;
    assert!(db.dispatch(600, Wake::Clock, &app, &cancel)?.is_empty());
    assert_eq!(app.calls.get(), 2);
    Ok(())
}

#[test]
fn resident_logon_once_skip_clock_jumps_and_schedule_edits()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("db");
    let mut db = Scheduler::open(&path)?;
    let mut s = schedule(&temp)?;
    s.trigger = Trigger::Logon;
    db.save(&s)?;
    db.observe_session("session:200", 200)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let token = CancellationToken::new();
    assert_eq!(db.dispatch(205, Wake::Clock, &app, &token)?.len(), 1);
    db.observe_session("session:200", 200)?;
    assert!(db.dispatch(4000, Wake::Clock, &app, &token)?.is_empty());
    assert!(db.dispatch(150, Wake::Clock, &app, &token)?.is_empty());
    // An independent GUI/CLI connection edits the running resident's store.
    let other = Scheduler::open(&path)?;
    s.trigger = Trigger::Interval { seconds: 60 };
    s.created_at = 1000;
    s.missed = MissedJobs::Skip;
    other.save(&s)?;
    assert!(db.dispatch(999, Wake::Clock, &app, &token)?.is_empty());
    assert_eq!(db.dispatch(1060, Wake::Clock, &app, &token)?.len(), 1);
    other.remove(&s.approved.id)?;
    assert!(db.dispatch(1120, Wake::Clock, &app, &token)?.is_empty());
    Ok(())
}

#[test]
fn resident_migration_does_not_replay_legacy_logon_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mut db = Scheduler::open(temp.path().join("db"))?;
    let mut s = schedule(&temp)?;
    s.trigger = Trigger::Logon;
    db.save(&s)?;
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let token = CancellationToken::new();
    assert_eq!(
        db.dispatch(205, Wake::Session(SessionEvent::Logon), &app, &token)?
            .len(),
        1
    );
    // The historical receipt is rounded to 180; actual WTS logon was 201.
    db.observe_session("session:201", 201)?;
    assert!(db.dispatch(250, Wake::Clock, &app, &token)?.is_empty());
    assert_eq!(app.calls.get(), 1);
    Ok(())
}

#[test]
fn resident_session_missed_policy_and_ambiguous_identity_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let mut db = Scheduler::open(temp.path().join("db"))?;
    let mut s = schedule(&temp)?;
    s.trigger = Trigger::LogonDelayed { seconds: 90 };
    s.missed = MissedJobs::Skip;
    db.save(&s)?;
    db.observe_session("session:200", 200)?;
    assert!(db.observe_session("different:200", 200).is_err());
    let app = Mock {
        calls: Cell::new(0),
        status: JobStatus::Succeeded,
    };
    let token = CancellationToken::new();
    assert!(db.dispatch(350, Wake::Clock, &app, &token)?.is_empty());
    s.missed = MissedJobs::RunOnce;
    db.save(&s)?;
    assert_eq!(db.dispatch(100000, Wake::Clock, &app, &token)?.len(), 1);
    assert!(db.dispatch(100001, Wake::Clock, &app, &token)?.is_empty());
    Ok(())
}
