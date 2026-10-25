//! Durable scheduler: claims an occurrence before calling the application API.
//! UTC seconds, sequential bounded dispatch, no file-transfer or GUI dependency.
mod store;
use platform_api::SessionEvent;
use serde::{Deserialize, Serialize};
pub use store::{ScheduleState, Scheduler};
use sync_core::CancellationToken;
use sync_profiles::Profile;

/// Scheduling/persistence failure; never a successful empty schedule.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// Configuration, schema or state invariant failure.
    #[error("scheduler: {0}")]
    Invalid(&'static str),
    /// SQLite failure, including disk full and corruption.
    #[error("scheduler persistence: {0}")]
    Sql(#[from] rusqlite::Error),
    /// Stored configuration/receipt is malformed.
    #[error("scheduler format: {0}")]
    Format(#[from] serde_json::Error),
}
/// Native event delivered by a trusted OS installation, not inferred from time.
#[derive(Debug, Clone, Copy)]
pub enum Wake {
    /// Regular poll; also checks missed timed occurrences.
    Clock,
    /// Explicit session activation. Repeated events in one minute are coalesced.
    Session(SessionEvent),
}
/// Supported triggers. All wall-clock schedules are UTC, with no DST ambiguity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// Fixed phase anchored at creation; period in 60..=31536000 seconds.
    Interval {
        /// Seconds between occurrences, independent of execution duration.
        seconds: u32,
    },
    /// One occurrence per UTC day.
    DailyUtc {
        /// Hour 0..23.
        hour: u8,
        /// Minute 0..59.
        minute: u8,
    },
    /// Windows logon, supported only by the Windows adapter.
    Logon,
    /// Windows logon followed by a durable delay; requires clock polling too.
    LogonDelayed {
        /// Delay in 1..=86400 seconds, dispatched on the next eligible poll.
        seconds: u32,
    },
    /// Linux user manager start, not each desktop login.
    UserManagerStart,
}
/// What happens to timed occurrences while the machine/agent was unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedJobs {
    /// Run only the newest due occurrence; never replay a backlog.
    RunOnce,
    /// Ignore occurrences more than 59 seconds late.
    Skip,
}
/// Persistent explicit authorization, bound to the complete approved profile.
/// Editing the profile invalidates authorization at the application boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    /// Exact approved configuration, including roots, filters and deletion policy.
    pub approved: Profile,
    /// Trigger interpreted using UTC seconds.
    pub trigger: Trigger,
    /// First eligible instant and fixed interval anchor (Unix seconds).
    pub created_at: i64,
    /// Explicit enable/disable, preserving receipts.
    pub enabled: bool,
    /// Explicit unattended destructive authorization, separate from profile policy.
    pub allow_deletions: bool,
    /// Missed occurrence policy.
    pub missed: MissedJobs,
    /// Presentation only: the agent opens the desktop window so that each run
    /// can be followed live. Never widens authorization nor changes execution.
    /// Absent from configurations written before store v3 (read as `false`).
    #[serde(default)]
    pub show_window: bool,
}
impl Schedule {
    /// Validates configuration; no root access or mutation.
    /// # Errors
    /// Invalid profile, time, interval or daily fields.
    pub fn validate(&self) -> Result<(), SchedulerError> {
        self.approved
            .validate()
            .map_err(|_| SchedulerError::Invalid("invalid approved profile"))?;
        if self.created_at < 0 || self.created_at > i64::MAX - 86_400 {
            return Err(SchedulerError::Invalid("creation time out of range"));
        }
        match self.trigger {
            Trigger::Interval { seconds } if !(60..=31_536_000).contains(&seconds) => {
                Err(SchedulerError::Invalid("interval out of range"))
            }
            Trigger::DailyUtc { hour, minute } if hour > 23 || minute > 59 => {
                Err(SchedulerError::Invalid("daily UTC time out of range"))
            }
            Trigger::LogonDelayed { seconds } if !(1..=86_400).contains(&seconds) => {
                Err(SchedulerError::Invalid("logon delay out of range"))
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn slot(&self, now: i64, wake: Wake) -> Option<i64> {
        if !self.enabled || now < self.created_at {
            return None;
        }
        let slot = match (self.trigger.clone(), wake) {
            (Trigger::Interval { seconds }, _) => {
                self.created_at + (now - self.created_at) / i64::from(seconds) * i64::from(seconds)
            }
            (Trigger::DailyUtc { hour, minute }, _) => {
                let today = now / 86_400 * 86_400;
                let candidate = today + i64::from(hour) * 3600 + i64::from(minute) * 60;
                if candidate > now {
                    candidate - 86_400
                } else {
                    candidate
                }
            }
            (Trigger::Logon, Wake::Session(SessionEvent::Logon))
            | (Trigger::UserManagerStart, Wake::Session(SessionEvent::UserManagerStart)) => {
                (now / 60 * 60).max(self.created_at)
            }
            _ => return None,
        };
        if slot < self.created_at || (self.missed == MissedJobs::Skip && now - slot >= 60) {
            None
        } else {
            Some(slot)
        }
    }
}
/// Actual application outcome, never inferred from an `Ok` transport result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Every operation succeeded (and baseline verified when applicable).
    Succeeded,
    /// Warnings, skipped operations, unresolved conflict or cancellation.
    Attention,
    /// Application failed; partial work may exist.
    Failed,
    /// No mutation began because another owner is busy; same slot may retry.
    Deferred,
}
/// Bounded scheduler receipt; detailed plan/report remains in application history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    /// Actual outcome.
    pub status: JobStatus,
    /// Redacted diagnostic code; no raw paths, credentials or log streams.
    pub code: String,
}
/// One contextual dispatch result. Application reports retain execution IDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchReceipt {
    /// Profile whose occurrence was processed or found unfinished.
    pub profile: sync_core::ProfileId,
    /// UTC occurrence identity; separate from actual application execution time.
    pub occurrence: i64,
    /// Outcome flattened in command JSON, without losing its typed Rust form.
    #[serde(flatten)]
    pub completion: Completion,
}
/// Presentation identity of a due profile; never contains endpoints or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedProfile {
    /// Stable profile identity.
    pub id: sync_core::ProfileId,
    /// User-facing profile name.
    pub name: String,
}

/// Application port. Repeat authorization/root validation, preserve partial work,
/// and honor cancellation cooperatively. Queue observations never authorize work.
pub trait ScheduledApplication {
    /// Executes the approved profile using the ordinary application engines.
    /// Business failures are explicit completions; a process crash leaves a claim.
    fn run(&self, schedule: &Schedule, cancel: &CancellationToken) -> Completion;
    /// Observes the current claim and remaining due profiles in execution order.
    /// Presentation only: observers must not use this to authorize mutations.
    /// Jobs remain sequential and each claim rechecks its persisted configuration.
    fn queue_changed(&self, _current: Option<&QueuedProfile>, _pending: &[QueuedProfile]) {}
}
