//! SYNCENGINE-PATCH: opt-in native bulk enumeration from an owned directory handle.
//! Safety review and provenance: SYNCENGINE-PATCH.md. No caller-side unsafe.
use super::get_path::get_path;
use crate::fs::{FileType, ImplFileTypeExt, ImplMetadataExt, Metadata, Permissions};
use crate::time::SystemTime;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::time::{Duration, UNIX_EPOCH};
use std::{fs, io};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
    FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
};
#[cfg(test)]
use windows_sys::Win32::System::SystemServices::{
    IO_REPARSE_TAG_MOUNT_POINT, IO_REPARSE_TAG_SYMLINK,
};

/// One directory entry; metadata comes from WIN32_FIND_DATA, never a per-file open.
pub struct Entry(pub(super) WIN32_FIND_DATAW);
impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkEntry")
            .field("name", &self.file_name())
            .finish()
    }
}
impl Entry {
    /// Original UTF-16 name, including unpaired surrogates.
    pub fn file_name(&self) -> OsString {
        let end = self
            .0
            .cFileName
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(self.0.cFileName.len());
        OsString::from_wide(&self.0.cFileName[..end])
    }
    /// Enumeration metadata, without volume/file identities, like DirEntry::metadata.
    pub fn metadata(&self) -> Metadata {
        let attributes = self.0.dwFileAttributes;
        // IsReparseTagNameSurrogate: covers junctions, symlinks and future
        // name-surrogate tags; cloud-placeholder tags alone are not links.
        const NAME_SURROGATE: u32 = 0x2000_0000;
        let link = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            && self.0.dwReserved0 & NAME_SURROGATE != 0;
        let file_type = if link {
            FileType::ext(ImplFileTypeExt::SymlinkUnknown)
        } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            FileType::dir()
        } else {
            FileType::file()
        };
        Metadata {
            file_type,
            len: (u64::from(self.0.nFileSizeHigh) << 32) | u64::from(self.0.nFileSizeLow),
            permissions: Permissions {
                readonly: attributes & FILE_ATTRIBUTE_READONLY != 0,
            },
            modified: time(self.0.ftLastWriteTime),
            accessed: time(self.0.ftLastAccessTime),
            created: time(self.0.ftCreationTime),
            ext: ImplMetadataExt::from_find_data(&self.0),
        }
    }
}

