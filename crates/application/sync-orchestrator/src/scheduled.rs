//! Non-interactive authorization and conservative cross-state exclusion.
use super::{AppError, Application, error, io};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use sync_core::CancellationToken;
use sync_profiles::Profile;
use sync_scheduler::{Completion, JobStatus, Schedule, ScheduledApplication};

pub(super) fn default_lock_root() -> Result<PathBuf, AppError> {
    let root = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
    }
    .ok_or_else(|| {
        error(
            "unavailable",
            "répertoire de verrous utilisateur indisponible",
        )
    })?;
    if !root.is_absolute() {
        return Err(error(
            "invalid_input",
            "répertoire de verrous absolu requis",
        ));
    }
    Ok(root.join("SyncEngine/job-locks"))
}
pub(super) fn job_lock(root: &Path) -> Result<File, AppError> {
    // Never truncate/unlink: deleting a locked file would permit a second owner.
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("execution.lock"))
        .map_err(io)?;
    match file.try_lock() {
        Ok(()) => (),
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(error(
                "busy",
                "une autre exécution utilise les destinations",
            ));
        }
        Err(std::fs::TryLockError::Error(cause)) => return Err(io(cause)),
    }
    Ok(file)
}
impl Application {
    /// Acknowledges stopped executions AFTER manual inspection of journals and
    /// synchronized roots. Keeps old baselines and records interruption as failure.
    /// This never replays a plan or removes temporary/versioned content.
    /// # Errors
    /// Explicit diagnosis missing, busy owner, or persistence failure. The two
    /// databases are updated sequentially; retry diagnosis if either write fails.
    pub fn acknowledge_interrupted(
        &self,
        id: &sync_core::ProfileId,
        diagnosed: bool,
    ) -> Result<(), AppError> {
        let _gate = self.gate()?;
        let _owner = job_lock(&self.lock_root)?;
        if !diagnosed {
            return Err(error(
                "confirmation_required",
                "diagnostic explicite requis",
            ));
        }
        let mut history =
            sync_history::History::open(self.state.join("state.sqlite")).map_err(io)?;
        for record in history.records(id).map_err(io)? {
            if record.state == sync_history::RunState::Running {
                history.mark_interrupted(&record.execution).map_err(io)?;
            }
        }
        super::runs::acknowledge(&self.state, id)?;
        *super::lock(&self.cache)? = None;
        Ok(())
    }

    /// Builds a fresh plan and executes exactly that plan under explicit unattended
    /// authorization. Full profile equality is checked twice. Never prompts the UI.
    /// An unfinished application attempt blocks execution until manual diagnosis.
    /// # Errors
    /// Busy, changed profile, interruption, unapproved deletion, cancellation or
    /// ordinary preview/execution errors. Errors may describe partial work.
    pub fn execute_scheduled(
        &self,
        approved: &Profile,
        allow_deletions: bool,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, AppError> {
        let result = (|| {
            let _gate = self.gate()?;
            if cancel.is_cancelled() {
                return Err(error("cancelled", "job annulé"));
            }
            if self.profile(&approved.id)? != *approved {
                return Err(error(
                    "stale",
                    "profil modifié : réautoriser la programmation",
                ));
            }
            if self
                .history(approved.id.clone())?
                .iter()
                .any(|r| r.state == "running")
            {
                return Err(error(
                    "interrupted",
                    "intention inachevée : diagnostic requis",
                ));
            }
            let view = self.preview_locked(approved.id.clone(), Some(cancel))?;
            let id = view["plan"]["id"]
                .as_str()
                .ok_or_else(|| error("internal", "identifiant de plan absent"))?
                .to_owned();
            self.execute_locked(approved.id.clone(), id, true, allow_deletions, Some(cancel))
        })();
        result.map_err(|mut e| {
            e.profile = Some(approved.id.clone());
            e
        })
    }
}
impl ScheduledApplication for Application {
    fn run(&self, schedule: &Schedule, cancel: &CancellationToken) -> Completion {
        match self.execute_scheduled(&schedule.approved, schedule.allow_deletions, cancel) {
            Err(e) => Completion {
                status: if e.code == "busy" {
                    JobStatus::Deferred
                } else {
                    JobStatus::Failed
                },
                code: e.code,
            },
            Ok(report) => {
                let execution = report
                    .get("execution")
                    .filter(|v| v.is_object())
                    .unwrap_or(&report);
                let succeeded = execution["status"] == "succeeded"
                    && (report.get("baseline_updated").is_none()
                        || report["baseline_updated"] == true);
                Completion {
                    status: if succeeded {
                        JobStatus::Succeeded
                    } else {
                        JobStatus::Attention
                    },
                    code: if succeeded {
                        "succeeded"
                    } else {
                        "report_requires_attention"
                    }
                    .into(),
                }
            }
        }
    }
}
