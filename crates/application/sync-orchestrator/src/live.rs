//! Cross-process presentation of scheduled jobs, through the state directory:
//! the agent publishes its live snapshot so that a window can follow a run it
//! did not start, a window marks its presence, and a stop request can be left
//! for the agent.
//!
//! Presentation only: one small JSON file replaced as a whole (temporary file
//! then rename), a few times per second at most. It is never a source of
//! truth; durable outcomes stay in history and scheduler receipts. A file left
//! by a killed agent is recognized by its age (see [`LiveJob::is_live`]).
use super::{AppError, Application, JobPhase, JobSnapshot, error, io};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::path::Path;
use sync_core::Timestamp;

/// Version of the [`LiveJob`] file format.
pub const LIVE_FORMAT: u32 = 1;
const LIVE_FILE: &str = "agent-live.json";
const PRESENCE_FILE: &str = "window.lock";
/// Persistent stop marker: cancels the running agent job and blocks the next
/// ones until cleared (same file as `syncengine-agent cancel`).
const CANCEL_FILE: &str = "cancel.request";

/// Latest observation published by the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveJob {
    /// File format version ([`LIVE_FORMAT`]).
    pub format: u32,
    /// Publication time, Unix seconds (UTC).
    pub updated_at: i64,
    /// Bounded snapshot, same model as the in-process one.
    pub snapshot: JobSnapshot,
}

impl LiveJob {
    /// True while the job runs and was refreshed at most `max_age` seconds
    /// before `now`. An older active file means the agent stopped abruptly:
    /// the scheduler's interrupted claim then tells the truth.
    #[must_use]
    pub fn is_live(&self, now: i64, max_age: i64) -> bool {
        self.snapshot.active && now.abs_diff(self.updated_at) <= max_age.unsigned_abs()
    }
}

impl Application {
    /// Publishes the current snapshot for other processes (called by the
    /// agent). Does nothing while this instance has not started a job, so an
    /// idle or refused agent never overwrites the observation of the running one.
    /// Returns whether a snapshot was written.
    /// # Errors
    /// Clock, serialization or file failure; the job itself is unaffected.
    pub fn publish_live(&self) -> Result<bool, AppError> {
        let snapshot = self.snapshot()?;
        if snapshot.phase == JobPhase::Idle {
            return Ok(false);
        }
        let live = LiveJob {
            format: LIVE_FORMAT,
            updated_at: Timestamp::now().map_err(io)?.secs(),
            snapshot,
        };
        let temp = self.state.join(format!("{LIVE_FILE}.tmp"));
        std::fs::write(&temp, serde_json::to_vec(&live).map_err(io)?).map_err(io)?;
        std::fs::rename(&temp, self.state.join(LIVE_FILE)).map_err(io)?;
        Ok(true)
    }

    /// Latest observation published by an agent, `None` when there is none.
    /// Check [`LiveJob::is_live`] before presenting it as running.
    /// # Errors
    /// Unreadable file (for instance while it is being replaced), unknown
    /// format or malformed content.
    pub fn agent_live(&self) -> Result<Option<LiveJob>, AppError> {
        let bytes = match std::fs::read(self.state.join(LIVE_FILE)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let live: LiveJob = serde_json::from_slice(&bytes).map_err(|e| error("format", e))?;
        if live.format != LIVE_FORMAT {
            return Err(error("format", "format de progression inconnu"));
        }
        Ok(Some(live))
    }

    /// Leaves a persistent stop request for the agent: the running scheduled
    /// job is cancelled cooperatively and no further one starts until
    /// [`Application::agent_cancel_clear`].
    /// # Errors
    /// File creation failure.
    pub fn agent_cancel_request(&self) -> Result<(), AppError> {
        request_agent_cancel(&self.state).map_err(io)
    }

    /// Removes the stop request; absent already is not an error.
    /// # Errors
    /// File removal failure.
    pub fn agent_cancel_clear(&self) -> Result<(), AppError> {
        clear_agent_cancel(&self.state).map_err(io)
    }

    /// Whether a stop request currently blocks scheduled jobs. An unreadable
    /// state counts as pending, like the agent does.
    #[must_use]
    pub fn agent_cancel_pending(&self) -> bool {
        !matches!(self.state.join(CANCEL_FILE).try_exists(), Ok(false))
    }
}

/// Writes the stop marker durably (used by the agent CLI and the window).
/// # Errors
/// File creation or synchronization failure.
pub fn request_agent_cancel(state: &Path) -> std::io::Result<()> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join(CANCEL_FILE))?
        .sync_all()
}

/// Removes the stop marker; absent already is not an error.
/// # Errors
/// File removal failure.
pub fn clear_agent_cancel(state: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(state.join(CANCEL_FILE)) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Path of the stop marker, watched by the agent while a job runs.
#[must_use]
pub fn agent_cancel_marker(state: &Path) -> std::path::PathBuf {
    state.join(CANCEL_FILE)
}

/// Marks an open window for this state directory while the returned file
/// lives (operating-system lock, released even if the process is killed).
/// `None` when another window already holds it.
/// # Errors
/// File creation or locking failure.
pub fn hold_desktop_presence(state: &Path) -> std::io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join(PRESENCE_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// Whether a window currently holds the presence lock of this state directory.
/// # Errors
/// File creation or locking failure.
pub fn desktop_present(state: &Path) -> std::io::Result<bool> {
    // Holding the lock for an instant is harmless: it is released on drop.
    Ok(hold_desktop_presence(state)?.is_none())
}
