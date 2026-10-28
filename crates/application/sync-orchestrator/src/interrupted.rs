//! Interrupted executions: readable diagnosis from their journal, explicit
//! acknowledgement, and resumption by a fresh preview.
//!
//! An attempt still marked running while this facade owns the state (no
//! command can execute without that ownership) was stopped by a crash, a
//! power cut or a session end. Its journal (`run-….jsonl`) is read line by
//! line to say what was done and which files were started but never
//! finished; it is never replayed. Acknowledging the interruption removes the
//! journal afterwards. Resuming additionally removes the temporary files of
//! the interrupted copies that carry the local staging name, then the caller
//! starts a new full preview: files already copied appear identical and are
//! skipped.
use super::{AppError, Application, endpoints, error, io, runs, scheduled};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use sync_core::{CancellationToken, EntryKind, ExecutionId, ProfileId, ResourcePath};
use sync_journal::{InFlight, JournalRecord, JournalTotals, OutcomeCode, Recovery};
use sync_plan::{Action, PlanSide};

/// Finished operations kept by a detail view, newest last.
pub const INTERRUPTION_RECENT: usize = 100;

/// One interrupted execution, as shown when the application opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Interruption {
    /// Profile of the execution.
    pub profile: ProfileId,
    /// Execution identity (`run-…`).
    pub execution: String,
    /// Start time, Unix seconds (UTC), when recorded.
    pub started: Option<i64>,
    /// Mutations planned (copies, replacements, deletions).
    pub planned: u64,
    /// Operations with a recorded outcome.
    pub finished: u64,
    /// Operations started but never finished: their state is unknown.
    pub in_flight: u64,
    /// Why the journal could not be read (missing, corrupt): counts are
    /// then unknown, never presented as zero work.
    pub journal_error: Option<String>,
}

/// A file started but never finished at the time of the stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterruptedFile {
    /// Logical path.
    pub path: String,
    /// Planned action.
    pub action: Action,
    /// Receiving side.
    pub target: PlanSide,
    /// Temporary file that may remain on the receiving side.
    pub staging: Option<String>,
    /// Copy kept in the trash before the interruption.
    pub version: Option<String>,
}

/// An operation with a recorded outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FinishedOperation {
    /// Logical path.
    pub path: String,
    /// Planned action.
    pub action: Action,
    /// Recorded outcome.
    pub outcome: OutcomeCode,
    /// Validated bytes.
    pub bytes: u64,
    /// Redacted detail of a refusal or failure.
    pub detail: Option<String>,
}

/// Readable view of an interrupted execution, built from its journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterruptionDetail {
    /// Summary.
    pub interruption: Interruption,
    /// Source label recorded at start (no secret).
    pub source: Option<String>,
    /// Destination label recorded at start (no secret).
    pub destination: Option<String>,
    /// Outcomes counted from the journal.
    pub totals: JournalTotals,
    /// Files started but never finished, first.
    pub in_flight: Vec<InterruptedFile>,
    /// Last finished operations (at most [`INTERRUPTION_RECENT`]), newest last.
    pub recent: Vec<FinishedOperation>,
    /// Refused or failed operations (the first [`INTERRUPTION_RECENT`]).
    pub problems: Vec<FinishedOperation>,
    /// Time of the last journal record, Unix seconds: about when it stopped.
    pub last_record: Option<i64>,
    /// The last line was cut by the stop and ignored.
    pub truncated_tail: bool,
}

/// What resuming did before the new preview.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Resumed {
    /// Temporary files removed (receiving side, logical path).
    pub removed: Vec<String>,
    /// Temporary files left in place, with the reason: to check by hand.
    pub leftovers: Vec<String>,
    /// Clean-up that could not be completed (journal removal).
    pub warnings: Vec<String>,
}

fn journal_path(state: &Path, execution: &str) -> Result<std::path::PathBuf, AppError> {
    // ExecutionId excludes separators: one file of the state directory.
    let execution = ExecutionId::new(execution).map_err(|e| error("integrity", e))?;
    Ok(state.join(format!("{execution}.jsonl")))
}

