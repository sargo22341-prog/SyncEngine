use std::path::{Path, PathBuf};
use std::{fs, io};

// SYNCENGINE-PATCH: preserve UNC roots rather than turning them into relative paths.
#[path = "normalize_path.rs"]
mod normalize_path;

/// Calculates system path of `file`.
///
/// This function will automatically strip the extended prefix from the
/// resultant path to allow for joining this resultant path with relative
/// components.
pub(crate) fn get_path(file: &fs::File) -> io::Result<PathBuf> {
    // get system path to the handle
    let path = winx::file::get_file_path(file)?;

    Ok(normalize_path::normalize(&path))
}

/// Convenience function for calling `get_path` and concatenating the result
/// with `path`.
pub(super) fn concatenate(file: &fs::File, path: &Path) -> io::Result<PathBuf> {
    let file_path = get_path(file)?;
    Ok(file_path.join(path))
}
