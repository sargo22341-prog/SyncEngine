//! Bounded process observation, independent of scheduler receipts and history.
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// Resident lifecycle phase, independent of the outcome of a synchronization.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidentPhase {
    /// No main-loop observation yet.
    #[default]
    Starting,
    /// Ready for the next schedule refresh.
    Idle,
    /// Dispatching due jobs through the application.
    Dispatching,
    /// Automatic jobs suspended by explicit cancellation.
    Paused,
    /// Last dispatch requires examination of scheduler receipts.
    Attention,
    /// Cooperative process stop completed.
    Stopped,
    /// Initialization or persistence failed.
    Error,
}

/// Resident process observation. Availability is verified by the OS file lock,
/// not by a PID or a file left behind after a crash.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResidentStatus {
    /// Process owns this state directory's agent lock right now.
    pub available: bool,
    /// Last recorded phase; stale when `available` is false.
    pub phase: ResidentPhase,
    /// Last main-loop observation, UTC seconds; never a job success indicator.
    pub updated_at: i64,
}
/// Exclusively owns the agent for one canonical state directory, across sessions.
/// Retain the returned handle until exit. No lock file is deleted.
/// # Errors
/// A second instance or an inaccessible state directory.
pub fn hold_agent_presence(state: &Path) -> std::io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("agent.lock"))?;
    file.try_lock().map_err(std::io::Error::from)?;
    Ok(file)
}
/// Publishes one bounded observation by temporary-file replacement.
/// The caller must hold the agent lock. Errors must be reported.
/// # Errors
/// Clock, serialization or storage failure.
pub fn publish_resident_status(state: &Path, phase: ResidentPhase) -> Result<(), super::AppError> {
    let status = ResidentStatus {
        available: true,
        phase,
        updated_at: sync_core::Timestamp::now().map_err(super::io)?.secs(),
    };
    let temp = state.join("agent-status.json.tmp");
    std::fs::write(&temp, serde_json::to_vec(&status).map_err(super::io)?).map_err(super::io)?;
    std::fs::rename(temp, state.join("agent-status.json")).map_err(super::io)
}
/// Reads a process observation and checks the live OS lock. A stale file never
/// establishes process availability. History remains the execution truth.
/// # Errors
/// Unreadable or malformed observation or lock access failure.
pub fn resident_status(state: &Path) -> Result<ResidentStatus, super::AppError> {
    let mut status = match std::fs::read(state.join("agent-status.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(super::io)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ResidentStatus::default(),
        Err(e) => return Err(super::io(e)),
    };
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("agent.lock"))
        .map_err(super::io)?;
    status.available = match file.try_lock() {
        Ok(()) => false,
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(e)) => return Err(super::io(e)),
    };
    Ok(status)
}

/// Clears an explicit cooperative resident stop request. Does not start a
/// process; the presentation client must ask the native supervisor to start.
/// # Errors
/// Stop marker cannot be removed.
pub fn clear_agent_stop(state: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(state.join("agent-stop.request")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
/// Whether an explicit resident stop is pending (unreadable means stopped).
#[must_use]
pub fn agent_stop_pending(state: &Path) -> bool {
    !matches!(state.join("agent-stop.request").try_exists(), Ok(false))
}
