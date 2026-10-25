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
    /// Bounded, redacted process failure, independent of job receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
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
    publish(state, phase, None)
}
/// Publishes a process failure with a redacted diagnostic of at most 2048 characters.
/// The caller must hold the agent lock. This observation never authorizes a retry.
/// # Errors
/// Clock, serialization or storage failure.
pub fn publish_resident_failure(state: &Path, diagnostic: &str) -> Result<(), super::AppError> {
    let diagnostic = sync_errors::redact_secrets(diagnostic)
        .chars()
        .take(2048)
        .collect();
    publish(state, ResidentPhase::Error, Some(diagnostic))
}
fn publish(
    state: &Path,
    phase: ResidentPhase,
    diagnostic: Option<String>,
) -> Result<(), super::AppError> {
    let status = ResidentStatus {
        available: true,
        phase,
        updated_at: sync_core::Timestamp::now().map_err(super::io)?.secs(),
        diagnostic,
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
/// Requests cooperative cancellation and persistent resident shutdown.
/// # Errors
/// Stop marker cannot be written or synchronized.
pub fn request_agent_stop(state: &Path) -> std::io::Result<()> {
    File::create(state.join("agent-stop.request"))?.sync_all()
}
/// Whether an explicit resident stop is pending (unreadable means stopped).
#[must_use]
pub fn agent_stop_pending(state: &Path) -> bool {
    !matches!(state.join("agent-stop.request").try_exists(), Ok(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_is_bounded_redacted_and_cleared_by_a_new_observation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let _owner = hold_agent_presence(temp.path())?;
        let message = format!(
            "application_open: password=private-test-token {}",
            "é".repeat(3000)
        );
        publish_resident_failure(temp.path(), &message)?;
        let status = resident_status(temp.path())?;
        assert!(status.available);
        assert_eq!(status.phase, ResidentPhase::Error);
        let diagnostic = status.diagnostic.ok_or("missing diagnostic")?;
        assert_eq!(diagnostic.chars().count(), 2048);
        assert!(!diagnostic.contains("private-test-token"));
        publish_resident_status(temp.path(), ResidentPhase::Idle)?;
        assert!(resident_status(temp.path())?.diagnostic.is_none());
        Ok(())
    }

    #[test]
    fn previous_observations_remain_readable() -> Result<(), serde_json::Error> {
        let status: ResidentStatus =
            serde_json::from_str(r#"{"available":true,"phase":"error","updated_at":1}"#)?;
        assert!(status.diagnostic.is_none());
        Ok(())
    }
}
