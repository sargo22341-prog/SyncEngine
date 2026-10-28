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

/// Version of the [`LiveJob`] file format written by this version (2 adds
/// the files in flight, [`JobSnapshot::transfers`]).
pub const LIVE_FORMAT: u32 = 2;
/// Oldest [`LiveJob`] format still read: its missing fields take their
/// defaults (no file in flight shown).
pub const LIVE_FORMAT_MIN: u32 = 1;
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

/// Versioned, bounded observation of the resident's sequential dispatch queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveQueue {
    /// Format version, currently 1.
    pub format: u32,
    /// Publication time in UTC seconds, independent of job success.
    pub updated_at: i64,
    /// Currently claimed profile, if any.
    pub current: Option<sync_scheduler::QueuedProfile>,
    /// Due profiles awaiting their turn, in scheduler order.
    pub pending: Vec<sync_scheduler::QueuedProfile>,
}
impl LiveQueue {
    /// Reads the queue without initializing stores.
    /// # Errors
    /// Invalid, oversized, unreadable or unsupported observation.
    pub fn read(state: &Path) -> Result<Option<Self>, AppError> {
        let path = state.join("agent-queue.json");
        let metadata = match path.metadata() {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        if metadata.len() > 2 * 1024 * 1024 {
            return Err(error("format", "file de profils trop volumineuse"));
        }
        let queue: Self = serde_json::from_slice(&std::fs::read(path).map_err(io)?).map_err(io)?;
        if queue.format != 1 || queue.pending.len() + usize::from(queue.current.is_some()) > 1000 {
            return Err(error("format", "format de file de profils inconnu"));
        }
        Ok(Some(queue))
    }
    /// Replaces a bounded queue observation. The caller holds the resident lock.
    /// This is presentation only and never authorizes a job.
    /// # Errors
    /// Clock, serialization, size limit or storage failure.
    pub fn publish(
        state: &Path,
        current: Option<&sync_scheduler::QueuedProfile>,
        pending: &[sync_scheduler::QueuedProfile],
    ) -> Result<(), AppError> {
        if pending.len() + usize::from(current.is_some()) > 1000 {
            return Err(error("invalid_input", "file de profils trop longue"));
        }
        let queue = Self {
            format: 1,
            updated_at: Timestamp::now().map_err(io)?.secs(),
            current: current.cloned(),
            pending: pending.to_vec(),
        };
        let bytes = serde_json::to_vec(&queue).map_err(io)?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err(error("invalid_input", "file de profils trop volumineuse"));
        }
        let temp = state.join("agent-queue.json.tmp");
        std::fs::write(&temp, bytes).map_err(io)?;
        std::fs::rename(temp, state.join("agent-queue.json")).map_err(io)
    }
}

impl LiveJob {
    /// Reads the agent observation without opening or migrating application stores.
    /// This allows a window to follow a job while that job owns the command lock.
    /// Check [`Self::is_live`] before presenting it as running.
    /// # Errors
    /// Unreadable, malformed or unsupported observation. Missing means `None`.
    pub fn read(state: &Path) -> Result<Option<Self>, AppError> {
        let bytes = match std::fs::read(state.join(LIVE_FILE)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let live: Self = serde_json::from_slice(&bytes).map_err(|e| error("format", e))?;
        if !(LIVE_FORMAT_MIN..=LIVE_FORMAT).contains(&live.format) {
            return Err(error("format", "format de progression inconnu"));
        }
        Ok(Some(live))
    }

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
        LiveJob::read(&self.state)
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

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[test]
    fn queue_observation_is_optional_versioned_and_contains_no_store_initialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        assert!(LiveQueue::read(temp.path())?.is_none());
        let a = sync_scheduler::QueuedProfile {
            id: sync_core::ProfileId::new("a")?,
            name: "Photos".into(),
        };
        let b = sync_scheduler::QueuedProfile {
            id: sync_core::ProfileId::new("b")?,
            name: "Documents".into(),
        };
        LiveQueue::publish(temp.path(), Some(&a), std::slice::from_ref(&b))?;
        let queue = LiveQueue::read(temp.path())?.ok_or("missing queue")?;
        assert_eq!(queue.current, Some(a));
        assert_eq!(queue.pending, vec![b]);
        let overflow = vec![queue.pending[0].clone(); 1000];
        assert!(LiveQueue::publish(temp.path(), queue.current.as_ref(), &overflow).is_err());
        assert_eq!(
            LiveQueue::read(temp.path())?
                .ok_or("lost valid queue")?
                .pending
                .len(),
            1
        );
        assert!(!temp.path().join("scheduler.sqlite").exists());
        let mut invalid = queue;
        invalid.format = 2;
        std::fs::write(
            temp.path().join("agent-queue.json"),
            serde_json::to_vec(&invalid)?,
        )?;
        assert!(LiveQueue::read(temp.path()).is_err());
        std::fs::write(
            temp.path().join("agent-queue.json"),
            vec![b' '; 2 * 1024 * 1024 + 1],
        )?;
        assert!(LiveQueue::read(temp.path()).is_err());
        Ok(())
    }
}

