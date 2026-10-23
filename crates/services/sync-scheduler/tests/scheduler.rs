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
    };
    Ok(Schedule {
        approved: profile,
        trigger: Trigger::Interval { seconds: 60 },
        created_at: 100,
        enabled: true,
        allow_deletions: false,
        missed: MissedJobs::RunOnce,
    })
}
struct Mock {
    calls: Cell<usize>,
    status: JobStatus,
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
        2
    );
    Ok(())
}
