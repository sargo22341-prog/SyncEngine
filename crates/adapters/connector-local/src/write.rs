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
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath, TimestampPrecision};
pub(crate) const STAGING_PREFIX: &str = ".syncengine-";
pub(crate) const STAGING_SUFFIX: &str = ".partial";
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
        open.write(true).create_new(true);
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
                self.parent
                    .remove_file(&self.staging_name)
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
                self.parent
                    .rename(&self.staging_name, &self.parent, &self.target_name)
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
        self.file
            .as_mut()
            .ok_or_else(|| {
                local_error(
                    StorageErrorKind::Permanent,
                    op,
                    &self.target,
                    "writer closed",
                )
            })?
            .write_all(data)
            .map_err(|e| io_error(e, op, &self.target))
    }
    fn staging_path(&self) -> Option<&ResourcePath> {
        Some(&self.staging)
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
