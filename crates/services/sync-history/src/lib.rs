//! SQLite reference state and history. Migrations and state publication are
//! transactional. A running execution remains visible after process death;
//! callers must explicitly diagnose it before marking it interrupted.
//! I/O is synchronous: async callers must use a dedicated blocking thread.

use std::num::NonZeroUsize;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sync_core::{ExecutionId, ExecutionStatus, ProfileId, Timestamp};
use sync_errors::{ClassifiedError, ErrorClass};
use sync_plan::{Baseline, ExecutionPlan, StateBinding, SyncMode};
use thiserror::Error;

/// Current SQLite schema (migration from empty version zero is supported).
pub const HISTORY_SCHEMA_VERSION: u32 = 1;
const APPLICATION_ID: u32 = 1_398_361_669;

/// Persistence failures are explicit; none means an empty baseline.
#[derive(Debug, Error)]
pub enum HistoryError {
    /// Backup reservation failed; existing files are never overwritten.
    #[error("history backup: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite operation failed.
    #[error("SQLite history: {0}")]
    Sql(#[from] rusqlite::Error),
    /// Invalid serialized state.
    #[error("invalid persisted history: {0}")]
    Format(#[from] serde_json::Error),
    /// Unknown schema/application or invalid state transition.
    #[error("history integrity: {0}")]
    Integrity(&'static str),
    /// Another execution is still running; explicit recovery is required.
    #[error("profile has an unfinished execution")]
    Running,
    /// State changed after preview, or endpoint/filter identity changed.
    #[error("stale plan or mismatching reference identity")]
    Stale,
}

impl ClassifiedError for HistoryError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Sql(_) | Self::Io(_) => ErrorClass::Unavailable,
            Self::Format(_) | Self::Integrity(_) => ErrorClass::Integrity,
            Self::Running | Self::Stale => ErrorClass::Conflict,
        }
    }
}

/// Bound completed history by count. Baselines, tombstones and unfinished
/// or interrupted records are never purged by this policy.
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    completed_per_profile: NonZeroUsize,
}
impl RetentionPolicy {
    /// Retains at least this many most recent completed records per profile.
    #[must_use]
    pub fn new(completed_per_profile: NonZeroUsize) -> Self {
        Self {
            completed_per_profile,
        }
    }
}
impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            completed_per_profile: NonZeroUsize::MIN.saturating_add(99),
        }
    }
}

/// Durable execution state. `Interrupted` requires a fresh scan, not replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// Durable intent, without a final result.
    Running,
    /// Explicitly diagnosed after its owner stopped.
    Interrupted,
    /// Completed attempt; may have failed or have warnings.
    Finished(ExecutionStatus),
}

/// Inspectable persisted execution including the exact preview/execution plan.
#[derive(Debug)]
pub struct HistoryRecord {
    /// Actual UTC time of durable execution intent, separate from plan time.
    pub started_at: Timestamp,
    /// UTC completion or explicit interruption-diagnosis time.
    pub finished_at: Option<Timestamp>,
    /// Execution identifier.
    pub execution: ExecutionId,
    /// Durable state.
    pub state: RunState,
    /// Exact plan, including directions and baseline evidence.
    pub plan: ExecutionPlan,
    /// Structured executor report/diagnostic, if available.
    pub report: Option<serde_json::Value>,
}

/// Exclusive logical ownership token, obtained only after durable begin.
/// Dropping it intentionally leaves `Running` for interruption diagnosis.
#[derive(Debug)]
pub struct RunToken {
    execution: ExecutionId,
    binding: StateBinding,
    generation: u64,
}

/// SQLite owner. Place the database outside both synchronized trees.
pub struct History {
    connection: Connection,
}