/// Requests a reversible pause for exactly the observed agent command.
/// Old requests cannot pause a later command. Does not open application stores.
/// # Errors
/// Missing control identity, serialization or state-directory write failure.
pub fn request_agent_pause(state: &Path, control_id: &str, paused: bool) -> Result<(), AppError> {
    if control_id.is_empty() || control_id.len() > 256 {
        return Err(error("invalid_input", "identité de commande invalide"));
    }
    let bytes = serde_json::to_vec(&(control_id, paused)).map_err(io)?;
    let temp = state.join("agent-pause.request.tmp");
    std::fs::write(&temp, bytes).map_err(io)?;
    std::fs::rename(temp, state.join("agent-pause.request")).map_err(io)
}

impl Application {
    /// Applies a pause request for this command only (called by the agent watcher).
    /// An absent or stale request does not affect an existing token.
    /// # Errors
    /// Unreadable, oversized or malformed request, or internal state poisoned.
    pub fn poll_agent_pause(&self) -> Result<(), AppError> {
        let path = self.state.join("agent-pause.request");
        match path.metadata() {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io(e)),
            Ok(m) if m.len() > 1024 => return Err(error("format", "requête de pause trop longue")),
            Ok(_) => {}
        }
        let (id, paused): (String, bool) =
            serde_json::from_slice(&std::fs::read(path).map_err(io)?).map_err(io)?;
        let token = super::lock(&self.cancel)?;
        let snapshot = super::lock(&self.snapshot)?;
        if snapshot.active && snapshot.control_id.as_deref() == Some(&id) {
            token.set_paused(paused);
        }
        Ok(())
    }
}

#[cfg(test)]
mod pause_tests {
    use super::*;

    #[test]
    fn cross_process_pause_is_scoped_and_keeps_cancellation_available()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join("state");
        std::fs::create_dir(&state)?;
        let app = Application::open_with_lock_root(state.clone(), temp.path().join("locks"))?;
        let id = sync_core::ProfileId::new("test")?;
        let token = app.begin(&id, JobPhase::Scanning, None)?;
        let control = app.snapshot()?.control_id.ok_or("missing identity")?;
        request_agent_pause(&state, "old-command", true)?;
        app.poll_agent_pause()?;
        assert!(!token.is_paused());
        request_agent_pause(&state, &control, true)?;
        app.poll_agent_pause()?;
        assert!(token.is_paused());
        app.publish_live()?;
        assert!(
            LiveJob::read(&state)?
                .ok_or("missing observation")?
                .snapshot
                .pause_requested
        );
        request_agent_pause(&state, &control, false)?;
        app.poll_agent_pause()?;
        assert!(!token.is_paused());
        app.set_paused(true)?;
        app.cancel()?;
        assert!(token.checkpoint());
        assert!(!app.snapshot()?.pause_requested);
        let next = app.begin(&id, JobPhase::Executing, None)?;
        request_agent_pause(&state, &control, true)?;
        app.poll_agent_pause()?;
        assert!(!next.is_paused());
        assert_ne!(
            app.snapshot()?.control_id.as_deref(),
            Some(control.as_str())
        );
        assert!(request_agent_pause(&state, "", true).is_err());
        std::fs::write(state.join("agent-pause.request"), b"invalid")?;
        assert!(app.poll_agent_pause().is_err());
        assert!(!next.is_paused());
        Ok(())
    }
}
