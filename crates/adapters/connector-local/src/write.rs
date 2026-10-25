//! Mutations confined to stable directory handles. No ambient path publication.
use crate::{
    confined,
    connector::cancelled,
    errors::{io_error, local_error},
};
use cap_std::fs::{Dir, OpenOptions};
use connector_api::{
    CommitMode, ContentWriter, StorageError, StorageErrorKind, StorageOperation, WriteOptions,
};
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath, TimestampPrecision};
pub(crate) const STAGING_PREFIX: &str = ".syncengine-";
pub(crate) const STAGING_SUFFIX: &str = ".partial";
/// Written bytes after which the staging file is flushed to the device
/// (`sync_data`) while the transfer goes on. Without it the OS accepts a whole
/// large file into its write cache at memory speed, then the final
/// `sync_all` blocks for seconds: progress (and throughput) would report a
/// burst followed by a stall instead of the device's actual pace. Bounding
/// dirty data adds no write; the final `sync_all` stays. Measured on a 2 GB
/// file to a 134 MB/s disk (Windows 11): 64 MiB costs ~10 % in time, 128 MiB
/// none measurable (30.2 s / 29.8 s against 30.4 s / 29.2 s) with a steady
/// rate, 256 MiB none either but 2 s steps visible in a 4 s average window.
pub(crate) const FLUSH_EVERY_BYTES: u64 = 128 * 1024 * 1024;
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn staging_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.as_nanos())
        .unwrap_or(0);
    format!(
        "{STAGING_PREFIX}{}-{nanos}-{}{STAGING_SUFFIX}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
fn name(path: &ResourcePath) -> Result<&str, StorageError> {
    path.segments().last().ok_or_else(|| {
        local_error(
            StorageErrorKind::Permanent,
            StorageOperation::Write,
            path,
            "root mutation forbidden",
        )
    })
}
fn sync_directory(
    dir: &Dir,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<(), StorageError> {
    #[cfg(unix)]
    // cap-std may hold O_PATH handles on Linux; fsync requires a readable
    // directory descriptor. Reopen '.' relative to the granted handle.
    dir.open(".")
        .and_then(|d| d.sync_all())
        .map_err(|e| io_error(e, op, path))?;
    #[cfg(not(unix))]
    let _ = (dir, path, op);
    Ok(())
}
/// Windows: antivirus/indexers briefly open a just-closed file without
/// FILE_SHARE_DELETE, so renaming or removing it fails with ACCESS_DENIED (5),
/// SHARING_VIOLATION (32) or LOCK_VIOLATION (33). A failed attempt changes
/// nothing, so it is retried a bounded number of times (about 1.3 s in total).
/// A persistent denial (ACL, read-only target) is still reported afterwards.
/// Other platforms run the operation exactly once.
fn retry_transient_lock<T>(
    cancel: &CancellationToken,
    mut operation: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    #[cfg(windows)]
    {
        let mut delay = std::time::Duration::from_millis(10);
        for _ in 0..7 {
            match operation() {
                Err(e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)) => {
                    if cancel.is_cancelled() {
                        return Err(e);
                    }
                    std::thread::sleep(delay);
                    delay = delay.saturating_mul(2);
                }
                result => return result,
            }
        }
    }
    #[cfg(not(windows))]
    let _ = cancel;
    operation()
}
pub(crate) fn create_directory(
    root: &Dir,
    path: &ResourcePath,
    precision: TimestampPrecision,
) -> Result<FileEntry, StorageError> {
    let op = StorageOperation::CreateDirectory;
    let parent = confined::parent(root, path, op)?;
    parent
        .create_dir(name(path)?)
        .map_err(|e| io_error(e, op, path))?;
    sync_directory(&parent, path, op)?;
    let meta = parent
        .symlink_metadata(name(path)?)
        .map_err(|e| io_error(e, op, path))?;
    Ok(confined::entry(path.clone(), &meta, precision))
}
pub(crate) fn delete(
    root: &Dir,
    path: &ResourcePath,
    expected: EntryKind,
) -> Result<(), StorageError> {
    let op = StorageOperation::Delete;
    let parent = confined::parent(root, path, op)?;
    let name = name(path)?;
    let meta = parent
        .symlink_metadata(name)
        .map_err(|e| io_error(e, op, path))?;
    if meta.file_type().is_symlink() {
        return Err(local_error(
            StorageErrorKind::NotSupported,
            op,
            path,
            "links are never deleted",
        ));
    }
    let result = match expected {
        EntryKind::File if meta.is_file() => parent.remove_file(name),
        EntryKind::Directory if meta.is_dir() => parent.remove_dir(name),
        _ => {
            return Err(local_error(
                StorageErrorKind::Conflict,
                op,
                path,
                "entry type changed",
            ));
        }
    };
    result.map_err(|e| {
        if e.kind() == io::ErrorKind::DirectoryNotEmpty {
            local_error(
                StorageErrorKind::Conflict,
                op,
                path,
                "directory not empty; recursive deletion forbidden",
            )
        } else {
            io_error(e, op, path)
        }
    })?;
    sync_directory(&parent, path, op)
}
pub(crate) struct LocalWriter {
    parent: Dir,
    file: Option<File>,
    target: ResourcePath,
    target_name: String,
    staging: ResourcePath,
    staging_name: String,
    options: WriteOptions,
    precision: TimestampPrecision,
    cancel: CancellationToken,
    finished: bool,
    /// Bytes written since the last intermediate flush.
    unflushed: u64,
}
impl LocalWriter {
    pub(crate) fn open(
        root: &Dir,
        path: &ResourcePath,
        options: WriteOptions,
        precision: TimestampPrecision,
        cancel: &CancellationToken,
    ) -> Result<Self, StorageError> {
        let op = StorageOperation::Write;
        let parent = confined::parent(root, path, op)?;
        let staging_name = staging_name();
        let staging = path
            .parent()
            .unwrap_or_default()
            .join(&staging_name)
            .map_err(|_| {
                local_error(
                    StorageErrorKind::Permanent,
                    op,
                    path,
                    "invalid staging name",
                )
            })?;
        let mut open = OpenOptions::new();
        // Readable too: verification rereads through this same handle.
        open.read(true).write(true).create_new(true);
        let file = parent
            .open_with(&staging_name, &open)
            .map_err(|e| io_error(e, op, path))?
            .into_std();
        Ok(Self {
            parent,
            file: Some(file),
            target: path.clone(),
            target_name: name(path)?.into(),
            staging,
            staging_name,
            options,
            precision,
            cancel: cancel.clone(),
            finished: false,
            unflushed: 0,
        })
    }
    fn publish(&mut self, mode: CommitMode) -> Result<FileEntry, StorageError> {
        let op = StorageOperation::Write;
        if self.cancel.is_cancelled() {
            return Err(cancelled(op, &self.target));
        }
        let file = self.file.take().ok_or_else(|| {
            local_error(
                StorageErrorKind::Permanent,
                op,
                &self.target,
                "writer closed",
            )
        })?;
        if let Some(time) = self.options.modified.and_then(|t| t.to_system_time()) {
            file.set_modified(time)
                .map_err(|e| io_error(e, op, &self.target))?;
        }
        file.sync_all().map_err(|e| io_error(e, op, &self.target))?;
        drop(file);
        match mode {
            CommitMode::CreateNew => {
                // No check-then-rename fallback: unsupported filesystems fail closed.
                self.parent
                    .hard_link(&self.staging_name, &self.parent, &self.target_name)
                    .map_err(|e| io_error(e, op, &self.target))?;
                retry_transient_lock(&self.cancel, || self.parent.remove_file(&self.staging_name))
                    .map_err(|e| io_error(e, op, &self.staging))?;
            }
            CommitMode::ReplaceExisting => {
                let meta = self
                    .parent
                    .symlink_metadata(&self.target_name)
                    .map_err(|e| {
                        if e.kind() == io::ErrorKind::NotFound {
                            local_error(
                                StorageErrorKind::Conflict,
                                op,
                                &self.target,
                                "target disappeared",
                            )
                        } else {
                            io_error(e, op, &self.target)
                        }
                    })?;
                if !meta.is_file() || meta.file_type().is_symlink() {
                    return Err(local_error(
                        StorageErrorKind::Conflict,
                        op,
                        &self.target,
                        "target type changed",
                    ));
                }
                retry_transient_lock(&self.cancel, || {
                    self.parent
                        .rename(&self.staging_name, &self.parent, &self.target_name)
                })
                .map_err(|e| io_error(e, op, &self.target))?;
            }
            _ => {
                return Err(local_error(
                    StorageErrorKind::NotSupported,
                    op,
                    &self.target,
                    "unsupported commit mode",
                ));
            }
        }
        sync_directory(&self.parent, &self.target, op)?;
        let meta = self
            .parent
            .symlink_metadata(&self.target_name)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    local_error(
                        StorageErrorKind::Conflict,
                        op,
                        &self.target,
                        "target disappeared",
                    )
                } else {
                    io_error(e, op, &self.target)
                }
            })?;
        Ok(confined::entry(self.target.clone(), &meta, self.precision))
    }
    fn cleanup(&self) -> Result<(), StorageError> {
        match self.parent.remove_file(&self.staging_name) {
            Ok(()) => sync_directory(&self.parent, &self.staging, StorageOperation::Write),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(e, StorageOperation::Write, &self.staging)),
        }
    }
}
impl ContentWriter for LocalWriter {
    fn write(&mut self, data: &[u8]) -> Result<(), StorageError> {
        let op = StorageOperation::Write;
        if self.cancel.is_cancelled() {
            return Err(cancelled(op, &self.target));
        }
        let file = self.file.as_mut().ok_or_else(|| {
            local_error(
                StorageErrorKind::Permanent,
                op,
                &self.target,
                "writer closed",
            )
        })?;
        file.write_all(data)
            .map_err(|e| io_error(e, op, &self.target))?;
        self.unflushed = self.unflushed.saturating_add(data.len() as u64);
        if self.unflushed >= FLUSH_EVERY_BYTES {
            self.unflushed = 0;
            file.sync_data()
                .map_err(|e| io_error(e, op, &self.target))?;
        }
        Ok(())
    }
    fn staging_path(&self) -> Option<&ResourcePath> {
        Some(&self.staging)
    }
    fn read_back(
        &mut self,
        buffer: &mut [u8],
        sink: &mut dyn FnMut(&[u8]),
    ) -> Result<bool, StorageError> {
        let op = StorageOperation::Read;
        let file = self.file.as_mut().ok_or_else(|| {
            local_error(
                StorageErrorKind::Permanent,
                op,
                &self.staging,
                "writer closed",
            )
        })?;
        file.flush()
            .and_then(|()| file.rewind())
            .map_err(|e| io_error(e, op, &self.staging))?;
        loop {
            if self.cancel.checkpoint() {
                return Err(cancelled(op, &self.staging));
            }
            match file.read(buffer) {
                Ok(0) => return Ok(true),
                Ok(read) => sink(buffer.get(..read).unwrap_or_default()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(e, op, &self.staging)),
            }
        }
    }
    fn commit(mut self: Box<Self>, mode: CommitMode) -> Result<FileEntry, StorageError> {
        let result = self.publish(mode);
        self.file = None;
        self.finished = true;
        match result {
            Err(original) => match self.cleanup() {
                Ok(()) => Err(original),
                Err(cleanup) => Err(local_error(
                    original.kind(),
                    StorageOperation::Write,
                    &self.staging,
                    &format!(
                        "publication failed: {original}; temporary cleanup also failed: {cleanup}"
                    ),
                )
                .with_source(original)),
            },
            Ok(entry) => Ok(entry),
        }
    }
    fn abort(mut self: Box<Self>) -> Result<(), StorageError> {
        self.file = None;
        self.finished = true;
        self.cleanup()
    }
}
impl Drop for LocalWriter {
    fn drop(&mut self) {
        if !self.finished {
            self.file = None;
            let _cleanup_failed = self.cleanup().is_err();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staging_names_are_unique_and_recognisable() {
        let a = staging_name();
        let b = staging_name();
        assert_ne!(a, b);
        assert!(crate::is_staging_name(&a));
        assert!(ResourcePath::root().join(&a).is_ok());
    }
}
