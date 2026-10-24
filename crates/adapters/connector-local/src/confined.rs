//! Directory-handle based confinement; no ambient path mutation after opening.
use crate::errors::io_error;
use crate::paths::to_os_path;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use connector_api::{StorageError, StorageOperation};
use std::sync::{Arc, Mutex};
use sync_core::ResourcePath;

/// Directory handles kept by a read-only scanning connector.
const CACHED_HANDLES: usize = 64;

/// Bounded most-recently-used set of directory handles already reached by a
/// confined walk (no symlink, really a directory, at that time). Reusing one
/// skips re-walking its ancestors from the root: a scan then opens each
/// directory once instead of once per descendant.
///
/// A handle follows the directory object, not its name: if a cached directory
/// is renamed meanwhile, reads through it still observe that object. This is
/// why only connectors without mutations use it (preview scans); execution
/// re-walks from the root before every precondition and mutation.
#[derive(Debug, Default)]
pub(crate) struct Handles(Mutex<Vec<(ResourcePath, Arc<Dir>)>>);

impl Handles {
    fn get(&self, path: &ResourcePath) -> Option<Arc<Dir>> {
        let mut slots = self.0.lock().ok()?;
        let index = slots.iter().position(|(p, _)| p == path)?;
        let slot = slots.remove(index);
        let dir = Arc::clone(&slot.1);
        slots.insert(0, slot);
        Some(dir)
    }

    fn put(&self, path: ResourcePath, dir: Arc<Dir>) {
        // A poisoned cache only loses its speed-up, never correctness.
        if let Ok(mut slots) = self.0.lock() {
            slots.retain(|(p, _)| *p != path);
            slots.insert(0, (path, dir));
            slots.truncate(CACHED_HANDLES);
        }
    }
}

/// Opens `segment` under `current`, refusing links and non-directories.
fn step(
    current: &Dir,
    segment: &str,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<Dir, StorageError> {
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
    current
        .open_dir_nofollow(segment)
        .map_err(|e| io_error(e, op, path))
}

pub(crate) fn directory(
    root: &Dir,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<Dir, StorageError> {
    // Apply Win32 ambiguity rules as well as the logical path invariants.
    to_os_path(std::path::Path::new(""), path, op)?;
    let mut current = root.try_clone().map_err(|e| io_error(e, op, path))?;
    for segment in path.segments() {
        current = step(&current, segment, path, op)?;
    }
    Ok(current)
}

/// [`directory`] reusing the deepest cached ancestor when `cache` is given.
/// Same checks and errors for every segment actually opened.
pub(crate) fn directory_via(
    root: &Arc<Dir>,
    cache: Option<&Handles>,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<Arc<Dir>, StorageError> {
    let Some(cache) = cache else {
        return directory(root, path, op).map(Arc::new);
    };
    if path.is_root() {
        return Ok(Arc::clone(root));
    }
    to_os_path(std::path::Path::new(""), path, op)?;
    // Ancestors from `path` itself up to (excluding) the root.
    let mut chain = Vec::with_capacity(path.depth());
    let mut next = Some(path.clone());
    while let Some(current) = next.filter(|p| !p.is_root()) {
        next = current.parent();
        chain.push(current);
    }
    let mut base = None;
    let mut missing = chain.len();
    for (index, ancestor) in chain.iter().enumerate() {
        if let Some(dir) = cache.get(ancestor) {
            base = Some(dir);
            missing = index;
            break;
        }
    }
    let mut current = match base {
        Some(dir) => dir,
        None => Arc::clone(root),
    };
    for ancestor in chain.drain(..missing).rev() {
        let segment = ancestor.segments().last().unwrap_or("");
        current = Arc::new(step(&current, segment, path, op)?);
        cache.put(ancestor, Arc::clone(&current));
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

/// [`parent`] through the optional handle cache.
pub(crate) fn parent_via(
    root: &Arc<Dir>,
    cache: Option<&Handles>,
    path: &ResourcePath,
    op: StorageOperation,
) -> Result<Arc<Dir>, StorageError> {
    to_os_path(std::path::Path::new(""), path, op)?;
    directory_via(root, cache, &path.parent().unwrap_or_default(), op)
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
