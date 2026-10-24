use crate::{
    Completion, DispatchReceipt, JobStatus, Schedule, ScheduledApplication, SchedulerError, Wake,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;
use sync_core::{CancellationToken, ProfileId};
/// Rewrites every stored configuration in the current serialized form (v3 adds
/// `show_window`). Required because a claim compares the stored text with the
/// re-serialized schedule: an old text would never match and never run.
/// A malformed row aborts the migration; nothing is reset.
fn normalize_configs(tx: &rusqlite::Transaction<'_>) -> Result<(), SchedulerError> {
    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT profile,config FROM schedules")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    for (profile, config) in rows {
        let schedule: Schedule = serde_json::from_str(&config)?;
        tx.execute(
            "UPDATE schedules SET config=?2 WHERE profile=?1",
            params![profile, serde_json::to_string(&schedule)?],
        )?;
    }
    Ok(())
}
/// Latest durable claim and receipt. A nonempty running slot blocks this profile
/// until explicit diagnosis/acknowledgement; it is never silently retried.
#[derive(Debug, Serialize)]
pub struct ScheduleState {
    /// Approved schedule configuration.
    pub schedule: Schedule,
    /// Last completed/acknowledged occurrence, independent of success.
    pub last_slot: Option<i64>,
    /// Persisted in-flight occurrence; retained after abrupt termination.
    pub running_slot: Option<i64>,
    /// Latest result only (bounded); detailed application history is separate.
    pub result: Option<Completion>,
}
/// SQLite v3 store (transactional v1/v2 migrations). One schedule per profile, one receipt per schedule.
/// Keep outside synchronized roots; back up with all application state at rest.
pub struct Scheduler(Connection);
impl Scheduler {
    /// Opens/migrates only an empty store, checking identity and integrity.
    /// # Errors
    /// Corruption, foreign/future schema, permissions, busy or SQL failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SchedulerError> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(SchedulerError::Invalid("corrupt scheduler database"));
        }
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let app: u32 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        if version == 0 && app == 0 {
            let tables: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if tables != 0 {
                return Err(SchedulerError::Invalid("foreign database"));
            }
            tx.execute_batch(include_str!("../migrations/001_initial.sql"))?;
        } else if !(1..=3).contains(&version) || app != 1_398_361_682 {
            return Err(SchedulerError::Invalid("unknown scheduler schema"));
        }
        if version < 2 {
            tx.execute_batch(include_str!("../migrations/002_session_wakes.sql"))?;
        }
        if version < 3 {
            normalize_configs(&tx)?;
            tx.execute_batch(include_str!("../migrations/003_show_window.sql"))?;
        }
        tx.commit()?;
        db.execute_batch("PRAGMA synchronous=FULL;")?;
        Ok(Self(db))
    }
    /// Saves an approved schedule. Running/interrupted claims cannot be overwritten.
    /// Existing completion watermark is retained, so editing cannot replay a slot.
    /// # Errors
    /// Invalid input, running claim or persistence failure.
    pub fn save(&self, schedule: &Schedule) -> Result<(), SchedulerError> {
        schedule.validate()?;
        let changed=self.0.execute("INSERT INTO schedules(profile,config) VALUES(?1,?2) ON CONFLICT(profile) DO UPDATE SET config=excluded.config WHERE running_slot IS NULL",params![schedule.approved.id.as_str(),serde_json::to_string(schedule)?])?;
        if changed != 1 {
            return Err(SchedulerError::Invalid("running claim requires diagnosis"));
        }
        Ok(())
    }
    /// Lists at most 1000 schedules; exceeding the limit is an explicit error.
    /// # Errors
    /// Read/format/invariant error (never silently skipped).
    pub fn list(&self) -> Result<Vec<ScheduleState>, SchedulerError> {
        let mut stmt=self.0.prepare("SELECT profile,config,last_slot,running_slot,result FROM schedules ORDER BY profile LIMIT 1001")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        let values: Result<Vec<_>, SchedulerError> = rows
            .map(|row| {
                let (id, json, last_slot, running_slot, result) = row?;
                let schedule: Schedule = serde_json::from_str(&json)?;
                schedule.validate()?;
                if schedule.approved.id.as_str() != id {
                    return Err(SchedulerError::Invalid("schedule identity mismatch"));
                }
                Ok(ScheduleState {
                    schedule,
                    last_slot,
                    running_slot,
                    result: result.map(|j| serde_json::from_str(&j)).transpose()?,
                })
            })
            .collect();
        let values = values?;
        if values.len() > 1000 {
            return Err(SchedulerError::Invalid("schedule limit exceeded"));
        }
        Ok(values)
    }
    /// Removes a schedule only when no claim is running. Content/history retained.
    /// # Errors
    /// Active/interrupted claim or persistence failure.
    pub fn remove(&self, id: &ProfileId) -> Result<(), SchedulerError> {
        let count = self.0.execute(
            "DELETE FROM schedules WHERE profile=?1 AND running_slot IS NULL",
            [id.as_str()],
        )?;
        let exists: Option<i32> = self
            .0
            .query_row(
                "SELECT 1 FROM schedules WHERE profile=?1",
                [id.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if count == 0 && exists.is_some() {
            return Err(SchedulerError::Invalid("running claim requires diagnosis"));
        }
        Ok(())
    }
    /// Acknowledges an interrupted claim AFTER manual journal/state diagnosis.
    /// This does not recover files or clear application/bidirectional intentions.
    /// # Errors
    /// No running claim or persistence failure.
    pub fn acknowledge(&self, id: &ProfileId) -> Result<(), SchedulerError> {
        let result = serde_json::to_string(&Completion {
            status: JobStatus::Failed,
            code: "interruption_acknowledged".into(),
        })?;
        if self.0.execute("UPDATE schedules SET last_slot=running_slot,running_slot=NULL,result=?2 WHERE profile=?1 AND running_slot IS NOT NULL",params![id.as_str(),result])?!=1 {return Err(SchedulerError::Invalid("no interrupted claim"));}
        Ok(())
    }
    /// Dispatches due jobs sequentially; commits each claim before invoking the port.
    /// Missed periods coalesce; clock rollback cannot replay completed occurrences.
    /// Cancellation stops between profiles and is passed to the application.
    /// # Errors
    /// Store/config/claim/completion failure. On receipt failure claim stays running.
    pub fn dispatch(
        &mut self,
        now: i64,
        wake: Wake,
        app: &impl ScheduledApplication,
        cancel: &CancellationToken,
    ) -> Result<Vec<DispatchReceipt>, SchedulerError> {
        if !(0..=i64::MAX - 86_400).contains(&now) {
            return Err(SchedulerError::Invalid("clock out of range"));
        }
        if matches!(wake, Wake::Session(platform_api::SessionEvent::Logon)) {
            // Coalesce duplicate native notifications within one minute. Never
            // let a wall-clock rollback replace a newer observed event.
            self.0.execute("INSERT INTO session_wakes VALUES('logon',?1) ON CONFLICT(event) DO UPDATE SET occurred_at=excluded.occurred_at WHERE excluded.occurred_at/60>session_wakes.occurred_at/60", [now])?;
        }
        let logon: Option<i64> = self
            .0
            .query_row(
                "SELECT occurred_at FROM session_wakes WHERE event='logon'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let mut results = Vec::new();
        for state in self.list()? {
            if cancel.is_cancelled() {
                break;
            }
            if let Some(slot) = state.running_slot {
                results.push(DispatchReceipt {
                    profile: state.schedule.approved.id.clone(),
                    occurrence: slot,
                    completion: Completion {
                        status: JobStatus::Attention,
                        code: "in_flight_or_interrupted_requires_diagnosis".into(),
                    },
                });
                continue;
            }
            let due = if let crate::Trigger::LogonDelayed { seconds } = state.schedule.trigger {
                logon
                    .filter(|event| state.schedule.enabled && *event >= state.schedule.created_at)
                    .and_then(|event| event.checked_add(i64::from(seconds)))
                    .filter(|due| {
                        *due <= now
                            && (state.schedule.missed != crate::MissedJobs::Skip || now - *due < 60)
                    })
            } else {
                state.schedule.slot(now, wake)
            };
            let Some(slot) = due else {
                continue;
            };
            let json = serde_json::to_string(&state.schedule)?;
            let id = state.schedule.approved.id.as_str();
            // Atomic compare-and-claim handles concurrent dispatchers and edits.
            if self.0.execute("UPDATE schedules SET running_slot=?2,result=NULL WHERE profile=?1 AND config=?3 AND running_slot IS NULL AND (last_slot IS NULL OR last_slot<?2)",params![id,slot,json])?!=1 {continue;}
            let result = app.run(&state.schedule, cancel);
            let receipt = serde_json::to_string(&result)?;
            self.0.execute("UPDATE schedules SET last_slot=CASE WHEN ?3 THEN last_slot ELSE running_slot END,running_slot=NULL,result=?4 WHERE profile=?1 AND running_slot=?2",params![id,slot,result.status==JobStatus::Deferred,receipt])?;
            results.push(DispatchReceipt {
                profile: state.schedule.approved.id.clone(),
                occurrence: slot,
                completion: result,
            });
        }
        Ok(results)
    }
}
