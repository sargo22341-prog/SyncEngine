//! Exact names written in the program folder and their removal.
//!
//! Nothing is written outside the program folder. Removal never follows a
//! link or junction: a link found under one of our names is removed itself.

use crate::error::{ErrorKind, UpdateError, fail, io_at};
use crate::version::Target;
use std::path::{Path, PathBuf};

/// Archive being downloaded.
pub(crate) const DOWNLOAD: &str = "syncengine-update.download";
/// Folder of the extracted files awaiting replacement.
pub(crate) const STAGING: &str = "syncengine-update.staging";
/// Suffix of the previous files kept until the next start.
pub(crate) const OLD_SUFFIX: &str = ".old";

/// Program folder of the running interface `exe`.
///
/// # Errors
/// [`ErrorKind::Unsupported`] when the executable was renamed (its file name
/// is not the one shipped in the archive): replacing would leave it behind.
pub fn program_dir(exe: &Path, target: Target) -> Result<PathBuf, UpdateError> {
    let expected = target.desktop_name();
    // Windows file names are case-insensitive, Linux ones are not.
    let named = exe.file_name().is_some_and(|name| {
        if cfg!(windows) {
            name.eq_ignore_ascii_case(&expected)
        } else {
            name == expected.as_str()
        }
    });
    match exe.parent() {
        Some(dir) if named && exe.is_absolute() => Ok(dir.to_path_buf()),
        _ => Err(fail(
            ErrorKind::Unsupported,
            format!(
                "l’exécutable ne s’appelle pas {expected} : mettez à jour manuellement depuis la page de la release"
            ),
        )),
    }
}

/// Removes a file or a link, never a directory. Missing is success.
pub(crate) fn remove_file_if_present(path: &Path) -> Result<(), UpdateError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_at("lecture de", path, e)),
        Ok(meta) if meta.is_dir() => Err(fail(
            ErrorKind::Io,
            format!("{} est un dossier : non retiré", path.display()),
        )),
        Ok(meta) => remove_entry(path, meta.file_type()),
    }
}

/// Removes a non-directory entry, or a link itself (a directory link or
/// junction under Windows needs `remove_dir`, which does not follow it).
fn remove_entry(path: &Path, kind: std::fs::FileType) -> Result<(), UpdateError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if kind.is_symlink() => {
            std::fs::remove_dir(path).map_err(|_| io_at("suppression de", path, e))
        }
        Err(e) => Err(io_at("suppression de", path, e)),
    }
}

/// Removes `path` after a failure, keeping the original error first.
pub(crate) fn remove_quietly(path: &Path, error: UpdateError) -> UpdateError {
    match remove_file_if_present(path) {
        Ok(()) => error,
        Err(extra) => error.also(format!("fichier temporaire non retiré : {extra}")),
    }
}

/// Removes the staging folder: only the exact member names inside, then the
/// folder if it is empty. A link under the folder name is removed itself.
pub(crate) fn remove_staging(dir: &Path, target: Target) -> Result<(), UpdateError> {
    let staging = dir.join(STAGING);
    let meta = match std::fs::symlink_metadata(&staging) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_at("lecture de", &staging, e)),
        Ok(meta) => meta,
    };
    if !meta.is_dir() {
        return remove_entry(&staging, meta.file_type());
    }
    for member in target.members() {
        remove_file_if_present(&staging.join(member))?;
    }
    std::fs::remove_dir(&staging).map_err(|e| io_at("suppression de", &staging, e))
}

/// Result of [`cleanup`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cleanup {
    /// Removed entries.
    pub removed: Vec<PathBuf>,
    /// Entries that could not be removed, with the cause (retried at the
    /// next start; a `.old` executable may still be running).
    pub failed: Vec<(PathBuf, String)>,
}

/// Removes what a previous update left in the program folder: `<member>.old`
/// for the five archive members, the temporary download and the staging
/// folder. Exact names only; links are never followed.
///
/// Call it at start-up, once this window owns the state (no other window of
/// the same program folder may be updating).
#[must_use]
pub fn cleanup(dir: &Path, target: Target) -> Cleanup {
    let mut report = Cleanup::default();
    let mut files: Vec<PathBuf> = target
        .members()
        .into_iter()
        .map(|m| dir.join(format!("{m}{OLD_SUFFIX}")))
        .collect();
    files.push(dir.join(DOWNLOAD));
    for path in files {
        if std::fs::symlink_metadata(&path).is_err() {
            continue;
        }
        match remove_file_if_present(&path) {
            Ok(()) => report.removed.push(path),
            Err(e) => report.failed.push((path, e.message().to_owned())),
        }
    }
    let staging = dir.join(STAGING);
    if std::fs::symlink_metadata(&staging).is_ok() {
        match remove_staging(dir, target) {
            Ok(()) => report.removed.push(staging),
            Err(e) => report.failed.push((staging, e.message().to_owned())),
        }
    }
    report
}