fn time(value: windows_sys::Win32::Foundation::FILETIME) -> Option<SystemTime> {
    const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    let value = (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    let ticks = value.abs_diff(UNIX_EPOCH_TICKS);
    let duration = Duration::new(ticks / 10_000_000, ((ticks % 10_000_000) * 100) as u32);
    let time = if value >= UNIX_EPOCH_TICKS {
        UNIX_EPOCH.checked_add(duration)
    } else {
        UNIX_EPOCH.checked_sub(duration)
    };
    time.map(SystemTime::from_std)
}

// Handle stored as an opaque integer, never dereferenced. Ownership is unique;
// next requires &mut self, and Windows search handles may move between threads.
struct Search(usize);
impl Drop for Search {
    fn drop(&mut self) {
        // SAFETY: exactly one successful FindFirstFileExW created this handle;
        // it is never closed elsewhere and FindClose is its matching destructor.
        unsafe {
            FindClose(self.0 as HANDLE);
        }
    }
}

/// Bounded native cursor. The directory file stays open until the cursor is dropped.
pub struct ReadDir {
    search: Option<Search>,
    _directory: fs::File,
    first: Option<Entry>,
}
impl std::fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BulkReadDir").finish_non_exhaustive()
    }
}
impl ReadDir {
    /// Enumerates an owned directory using a large fetch buffer. A server which
    /// explicitly rejects the flag is retried once without it; other errors fail.
    pub fn new(directory: fs::File) -> io::Result<Self> {
        let root = get_path(&directory)?;
        let pattern = search_pattern(&root)?;
        let mut data = WIN32_FIND_DATAW::default();
        // SAFETY: pattern is NUL-terminated and lives across this call; data is a
        // writable, correctly aligned WIN32_FIND_DATAW; optional filter is null.
        let mut handle = unsafe {
            FindFirstFileExW(
                pattern.as_ptr(),
                FindExInfoBasic,
                &mut data as *mut _ as _,
                FindExSearchNameMatch,
                std::ptr::null(),
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // SAFETY: GetLastError is read immediately after the failed Win32 call.
            let code = unsafe { GetLastError() };
            if code == ERROR_INVALID_PARAMETER || code == ERROR_NOT_SUPPORTED {
                // SAFETY: identical buffers as above; only the optional flag changes.
                handle = unsafe {
                    FindFirstFileExW(
                        pattern.as_ptr(),
                        FindExInfoBasic,
                        &mut data as *mut _ as _,
                        FindExSearchNameMatch,
                        std::ptr::null(),
                        0,
                    )
                };
            } else if code == ERROR_FILE_NOT_FOUND {
                // Never infer an empty directory from an unavailable/missing root.
                if !fs::metadata(&root)?.is_dir() {
                    return Err(io::Error::from(io::ErrorKind::NotADirectory));
                }
                return Ok(Self {
                    _directory: directory,
                    search: None,
                    first: None,
                });
            } else {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        }
        if handle == INVALID_HANDLE_VALUE {
            // SAFETY: read immediately after the fallback call failed.
            let code = unsafe { GetLastError() };
            if code == ERROR_FILE_NOT_FOUND && fs::metadata(&root)?.is_dir() {
                return Ok(Self {
                    _directory: directory,
                    search: None,
                    first: None,
                });
            }
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        Ok(Self {
            _directory: directory,
            search: Some(Search(handle as usize)),
            first: Some(Entry(data)),
        })
    }
}
impl Iterator for ReadDir {
    type Item = io::Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = if let Some(first) = self.first.take() {
                first
            } else {
                let handle = self.search.as_ref()?;
                let mut data = WIN32_FIND_DATAW::default();
                // SAFETY: the unique search handle is alive; data is a writable
                // correctly aligned buffer, and iteration requires exclusive access.
                if unsafe { FindNextFileW(handle.0 as HANDLE, &mut data) } == 0 {
                    // SAFETY: read immediately after the failed enumeration call.
                    let code = unsafe { GetLastError() };
                    drop(self.search.take());
                    return if code == ERROR_NO_MORE_FILES {
                        None
                    } else {
                        Some(Err(io::Error::from_raw_os_error(code as i32)))
                    };
                }
                Entry(data)
            };
            let name = item.file_name();
            if name != "." && name != ".." {
                return Some(Ok(item));
            }
        }
    }
}

// Rust's standard iterator deliberately does not request LARGE_FETCH. This
// explicit API is only used when the caller exhaustively scans a directory.
fn search_pattern(root: &Path) -> io::Result<Vec<u16>> {
    if !root.is_absolute() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let clean: PathBuf = root
        .components()
        .filter(|c| *c != Component::CurDir)
        .collect();
    let path = clean.join("*");
    let wide: Vec<_> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let prefix = match path.components().next() {
        Some(Component::Prefix(p)) => p.kind(),
        _ => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
    };
    let mut result: Vec<u16> = match prefix {
        Prefix::Disk(_) => r"\\?\".encode_utf16().chain(wide).collect(),
        Prefix::UNC(_, _) => r"\\?\UNC\"
            .encode_utf16()
            .chain(wide.into_iter().skip(2))
            .collect(),
        Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _) => wide,
        _ => return Err(io::Error::from(io::ErrorKind::Unsupported)),
    };
    result.push(0);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_keep_unc_unicode_and_long_path_prefixes() {
        for (input, expected) in [
            (r"C:\folder", r"\\?\C:\folder\*"),
            (r"\\server\share\été", r"\\?\UNC\server\share\été\*"),
            (r"\\?\UNC\server\share", r"\\?\UNC\server\share\*"),
        ] {
            let mut wanted: Vec<_> = expected.encode_utf16().collect();
            wanted.push(0);
            assert_eq!(search_pattern(Path::new(input)).unwrap(), wanted);
        }
        assert!(search_pattern(Path::new("relative")).is_err());
    }

    #[test]
    fn facts_preserve_size_time_readonly_and_unpaired_utf16() {
        let mut data = WIN32_FIND_DATAW::default();
        data.cFileName[0] = 0xd800;
        data.nFileSizeHigh = 1;
        data.nFileSizeLow = 23;
        data.dwFileAttributes = FILE_ATTRIBUTE_READONLY;
        let ticks = 116_444_736_000_000_000_u64 + 12_345_678;
        data.ftLastWriteTime.dwHighDateTime = (ticks >> 32) as u32;
        data.ftLastWriteTime.dwLowDateTime = ticks as u32;
        let entry = Entry(data);
        assert_eq!(
            entry.file_name().encode_wide().collect::<Vec<_>>(),
            vec![0xd800]
        );
        let metadata = entry.metadata();
        assert!(metadata.is_file());
        assert!(metadata.permissions().readonly());
        assert_eq!(metadata.len(), (1_u64 << 32) + 23);
        assert_eq!(
            metadata.modified().unwrap().into_std(),
            UNIX_EPOCH + Duration::new(1, 234_567_800)
        );
    }

    #[test]
    fn junction_and_symbolic_link_facts_never_become_directories() {
        for tag in [
            IO_REPARSE_TAG_MOUNT_POINT,
            IO_REPARSE_TAG_SYMLINK,
            0x2000_0042,
        ] {
            let mut data = WIN32_FIND_DATAW::default();
            data.dwFileAttributes = FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY;
            data.dwReserved0 = tag;
            let metadata = Entry(data).metadata();
            assert!(metadata.is_symlink());
            assert!(!metadata.is_dir());
        }
    }
}
