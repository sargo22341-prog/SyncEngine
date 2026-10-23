//! Process exclusion shared by the application clients, including direct CLI runs.
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Default per-user execution lock directory. All production clients use this
/// location; explicit roots are intended for isolated tests or managed deployments.
/// # Errors
/// Missing or relative user state directory.
pub fn default_lock_root() -> io::Result<PathBuf> {
    let root = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
    }
    .filter(|p| p.is_absolute())
    .ok_or_else(|| io::Error::other("absolute user state directory required"))?;
    Ok(root.join("SyncEngine/job-locks"))
}

/// Acquires nonblocking, per-user exclusion for the entire execution lifecycle,
/// including durable intent and baseline publication. Hold the returned file
/// until completion. Never delete the lock file (even after a crash).
/// This excludes cooperating clients of the same account, not external writers.
/// # Errors
/// Busy (`WouldBlock`), invalid directory or filesystem failure.
pub fn execution_lock(root: &Path) -> io::Result<File> {
    std::fs::create_dir_all(root)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("execution.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "another SyncEngine execution owns the destinations",
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owners_are_exclusive_and_release_without_unlinking() -> io::Result<()> {
        let dir = tempfile::tempdir()?;
        let first = execution_lock(dir.path())?;
        assert_eq!(
            execution_lock(dir.path()).err().map(|e| e.kind()),
            Some(io::ErrorKind::WouldBlock)
        );
        drop(first);
        let _next = execution_lock(dir.path())?;
        assert!(dir.path().join("execution.lock").is_file());
        Ok(())
    }
}