/// Streams the journal of one attempt into its detail view.
fn read(state: &Path, attempt: &runs::Running) -> Result<InterruptionDetail, AppError> {
    let profile = ProfileId::new(attempt.profile.as_str()).map_err(|e| error("integrity", e))?;
    let mut detail = InterruptionDetail {
        interruption: Interruption {
            profile,
            execution: attempt.execution.clone(),
            started: attempt.started,
            planned: 0,
            finished: 0,
            in_flight: 0,
            journal_error: None,
        },
        source: None,
        destination: None,
        totals: JournalTotals::default(),
        in_flight: Vec::new(),
        recent: Vec::new(),
        problems: Vec::new(),
        last_record: None,
        truncated_tail: false,
    };
    let path = journal_path(state, &attempt.execution)?;
    let mut recovery = Recovery::new();
    let mut recent = VecDeque::with_capacity(INTERRUPTION_RECENT);
    let visited = sync_journal::visit_journal(&path, |line| {
        detail.last_record = Some(line.at().secs());
        let record = line.record();
        recovery.push(record);
        match record {
            JournalRecord::ExecutionStarted {
                source,
                destination,
                ..
            } => {
                detail.source = Some(source.clone());
                detail.destination = Some(destination.clone());
            }
            JournalRecord::OperationFinished {
                path,
                action,
                outcome,
                bytes,
                detail: text,
                ..
            } => {
                let finished = FinishedOperation {
                    path: path.as_str().to_owned(),
                    action: *action,
                    outcome: *outcome,
                    bytes: *bytes,
                    detail: text.clone(),
                };
                if matches!(outcome, OutcomeCode::Failed | OutcomeCode::Refused)
                    && detail.problems.len() < INTERRUPTION_RECENT
                {
                    detail.problems.push(finished.clone());
                }
                if recent.len() == INTERRUPTION_RECENT {
                    recent.pop_front();
                }
                recent.push_back(finished);
            }
            _ => {}
        }
    });
    match visited {
        Ok(truncated) => {
            let report = recovery.finish(truncated);
            let totals = report.totals;
            detail.interruption.planned = report.mutations_planned;
            detail.interruption.finished = totals.done
                + totals.no_action
                + totals.refused
                + totals.failed
                + totals.cancelled
                + totals.not_started;
            detail.interruption.in_flight = report.in_flight.len() as u64;
            detail.totals = totals;
            detail.truncated_tail = report.truncated_tail;
            detail.in_flight = report.in_flight.iter().map(file).collect();
            detail.recent = recent.into();
        }
        Err(e) => detail.interruption.journal_error = Some(e.to_string()),
    }
    Ok(detail)
}

fn file(op: &InFlight) -> InterruptedFile {
    InterruptedFile {
        path: op.path.as_str().to_owned(),
        action: op.action,
        target: op.target,
        staging: op.staging.as_ref().map(|s| s.as_str().to_owned()),
        version: op.version.as_ref().map(|s| s.as_str().to_owned()),
    }
}

/// Removes the journals of acknowledged attempts; failures are returned.
pub(super) fn discard_journals(state: &Path, executions: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    for execution in executions {
        let removed =
            journal_path(state, execution).and_then(|path| match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(error("journal_retention", e)),
            });
        if let Err(e) = removed {
            warnings.push(format!(
                "journal {execution} non supprimé (nouvel essai à la prochaine ouverture) : {}",
                e.message
            ));
        }
    }
    warnings
}

impl Application {
    /// Interrupted executions of every profile, oldest first. Taking the
    /// command ownership guarantees that none of them is still running.
    /// # Errors
    /// `busy` while a command (window, agent) owns the state; persistence failure.
    /// An unreadable journal is reported in [`Interruption::journal_error`].
    pub fn interruptions(&self) -> Result<Vec<Interruption>, AppError> {
        let _gate = self.gate()?;
        runs::running(&self.state, None)?
            .iter()
            .map(|attempt| read(&self.state, attempt).map(|d| d.interruption))
            .collect()
    }

