//! SYNCENGINE-PATCH: directory batches directly from the confined handle.
//! No normalized path lookup. Safety review: SYNCENGINE-PATCH.md.
use super::large_read_dir::{Entry, ReadDir};
use std::mem::offset_of;
use std::os::windows::io::AsRawHandle;
use std::{fs, io};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
    ERROR_NO_MORE_FILES,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileFullDirectoryInfo, FileFullDirectoryRestartInfo, FileIdExtdDirectoryInfo,
    FileIdExtdDirectoryRestartInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FULL_DIR_INFO, FILE_ID_EXTD_DIR_INFO as Info, WIN32_FIND_DATAW,
};

const BUFFER_BYTES: usize = 64 * 1024;
const NAME: usize = offset_of!(Info, FileName);

/// Opens the currently named root and reads its directory attribute, volume
/// serial number and file index in one query. The root itself may be a link.
/// Requires ambient authority because `root` is an OS path, not a capability.
pub fn root_identity(
    root: &std::path::Path,
    authority: crate::AmbientAuthority,
) -> io::Result<(u64, u64)> {
    query_root_identity(root, authority, false)
}

/// Same identity check with the original execution GENERIC_READ requirement.
pub fn readable_root_identity(
    root: &std::path::Path,
    authority: crate::AmbientAuthority,
) -> io::Result<(u64, u64)> {
    query_root_identity(root, authority, true)
}

fn query_root_identity(
    root: &std::path::Path,
    authority: crate::AmbientAuthority,
    readable: bool,
) -> io::Result<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let _ = authority;
    let mut options = fs::OpenOptions::new();
    if readable {
        options.read(true);
    } else {
        options.access_mode(FILE_READ_ATTRIBUTES);
    }
    let file = options
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(root)?;
    let info = winx::winapi_util::file::information(&file)?;
    if info.file_attributes() & u64::from(FILE_ATTRIBUTE_DIRECTORY) == 0 {
        return Err(io::Error::from(io::ErrorKind::NotADirectory));
    }
    Ok((info.volume_serial_number(), info.file_index()))
}

/// Exhaustive handle enumeration with a bounded 64 KiB, eight-byte aligned buffer.
/// The caller must not enumerate another duplicated handle concurrently: Windows
/// enumeration state belongs to the underlying file object. Each new cursor
/// restarts enumeration. Unsupported info classes fall back before yielding data.
#[derive(Debug)]
pub enum HandleReadDir {
    /// Native batches from the already opened directory.
    Direct(Batches),
    /// Compatibility iterator when the information class is unsupported.
    Legacy(ReadDir),
}
impl HandleReadDir {
    /// Takes ownership of an already confined readable directory handle.
    /// Access, network and malformed-data errors are never fallback triggers.
    pub fn new(directory: fs::File) -> io::Result<Self> {
        Self::with_format(directory, false)
    }

    /// Enumerates without requesting file identities, which some SMB servers
    /// do not support. Reparse entries receive a confined no-follow metadata
    /// query because FILE_FULL_DIR_INFO does not contain their tag.
    pub fn new_without_file_ids(directory: fs::File) -> io::Result<Self> {
        Self::with_format(directory, true)
    }

    fn with_format(directory: fs::File, full: bool) -> io::Result<Self> {
        let mut batches = Batches {
            directory,
            full,
            buffer: vec![0_u64; BUFFER_BYTES / 8],
            next: None,
            finished: false,
        };
        match batches.fetch(true) {
            Ok(()) => Ok(Self::Direct(batches)),
            Err(e) if unsupported(&e) => Ok(Self::Legacy(ReadDir::new(batches.directory)?)),
            Err(e) => Err(e),
        }
    }

    /// True when the server supports direct handle batches (no path conversion).
    pub fn uses_handle_batches(&self) -> bool {
        matches!(self, Self::Direct(_))
    }
}
fn unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error().map(|v| v as u32),
        Some(ERROR_INVALID_FUNCTION | ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED)
    )
}
impl Iterator for HandleReadDir {
    type Item = io::Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Direct(reader) => reader.next(),
            Self::Legacy(reader) => reader.next(),
        }
    }
}

