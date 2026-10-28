//! Non-interactive authorization and conservative cross-state exclusion.
use super::{AppError, Application, error, io};
use std::fs::File;
use std::path::{Path, PathBuf};
use sync_core::CancellationToken;
use sync_profiles::Profile;
use sync_scheduler::{Completion, JobStatus, Schedule, ScheduledApplication};

pub(super) fn default_lock_root() -> Result<PathBuf, AppError> {
    sync_executor::default_lock_root().map_err(io)
}
pub(super) fn job_lock(root: &Path) -> Result<File, AppError> {
    sync_executor::execution_lock(root).map_err(|cause| {
        if cause.kind() == std::io::ErrorKind::WouldBlock {
            error("busy", cause)
        } else {
            io(cause)
        }
    })
}
impl Application {
    /// Acknowledges stopped executions AFTER manual inspection of journals and
    /// synchronized roots. Keeps old baselines and records interruption as failure.
    /// Also acknowledges the scheduler claim when present, without replaying it.
    /// This never replays a plan or removes temporary/versioned content.
    /// # Errors
    /// Explicit diagnosis missing, busy owner, or persistence failure. The stores
    /// are updated sequentially; retry diagnosis if any write fails.
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
        let scheduler_path = self.state.join("scheduler.sqlite");
        if scheduler_path.try_exists().map_err(io)? {
            let scheduler = sync_scheduler::Scheduler::open(scheduler_path).map_err(io)?;
            if scheduler
                .list()
                .map_err(io)?
                .iter()
                .any(|row| &row.schedule.approved.id == id && row.running_slot.is_some())
            {
                scheduler.acknowledge(id).map_err(io)?;
            }
        }
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
            let view = self.preview_locked(
                approved.id.clone(),
                Some(cancel),
                true,
                crate::DeletionCeiling::Guarded,
            )?;
            let id = view["plan"]["id"]
                .as_str()
                .ok_or_else(|| error("internal", "identifiant de plan absent"))?
                .to_owned();
            // Never a permanent loss without a person: an overflowing trash
            // refuses the run (`trash_insufficient`), to be launched manually.
            self.execute_locked(
                approved.id.clone(),
                id,
                crate::Consent {
                    writes: true,
                    deletions: allow_deletions,
                    permanent_loss: false,
                },
                Some(cancel),
            )
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