    /// Readable view of one interrupted execution of `id`.
    /// # Errors
    /// `busy`, `not_found` when it is not (or no longer) interrupted, persistence failure.
    pub fn interruption_detail(
        &self,
        id: &ProfileId,
        execution: &str,
    ) -> Result<InterruptionDetail, AppError> {
        let _gate = self.gate()?;
        let attempt = runs::running(&self.state, Some(id))?
            .into_iter()
            .find(|a| a.execution == execution)
            .ok_or_else(|| error("not_found", "exécution interrompue introuvable"))?;
        read(&self.state, &attempt)
    }

    /// Acknowledges the interruptions of `id` (like
    /// [`Self::acknowledge_interrupted`]), then removes the temporary files
    /// left by its interrupted copies, and finally their journals. Only
    /// files whose name is the local staging name (`.syncengine-….partial`)
    /// are removed, through the profile connector, never following a link;
    /// others are listed in [`Resumed::leftovers`]. Nothing is replayed: the
    /// caller starts a new preview of the profile.
    /// # Errors
    /// Busy, unknown profile or persistence failure before acknowledgement.
    pub fn resume_interrupted(&self, id: &ProfileId) -> Result<Resumed, AppError> {
        let _gate = self.gate()?;
        let _owner = scheduled::job_lock(&self.lock_root)?;
        let profile = self.profile(id)?;
        let mut resumed = Resumed::default();
        // Staging paths come from the journals, read before they go.
        let mut stagings: Vec<(PlanSide, ResourcePath)> = Vec::new();
        for attempt in runs::running(&self.state, Some(id))? {
            let path = journal_path(&self.state, &attempt.execution)?;
            let mut recovery = Recovery::new();
            match sync_journal::visit_journal(&path, |line| recovery.push(line.record())) {
                Ok(truncated) => stagings.extend(
                    recovery
                        .finish(truncated)
                        .in_flight
                        .into_iter()
                        .filter_map(|op| op.staging.map(|s| (op.target, s))),
                ),
                Err(e) => resumed.warnings.push(format!(
                    "journal {} illisible : fichiers temporaires non recherchés ({e})",
                    attempt.execution
                )),
            }
        }
        let executions = self.acknowledge_locked(id)?;
        if !stagings.is_empty() {
            self.remove_stagings(&profile, &stagings, &mut resumed);
        }
        resumed
            .warnings
            .extend(discard_journals(&self.state, &executions));
        Ok(resumed)
    }

    fn remove_stagings(
        &self,
        profile: &sync_profiles::Profile,
        stagings: &[(PlanSide, ResourcePath)],
        resumed: &mut Resumed,
    ) {
        let cancel = CancellationToken::new();
        let roots = self.roots(profile);
        for (side, staging) in stagings {
            let label = staging.as_str().to_owned();
            let name = staging.segments().last().unwrap_or_default();
            if !connector_local::is_staging_name(name) {
                resumed
                    .leftovers
                    .push(format!("{label} : dépôt distant, à vérifier"));
                continue;
            }
            let removed = roots.as_ref().map_err(Clone::clone).and_then(|(s, d)| {
                let (root, remote) = match side {
                    PlanSide::Source => (s, profile.remote_source.as_ref()),
                    PlanSide::Destination => (d, profile.remote_destination.as_ref()),
                };
                let connector = endpoints::build(Path::new(root), remote, true, &cancel)?;
                match connector.stat(staging, &cancel) {
                    Ok(entry) if entry.kind() == EntryKind::File => connector
                        .delete(staging, EntryKind::File, &cancel)
                        .map(|()| true)
                        .map_err(io),
                    Ok(_) => Err(error("conflict", "ce n’est pas un fichier")),
                    Err(e) if e.kind() == connector_api::StorageErrorKind::NotFound => Ok(false),
                    Err(e) => Err(io(e)),
                }
            });
            match removed {
                Ok(true) => resumed.removed.push(label),
                Ok(false) => {}
                Err(e) => resumed.leftovers.push(format!("{label} : {}", e.message)),
            }
        }
    }
}
