//! Flush-behind of a staging file: the copy is paced by the device without
//! stopping its reads, and without asking the device to empty its own cache.
//!
//! The OS accepts written data into its cache at memory speed. Left alone, a
//! large file is reported transferred long before it reaches the disk, and
//! the final `sync_all` then blocks for seconds (a burst, then a stall).
//! The previous pacing flushed in the writing thread (`sync_data` every
//! 128 MiB): nothing was read during each flush, so the source and destination
//! disks took turns, and progress moved by 128 MiB steps (±33 MB/s on a 4 s
//! average). Small steps were not an option: each `sync_data` also empties the
//! device's write cache, which costs a lot on a hard disk.
//!
//! Here a dedicated thread flushes through a second handle of the same file
//! while the writer keeps going; the writer only waits when more than two
//! steps are not flushed yet. A second handle is needed on Windows:
//! synchronous requests on one file object are serialized, so a flush through
//! the writer's own handle would block its writes. Any handle of a file
//! flushes the file's cached data (shared per file).
//!
//! On Windows the flush writes the file data only, without the device cache
//! flush (`flush_data_no_sync`), every [`STEP_BYTES`]. Elsewhere, or when the
//! file system refuses it (network redirector…), `sync_data` is used every
//! [`SYNC_STEP_BYTES`]. Measured on Windows 11, 1.4 GB file to a SATA hard
//! disk (`copy_throughput` example): 102–106 MB/s with the inline 128 MiB
//! flushes, 117–125 MB/s here with a 4 s curve within ±15 %, and 120 MB/s from
//! another hard disk instead of 62–70 MB/s (reads overlap the flushes).
//!
//! This is pacing only: the writer's final `sync_all` on its own handle
//! remains the durability guarantee, whatever this thread did.
use std::fs::File;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;
use sync_core::CancellationToken;

/// Flush step of the data-only flush (Windows).
pub(crate) const STEP_BYTES: u64 = 16 * 1024 * 1024;
/// Flush step when every flush also empties the device cache (`sync_data`).
pub(crate) const SYNC_STEP_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Default)]
struct State {
    /// Bytes written so far.
    written: u64,
    /// Bytes counted as on the device.
    flushed: u64,
    /// Current step: [`STEP_BYTES`], or [`SYNC_STEP_BYTES`] after a fallback.
    step: u64,
    error: Option<io::Error>,
    stop: bool,
}

impl State {
    fn due(&self) -> bool {
        self.written - self.flushed >= self.step
    }
    /// Unflushed bytes beyond which the writer waits: two steps, so that
    /// one is written while the previous one is flushed.
    fn max_dirty(&self) -> u64 {
        2 * self.step
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A poisoned lock only means a panicking flush thread; the counters
        // stay consistent and the final `sync_all` remains authoritative.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Background flusher of one staging file.
pub(crate) struct FlushBehind {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl FlushBehind {
    /// Starts the thread with a second, writable handle of the file.
    /// # Errors
    /// The thread could not be created.
    pub(crate) fn start(handle: File) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                step: if cfg!(windows) {
                    STEP_BYTES
                } else {
                    SYNC_STEP_BYTES
                },
                ..State::default()
            }),
            changed: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("syncengine-flush".into())
            .spawn(move || flush_loop(&handle, &worker))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Records `bytes` more written, wakes the flush thread every step and
    /// waits while too much is unflushed.
    /// # Errors
    /// A background flush failed, or `cancel` was triggered while waiting
    /// (`Interrupted`).
    pub(crate) fn wrote(&mut self, bytes: u64, cancel: &CancellationToken) -> io::Result<()> {
        let mut state = self.shared.lock();
        state.written = state.written.saturating_add(bytes);
        if state.due() {
            self.shared.changed.notify_all();
        }
        loop {
            if let Some(error) = state.error.take() {
                return Err(error);
            }
            if state.written - state.flushed <= state.max_dirty() {
                return Ok(());
            }
            if cancel.is_cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Stops the thread after its current flush and closes its handle.
    pub(crate) fn stop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            // The loop has no panicking path; a panic would only lose pacing.
            let _paced_only = thread.join();
        }
    }
}

impl Drop for FlushBehind {
    fn drop(&mut self) {
        self.stop();
    }
}

fn flush_loop(handle: &File, shared: &Shared) {
    let mut data_only = cfg!(windows);
    let mut state = shared.lock();
    loop {
        if state.stop || state.error.is_some() {
            return;
        }
        if !state.due() {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        drop(state);
        let mut result = if data_only {
            flush_data(handle)
        } else {
            handle.sync_data()
        };
        if data_only && result.as_ref().is_err_and(unsupported) {
            // Not offered by this file system: full flushes, larger steps.
            data_only = false;
            shared.lock().step = SYNC_STEP_BYTES;
            result = handle.sync_data();
        }
        state = shared.lock();
        match result {
            // The cache manager also writes the pages dirtied during the
            // call: everything written by now is counted as flushed.
            Ok(()) => state.flushed = state.written,
            Err(error) => state.error = Some(error),
        }
        shared.changed.notify_all();
    }
}

/// `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED`, `ERROR_INVALID_PARAMETER`.
fn unsupported(error: &io::Error) -> bool {
    cfg!(windows) && matches!(error.raw_os_error(), Some(1 | 50 | 87))
}

#[cfg(windows)]
fn flush_data(handle: &File) -> io::Result<()> {
    cap_primitives::windows_listing::flush_data_no_sync(handle)
}

#[cfg(not(windows))]
fn flush_data(handle: &File) -> io::Result<()> {
    handle.sync_data()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn writer_is_bounded_by_background_flushes() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("staging");
        let mut file = File::create_new(&path)?;
        let mut flush = FlushBehind::start(File::options().write(true).open(&path)?)?;
        let cancel = CancellationToken::new();
        let chunk = vec![7_u8; 1024 * 1024];
        let total = 3 * SYNC_STEP_BYTES;
        for _ in 0..total / chunk.len() as u64 {
            file.write_all(&chunk)?;
            flush.wrote(chunk.len() as u64, &cancel)?;
            // Never more than the bound left unflushed after a write returns.
            let state = flush.shared.lock();
            assert!(state.written - state.flushed <= state.max_dirty());
        }
        flush.stop();
        assert!(flush.thread.is_none());
        file.sync_all()?;
        assert_eq!(std::fs::metadata(&path)?.len(), total);
        Ok(())
    }

    #[test]
    fn cancellation_ends_a_wait_and_stop_is_idempotent() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("staging");
        File::create_new(&path)?;
        let mut flush = FlushBehind::start(File::options().write(true).open(&path)?)?;
        // Given a stopped flusher, nothing is flushed any more.
        flush.stop();
        let cancel = CancellationToken::new();
        cancel.cancel();
        // When a write exceeds the bound, Then the wait ends on cancel.
        let error = flush.wrote(2 * SYNC_STEP_BYTES + 1, &cancel);
        assert_eq!(error.map_err(|e| e.kind()), Err(io::ErrorKind::Interrupted));
        flush.stop();
        Ok(())
    }

    /// Given a handle that cannot flush (read-only on Windows), When the
    /// writer passes a step, Then the flush error reaches the writer.
    #[cfg(windows)]
    #[test]
    fn a_failed_flush_is_reported_to_the_writer() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("staging");
        File::create_new(&path)?;
        let mut flush = FlushBehind::start(File::open(&path)?)?;
        let cancel = CancellationToken::new();
        assert!(flush.wrote(3 * STEP_BYTES, &cancel).is_err());
        flush.stop();
        Ok(())
    }
}