/// Internal state exposed only as an opaque variant payload.
pub struct Batches {
    directory: fs::File,
    full: bool,
    buffer: Vec<u64>,
    next: Option<usize>,
    finished: bool,
}
impl std::fmt::Debug for Batches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryBatches")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}
impl Batches {
    fn fetch(&mut self, restart: bool) -> io::Result<()> {
        self.buffer.fill(0);
        // SAFETY: the owned directory handle is alive; Vec<u64> provides the
        // required eight-byte alignment and BUFFER_BYTES writable bytes. The
        // synchronous API does not retain the pointer beyond the call.
        let success = unsafe {
            GetFileInformationByHandleEx(
                self.directory.as_raw_handle() as _,
                if self.full && restart {
                    FileFullDirectoryRestartInfo
                } else if self.full {
                    FileFullDirectoryInfo
                } else if restart {
                    FileIdExtdDirectoryRestartInfo
                } else {
                    FileIdExtdDirectoryInfo
                },
                self.buffer.as_mut_ptr().cast(),
                BUFFER_BYTES as u32,
            )
        };
        if success == 0 {
            // SAFETY: capture immediately, before any other Windows call.
            let code = unsafe { GetLastError() };
            self.next = None;
            if code == ERROR_NO_MORE_FILES {
                self.finished = true;
                return Ok(());
            }
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        self.next = Some(0);
        Ok(())
    }
}
impl Iterator for Batches {
    type Item = io::Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.finished {
                return None;
            }
            if self.next.is_none() {
                if let Err(e) = self.fetch(false) {
                    self.finished = true;
                    return Some(Err(e));
                }
                if self.finished {
                    return None;
                }
            }
            let at = self.next.take()?;
            // SAFETY: every u64 byte is initialized, including unused tail;
            // byte views have alignment one, live only while self is borrowed,
            // and all variable offsets/lengths are checked by parse_entry.
            let bytes = unsafe {
                std::slice::from_raw_parts(self.buffer.as_ptr().cast::<u8>(), BUFFER_BYTES)
            };
            match parse_record(bytes, at, self.full) {
                Ok((mut entry, next)) => {
                    self.next = next;
                    let name = entry.file_name();
                    if name != "." && name != ".." {
                        if self.full && entry.0.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                        {
                            // The name was validated as one component. Never follow
                            // a reparse point merely to recover its classification.
                            match crate::fs::stat(
                                &self.directory,
                                std::path::Path::new(&name),
                                crate::fs::FollowSymlinks::No,
                            ) {
                                Ok(meta) => {
                                    entry.0.dwReserved0 = if meta.file_type().is_symlink() {
                                        0xa000000c
                                    } else {
                                        0
                                    };
                                }
                                Err(error) => {
                                    self.finished = true;
                                    return Some(Err(error));
                                }
                            }
                        }
                        return Some(Ok(entry));
                    }
                }
                Err(e) => {
                    self.finished = true;
                    return Some(Err(e));
                }
            }
        }
    }
}
fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}
fn read<const N: usize>(bytes: &[u8], at: usize) -> io::Result<[u8; N]> {
    let slice = bytes
        .get(at..at.checked_add(N).ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let mut result = [0; N];
    result.copy_from_slice(slice);
    Ok(result)
}
#[cfg(test)]
fn parse_entry(bytes: &[u8], at: usize) -> io::Result<(Entry, Option<usize>)> {
    parse_record(bytes, at, false)
}
fn parse_record(bytes: &[u8], at: usize, full: bool) -> io::Result<(Entry, Option<usize>)> {
    if at % 8 != 0 {
        return Err(invalid());
    }
    let bytes = bytes.get(at..).ok_or_else(invalid)?;
    let next = u32::from_le_bytes(read(bytes, offset_of!(Info, NextEntryOffset))?) as usize;
    let len = u32::from_le_bytes(read(bytes, offset_of!(Info, FileNameLength))?) as usize;
    let name_at = if full {
        offset_of!(FILE_FULL_DIR_INFO, FileName)
    } else {
        NAME
    };
    let end = name_at.checked_add(len).ok_or_else(invalid)?;
    if len == 0
        || len % 2 != 0
        || len / 2 >= 260
        || end > bytes.len()
        || (next != 0 && (next % 8 != 0 || next < end || next >= bytes.len()))
    {
        return Err(invalid());
    }
    let mut data = WIN32_FIND_DATAW::default();
    for (i, pair) in bytes[name_at..end].chunks_exact(2).enumerate() {
        let ch = u16::from_le_bytes([pair[0], pair[1]]);
        if ch == 0 || ch == b'/' as u16 || ch == b'\\' as u16 {
            return Err(invalid());
        }
        data.cFileName[i] = ch;
    }
    data.dwFileAttributes = u32::from_le_bytes(read(bytes, offset_of!(Info, FileAttributes))?);
    if !full {
        data.dwReserved0 = u32::from_le_bytes(read(bytes, offset_of!(Info, ReparsePointTag))?);
    }
    let size = i64::from_le_bytes(read(bytes, offset_of!(Info, EndOfFile))?);
    if size < 0 {
        return Err(invalid());
    }
    data.nFileSizeHigh = ((size as u64) >> 32) as u32;
    data.nFileSizeLow = size as u32;
    let filetime = |offset| -> io::Result<_> {
        let ticks = u64::from_le_bytes(read(bytes, offset)?);
        Ok(windows_sys::Win32::Foundation::FILETIME {
            dwHighDateTime: (ticks >> 32) as u32,
            dwLowDateTime: ticks as u32,
        })
    };
    data.ftCreationTime = filetime(offset_of!(Info, CreationTime))?;
    data.ftLastWriteTime = filetime(offset_of!(Info, LastWriteTime))?;
    data.ftLastAccessTime = filetime(offset_of!(Info, LastAccessTime))?;
    Ok((
        Entry(data),
        if next == 0 {
            None
        } else {
            Some(at.checked_add(next).ok_or_else(invalid)?)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    fn record(name: &[u16], next: u32) -> Vec<u8> {
        let mut bytes = vec![0; (NAME + name.len() * 2 + 7) & !7];
        bytes[..4].copy_from_slice(&next.to_le_bytes());
        bytes[offset_of!(Info, FileNameLength)..][..4]
            .copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
        for (i, ch) in name.iter().enumerate() {
            bytes[NAME + i * 2..][..2].copy_from_slice(&ch.to_le_bytes());
        }
        bytes
    }
    fn put(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    #[test]
    fn full_records_use_the_shorter_name_offset_and_validate_the_same_bounds() {
        // Shared fixed fields end at EaSize. FileName starts sooner in FULL;
        // reading it at the EXTD offset would silently corrupt real NAS names.
        assert_eq!(
            offset_of!(Info, EaSize),
            offset_of!(FILE_FULL_DIR_INFO, EaSize)
        );
        let name_at = offset_of!(FILE_FULL_DIR_INFO, FileName);
        for name in [vec![0x00e9, 0xd800], vec![b'a' as u16; 259]] {
            let mut bytes = vec![0; (name_at + name.len() * 2 + 7) & !7];
            put(
                &mut bytes,
                offset_of!(Info, FileNameLength),
                (name.len() * 2) as u32,
            );
            for (i, ch) in name.iter().enumerate() {
                bytes[name_at + i * 2..][..2].copy_from_slice(&ch.to_le_bytes());
            }
            let size = (1_i64 << 40) + 17;
            bytes[offset_of!(Info, EndOfFile)..][..8].copy_from_slice(&size.to_le_bytes());
            let (entry, next) = parse_record(&bytes, 0, true).unwrap();
            use std::os::windows::ffi::OsStrExt;
            assert_eq!(entry.file_name().encode_wide().collect::<Vec<_>>(), name);
            assert_eq!(entry.metadata().len(), size as u64);
            assert!(next.is_none());
            for (field, value) in [
                (offset_of!(Info, FileNameLength), 1),
                (offset_of!(Info, NextEntryOffset), 8),
                (offset_of!(Info, FileNameLength), u32::MAX),
            ] {
                let mut corrupt = bytes.clone();
                put(&mut corrupt, field, value);
                assert!(parse_record(&corrupt, 0, true).is_err());
            }
            for len in 0..name_at + name.len() * 2 {
                assert!(parse_record(&bytes[..len], 0, true).is_err());
            }
        }
    }
    #[test]
    fn records_preserve_names_large_sizes_times_and_reparse_tags() {
        let mut bytes = record(&[0x00e9, 0xd800], 0);
        let size = (1_i64 << 40) + 17;
        bytes[offset_of!(Info, EndOfFile)..][..8].copy_from_slice(&size.to_le_bytes());
        put(
            &mut bytes,
            offset_of!(Info, FileAttributes),
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
        );
        put(&mut bytes, offset_of!(Info, ReparsePointTag), 0xa0000003);
        let ticks = 116_444_736_000_000_000_u64 + 23_456_789;
        bytes[offset_of!(Info, LastWriteTime)..][..8].copy_from_slice(&ticks.to_le_bytes());
        let (entry, next) = parse_entry(&bytes, 0).unwrap();
        use std::os::windows::ffi::OsStrExt;
        assert_eq!(
            entry.file_name().encode_wide().collect::<Vec<_>>(),
            vec![0x00e9, 0xd800]
        );
        assert!(next.is_none());
        let meta = entry.metadata();
        assert!(meta.is_symlink());
        assert!(!meta.is_dir());
        assert_eq!(meta.len(), size as u64);
        assert_eq!(
            meta.modified().unwrap().into_std(),
            std::time::UNIX_EPOCH + std::time::Duration::new(2, 345_678_900)
        );
    }
    #[test]
    fn corrupt_lengths_offsets_and_names_fail_closed() {
        for (field, value) in [
            (offset_of!(Info, FileNameLength), u32::MAX),
            (offset_of!(Info, FileNameLength), 1),
            (offset_of!(Info, FileNameLength), 0),
            (offset_of!(Info, NextEntryOffset), u32::MAX),
            (offset_of!(Info, NextEntryOffset), 8),
            (offset_of!(Info, NextEntryOffset), 101),
        ] {
            let mut bytes = record(&[b'a' as u16], 0);
            put(&mut bytes, field, value);
            assert!(parse_entry(&bytes, 0).is_err());
        }
        for name in [
            vec![0],
            vec![b'/' as u16],
            vec![b'\\' as u16],
            vec![b'a' as u16; 260],
        ] {
            assert!(parse_entry(&record(&name, 0), 0).is_err());
        }
        let mut bytes = record(&[b'a' as u16], 0);
        bytes[offset_of!(Info, EndOfFile)..][..8].copy_from_slice(&(-1_i64).to_le_bytes());
        assert!(parse_entry(&bytes, 0).is_err());
        for n in 0..NAME + 2 {
            assert!(parse_entry(&bytes[..n], 0).is_err());
        }
        assert!(parse_entry(&bytes, 1).is_err());
    }
    #[test]
    fn fallback_never_masks_access_or_network_errors() {
        for code in [
            ERROR_INVALID_FUNCTION,
            ERROR_INVALID_PARAMETER,
            ERROR_NOT_SUPPORTED,
        ] {
            assert!(unsupported(&io::Error::from_raw_os_error(code as i32)));
        }
        for code in [5, 21, 53, 64, 121, 1231] {
            assert!(!unsupported(&io::Error::from_raw_os_error(code)));
        }
    }
    #[test]
    #[ignore = "performance: one million synthetic records, no filesystem or NAS"]
    fn million_records_in_bounded_batches() {
        let one = record(&[b'a' as u16], 0);
        let count = 500;
        let mut batch = Vec::new();
        for i in 0..count {
            let mut bytes = one.clone();
            put(
                &mut bytes,
                0,
                if i == count - 1 { 0 } else { one.len() as u32 },
            );
            batch.extend(bytes);
        }
        assert!(batch.len() <= BUFFER_BYTES);
        let start = std::time::Instant::now();
        let mut seen = 0;
        for _ in 0..2000 {
            let mut next = Some(0);
            while let Some(at) = next {
                let (entry, following) = parse_entry(std::hint::black_box(&batch), at).unwrap();
                assert!(entry.metadata().is_file());
                seen += 1;
                next = following;
            }
        }
        assert_eq!(seen, 1_000_000);
        eprintln!(
            "synthetic_records={seen} buffer_bytes={} elapsed={:?}",
            batch.len(),
            start.elapsed()
        );
    }
}
