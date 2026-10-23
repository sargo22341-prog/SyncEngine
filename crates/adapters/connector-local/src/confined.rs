//! Directory-handle based confinement; no ambient path mutation after opening.
use crate::errors::io_error;
use crate::paths::to_os_path;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use connector_api::{StorageError, StorageOperation};
use sync_core::ResourcePath;

pub(crate) fn directory(
    root: &Dir,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<Dir, StorageError> {
    // Apply Win32 ambiguity rules as well as the logical path invariants.
    to_os_path(std::path::Path::new(""), path, op)?;
    let mut current = root.try_clone().map_err(|e| io_error(e, op, path))?;
    for segment in path.segments() {
        let meta = current
            .symlink_metadata(segment)
            .map_err(|e| io_error(e, op, path))?;
        if meta.file_type().is_symlink() {
            return Err(crate::errors::local_error(
                connector_api::StorageErrorKind::Permanent,
                op,
                path,
                "symlink parent refused",
            ));
        }
        if !meta.is_dir() {
            let kind = if op == StorageOperation::List {
                connector_api::StorageErrorKind::Permanent
            } else {
                connector_api::StorageErrorKind::NotFound
            };
            return Err(crate::errors::local_error(
                kind,
                op,
                path,
                "parent is not a directory",
            ));
        }
        current = current
            .open_dir_nofollow(segment)
            .map_err(|e| io_error(e, op, path))?;
    }
    Ok(current)
}

pub(crate) fn parent(
    root: &Dir,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<Dir, StorageError> {
    to_os_path(std::path::Path::new(""), path, op)?;
    directory(root, &path.parent().unwrap_or_default(), op)
}

pub(crate) fn entry(
    path: ResourcePath,
    meta: &cap_std::fs::Metadata,
    precision: sync_core::TimestampPrecision,
) -> sync_core::FileEntry {
    let kind = if meta.file_type().is_symlink() {
        sync_core::EntryKind::Symlink
    } else if meta.is_dir() {
        sync_core::EntryKind::Directory
    } else if meta.is_file() {
        sync_core::EntryKind::File
    } else {
        sync_core::EntryKind::Other
    };
    let mut data = sync_core::FileMetadata::unknown();
    if kind == sync_core::EntryKind::File {
        data = data.with_size(meta.len());
    }
    if let Ok(time) = meta.modified()
        && let Ok(time) = sync_core::Timestamp::from_system_time(time.into_std())
    {
        data = data.with_modified(time, precision);
    }
    sync_core::FileEntry::new(path, kind, data)
}
