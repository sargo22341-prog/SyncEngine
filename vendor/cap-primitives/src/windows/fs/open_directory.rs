//! SYNCENGINE-PATCH: one-component confined opens with a minimal tag query.
//! Safety and compatibility review: SYNCENGINE-PATCH.md.
use crate::fs::{FollowSymlinks, OpenOptions, OpenOptionsExt};
use std::ffi::OsStr;
use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
use std::path::Path;
use std::{fs, io};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_STOPPED_ON_SYMLINK,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

/// Opens exactly one child directory, never following a name-surrogate reparse
/// point. The parent capability stays authoritative; no absolute path is used.
/// The original cap-primitives implementation is the compatibility fallback
/// only if the minimal information class is explicitly unsupported.
pub fn open_directory_nofollow(parent: &fs::File, name: &OsStr) -> io::Result<fs::File> {
    let wide: Vec<_> = name.encode_wide().collect();
    if wide.is_empty()
        || name == "."
        || name == ".."
        || matches!(wide.last(), Some(32 | 46))
        || wide.iter().any(|c| matches!(*c, 0 | 47 | 58 | 92))
    {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut options = OpenOptions::new();
    // OPEN_REPARSE_POINT opens the link object itself for inspection, not its
    // target. Do not request dir_required: that adds full generic metadata.
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    let file = crate::fs::open(parent, Path::new(name), &options)?;
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: the owned file remains live; info is initialized, correctly
    // aligned, writable and exactly the fixed buffer size supplied. The
    // synchronous call retains no pointer. No variable data is parsed.
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if success == 0 {
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error().map(|c| c as u32),
            Some(ERROR_INVALID_FUNCTION | ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED)
        ) {
            drop(file);
            return crate::fs::open_dir_nofollow(parent, Path::new(name));
        }
        return Err(error);
    }
    // Test the tag before the directory bit: links may themselves be directories.
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && info.ReparseTag & 0x2000_0000 != 0
    {
        return Err(io::Error::from_raw_os_error(
            ERROR_STOPPED_ON_SYMLINK as i32,
        ));
    }
    if info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::from(io::ErrorKind::NotADirectory));
    }
    Ok(file)
}