impl History {
    /// Opens or creates a database, checks its identity and applies migrations.
    /// Corrupt/unknown databases are refused without reset or recreation.
    ///
    /// # Errors
    /// I/O, integrity, unknown schema or migration failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        Self::initialize(Connection::open(path)?)
    }

    /// Isolated in-memory database with the same migrations and invariants.
    ///
    /// # Errors
    /// SQLite initialization or migration failure.
    pub fn in_memory() -> Result<Self, HistoryError> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self, HistoryError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let check: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(HistoryError::Integrity("SQLite quick_check failed"));
        }
        // Serialize the version inspection and migration across processes.
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let application: u32 = tx.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        if version == 0 && application == 0 {
            let tables: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if tables != 0 {
                return Err(HistoryError::Integrity("nonempty foreign database"));
            }
            tx.execute_batch(include_str!("../migrations/001_initial.sql"))?;
        } else if version != HISTORY_SCHEMA_VERSION || application != APPLICATION_ID {
            return Err(HistoryError::Integrity(
                "unsupported database version or application",
            ));
        }
        tx.commit()?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        Ok(Self { connection })
    }

    /// Loads validated state; an unknown profile returns `None`.
    ///
    /// # Errors
    /// Identity mismatch, invalid JSON or SQL failure.
    pub fn baseline(&self, binding: &StateBinding) -> Result<Option<Baseline>, HistoryError> {
        let row: Option<(String, i64, Option<String>)> = self
            .connection
            .query_row(
                "SELECT binding,generation,baseline FROM profiles WHERE profile=?1",
                [&binding.profile.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((identity, generation, json)) = row else {
            return Ok(None);
        };
        if serde_json::from_str::<StateBinding>(&identity)? != *binding {
            return Err(HistoryError::Stale);
        }
        let generation = u64::try_from(generation)
            .map_err(|_| HistoryError::Integrity("negative generation"))?;
        let baseline: Option<Baseline> = json.map(|j| serde_json::from_str(&j)).transpose()?;
        if baseline.as_ref().map_or(0, Baseline::generation) != generation
            || baseline.as_ref().is_some_and(|b| b.binding() != binding)
        {
            return Err(HistoryError::Integrity("inconsistent baseline generation"));
        }
        Ok(baseline)
    }

    /// Commits intent before any file mutation. Checks that the preview's
    /// complete baseline equals the current reference, not just its generation.
    ///
    /// # Errors
    /// Running owner, stale preview, wrong mode/profile or SQL failure.
    pub fn begin(
        &mut self,
        execution: ExecutionId,
        binding: &StateBinding,
        plan: &ExecutionPlan,
    ) -> Result<RunToken, HistoryError> {
        if plan.mode() != SyncMode::Bidirectional
            || plan.profile() != Some(&binding.profile)
            || plan.source().location() != binding.source
            || plan.destination().location() != binding.destination
            || plan.source().connector() != &binding.source_connector
            || plan.destination().connector() != &binding.destination_connector
            || plan.filter_rules() != &binding.filter
        {
            return Err(HistoryError::Stale);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO profiles(profile,binding) VALUES (?1,?2)",
            params![binding.profile.as_str(), serde_json::to_string(binding)?],
        )?;
        let (identity, generation, json): (String, i64, Option<String>) = tx.query_row(
            "SELECT binding,generation,baseline FROM profiles WHERE profile=?1",
            [binding.profile.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let generation = u64::try_from(generation)
            .map_err(|_| HistoryError::Integrity("negative generation"))?;
        let baseline: Option<Baseline> = json.map(|j| serde_json::from_str(&j)).transpose()?;
        if serde_json::from_str::<StateBinding>(&identity)? != *binding
            || baseline.as_ref() != plan.baseline()
            || baseline.as_ref().map_or(0, Baseline::generation) != generation
        {
            return Err(HistoryError::Stale);
        }
        let running: i64 = tx.query_row(
            "SELECT count(*) FROM executions WHERE profile=?1 AND state='running'",
            [binding.profile.as_str()],
            |r| r.get(0),
        )?;
        if running != 0 {
            return Err(HistoryError::Running);
        }
        // Serialize ownership of exact physical roots across all profiles
        // using this DB. Canonical root identities are supplied by the caller.
        let mut stmt = tx.prepare("SELECT p.binding FROM profiles p JOIN executions e ON e.profile=p.profile WHERE e.state='running'")?;
        let identities = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for other in identities {
            let other: StateBinding = serde_json::from_str(&other?)?;
            if [&binding.source, &binding.destination]
                .iter()
                .any(|root| *root == &other.source || *root == &other.destination)
            {
                return Err(HistoryError::Running);
            }
        }
        drop(stmt);
        tx.execute("INSERT INTO executions(execution,profile,generation,plan,started_at,state) VALUES(?1,?2,?3,?4,?5,'running')",
            params![execution.as_str(),binding.profile.as_str(),sql_generation(generation)?,serde_json::to_string(plan)?,now_json()?])?;
        tx.commit()?;
        Ok(RunToken {
            execution,
            binding: binding.clone(),
            generation,
        })
    }

    /// Atomically records completion and optionally publishes a verified
    /// baseline. Only an unqualified successful execution can advance it.
    /// The application must rescan and establish convergence before calling.
    ///
    /// # Errors
    /// Invalid transition/generation/identity, stale owner, serialization or SQL.
    pub fn finish(
        &mut self,
        token: RunToken,
        status: ExecutionStatus,
        report: serde_json::Value,
        next: Option<&Baseline>,
        retention: RetentionPolicy,
    ) -> Result<(), HistoryError> {
        if matches!(status, ExecutionStatus::Running | ExecutionStatus::Pending) {
            return Err(HistoryError::Integrity("nonterminal completion"));
        }
        if let Some(next) = next
            && (status != ExecutionStatus::Succeeded
                || next.binding() != &token.binding
                || token.generation.checked_add(1) != Some(next.generation()))
        {
            return Err(HistoryError::Stale);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE executions SET state='finished',outcome=?1,report=?2,finished_at=?6 WHERE execution=?3 AND profile=?4 AND generation=?5 AND state='running'",
            params![serde_json::to_string(&status)?,serde_json::to_string(&report)?,token.execution.as_str(),token.binding.profile.as_str(),sql_generation(token.generation)?,now_json()?])?;
        if changed != 1 {
            return Err(HistoryError::Stale);
        }
        if let Some(next) = next {
            let old: Option<String> = tx.query_row(
                "SELECT baseline FROM profiles WHERE profile=?1",
                [token.binding.profile.as_str()],
                |r| r.get(0),
            )?;
            if let Some(old) = old {
                let old: Baseline = serde_json::from_str(&old)?;
                if old
                    .entries()
                    .keys()
                    .any(|p| !next.entries().contains_key(p))
                {
                    return Err(HistoryError::Integrity("baseline path or tombstone lost"));
                }
            }
            let changed = tx.execute("UPDATE profiles SET generation=?1,baseline=?2 WHERE profile=?3 AND generation=?4 AND binding=?5",
                params![sql_generation(next.generation())?,serde_json::to_string(next)?,token.binding.profile.as_str(),sql_generation(token.generation)?,serde_json::to_string(&token.binding)?])?;
            if changed != 1 {
                return Err(HistoryError::Stale);
            }
        }
        let count = i64::try_from(retention.completed_per_profile.get()).unwrap_or(i64::MAX);
        tx.execute("DELETE FROM executions WHERE profile=?1 AND state='finished' AND rowid NOT IN (SELECT rowid FROM executions WHERE profile=?1 AND state='finished' ORDER BY rowid DESC LIMIT ?2)", params![token.binding.profile.as_str(),count])?;
        tx.commit()?;
        Ok(())
    }

    /// Explicit recovery after verifying the old process is stopped. Does not
    /// alter the baseline and does not replay any file operation.
    ///
    /// # Errors
    /// Unknown/nonrunning execution or SQL failure.
    pub fn mark_interrupted(&mut self, execution: &ExecutionId) -> Result<(), HistoryError> {
        if self.connection.execute(
            "UPDATE executions SET state='interrupted',finished_at=?2 WHERE execution=?1 AND state='running'",
            params![execution.as_str(), now_json()?],
        )? != 1
        {
            return Err(HistoryError::Stale);
        }
        Ok(())
    }

    /// Reads retained records oldest first. Invalid plans are reported.
    ///
    /// # Errors
    /// SQL or persisted-format failure; never hides corrupted rows.
    pub fn records(&self, profile: &ProfileId) -> Result<Vec<HistoryRecord>, HistoryError> {
        let mut stmt = self.connection.prepare("SELECT execution,state,outcome,plan,report,started_at,finished_at FROM executions WHERE profile=?1 ORDER BY rowid")?;
        let rows = stmt.query_map([profile.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
            ))
        })?;
        let mut records = Vec::new();
        for row in rows {
            let (id, state, outcome, plan, report, started_at, finished_at) = row?;
            let state = match state.as_str() {
                "running" => RunState::Running,
                "interrupted" => RunState::Interrupted,
                "finished" => RunState::Finished(serde_json::from_str(
                    &outcome.ok_or(HistoryError::Integrity("missing outcome"))?,
                )?),
                _ => return Err(HistoryError::Integrity("invalid run state")),
            };
            records.push(HistoryRecord {
                started_at: serde_json::from_str(&started_at)?,
                finished_at: finished_at.map(|t| serde_json::from_str(&t)).transpose()?,
                execution: ExecutionId::new(id)
                    .map_err(|_| HistoryError::Integrity("invalid execution id"))?,
                state,
                plan: serde_json::from_str(&plan)?,
                report: report.map(|r| serde_json::from_str(&r)).transpose()?,
            });
        }
        Ok(records)
    }

    /// SQLite online backup to a separate, nonexistent file. Never use raw
    /// file copying while a database is open.
    ///
    /// # Errors
    /// Existing target or SQLite backup failure.
    pub fn backup_to(&self, path: &Path) -> Result<(), HistoryError> {
        // Reserve without truncation, including against a racing creator.
        let reservation = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        self.connection
            .backup("main", path, None::<fn(rusqlite::backup::Progress)>)?;
        reservation.sync_all()?;
        Ok(())
    }
}

fn sql_generation(generation: u64) -> Result<i64, HistoryError> {
    i64::try_from(generation).map_err(|_| HistoryError::Integrity("generation overflow"))
}

fn now_json() -> Result<String, HistoryError> {
    let now = Timestamp::now().map_err(|_| HistoryError::Integrity("invalid system clock"))?;
    Ok(serde_json::to_string(&now)?)
}
