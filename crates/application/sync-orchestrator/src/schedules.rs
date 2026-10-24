//! Configuration only; dispatch remains in the independent native agent.
use crate::{AppError, Application, error, io};
use sync_core::{ProfileId, Timestamp};
use sync_scheduler::{MissedJobs, Schedule, ScheduleState, Scheduler, Trigger};

impl Application {
    /// Lists actual persistent authorizations and interrupted claims.
    /// # Errors
    /// Busy or scheduler database/configuration failure.
    pub fn schedules(&self) -> Result<Vec<ScheduleState>, AppError> {
        let _gate = self.gate()?;
        Scheduler::open(self.state.join("scheduler.sqlite"))
            .map_err(io)?
            .list()
            .map_err(io)
    }

    /// Authorizes the current complete profile, never a profile supplied by IPC.
    /// An edit invalidates authorization; native task installation is separate.
    /// `show_window` only asks the agent to open the desktop window to follow
    /// each run (Windows); it is not a consent and changes nothing else.
    /// # Errors
    /// Missing consent, unsupported trigger, busy, invalid or interrupted state.
    pub fn schedule_save(
        &self,
        id: ProfileId,
        trigger: Trigger,
        missed: MissedJobs,
        approve_writes: bool,
        approve_deletions: bool,
        show_window: bool,
    ) -> Result<(), AppError> {
        let _gate = self.gate()?;
        if !approve_writes {
            return Err(error(
                "confirmation_required",
                "autorisation des écritures sans fenêtre requise",
            ));
        }
        if matches!(trigger, Trigger::Logon | Trigger::LogonDelayed { .. }) && !cfg!(windows)
            || matches!(trigger, Trigger::UserManagerStart) && !cfg!(target_os = "linux")
        {
            return Err(error(
                "not_supported",
                "déclencheur indisponible sur cet OS",
            ));
        }
        let profile = self.profile(&id)?;
        self.roots(&profile)?;
        if approve_deletions && !profile.allow_delete {
            return Err(error(
                "invalid_input",
                "le profil interdit les suppressions",
            ));
        }
        let schedule = Schedule {
            approved: profile,
            trigger,
            missed,
            created_at: Timestamp::now().map_err(io)?.secs(),
            enabled: true,
            allow_deletions: approve_deletions,
            show_window,
        };
        Scheduler::open(self.state.join("scheduler.sqlite"))
            .map_err(io)?
            .save(&schedule)
            .map_err(io)
    }

    /// Removes authorization only; refuses a running/interrupted claim.
    /// # Errors
    /// Busy, interrupted claim or persistence failure.
    pub fn schedule_remove(&self, id: ProfileId) -> Result<(), AppError> {
        let _gate = self.gate()?;
        Scheduler::open(self.state.join("scheduler.sqlite"))
            .map_err(io)?
            .remove(&id)
            .map_err(io)
    }

    /// State directory to use when exporting/installing native agent tasks.
    /// No credentials are returned. Native installation is never implied by save.
    #[must_use]
    pub fn state_directory(&self) -> &std::path::Path {
        &self.state
    }
}
