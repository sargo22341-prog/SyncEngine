//! SYNCENGINE-PATCH: data-only flush without device cache flush.
//! Safety and compatibility review: SYNCENGINE-PATCH.md.
use std::os::windows::io::AsRawHandle;
use std::{fs, io};
use windows_sys::Wdk::Storage::FileSystem::NtFlushBuffersFileEx;
use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
use windows_sys::Win32::System::SystemServices::{FLUSH_FLAGS_FILE_DATA_ONLY, FLUSH_FLAGS_NO_SYNC};

/// Writes the cached data of `file` to its storage device and waits for that
/// write, without its metadata and without asking the device to empty its
/// own volatile cache (`NtFlushBuffersFileEx` with `FLUSH_FLAGS_FILE_DATA_ONLY
/// | FLUSH_FLAGS_NO_SYNC`, Windows 8 and later). It paces a writer: it is not
/// a durability barrier, `File::sync_all` still is. The handle needs write
/// access. File systems without this request (network redirectors, FAT…)
/// return an error, typically `ERROR_INVALID_PARAMETER` or
/// `ERROR_NOT_SUPPORTED`; callers fall back to `File::sync_data`.
pub fn flush_data_no_sync(file: &fs::File) -> io::Result<()> {
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the handle is owned by `file` and stays open for the call; no
    // parameter block is passed (null pointer with size 0, as documented);
    // `status_block` is an initialized local written by the call only.
    let status = unsafe {
        NtFlushBuffersFileEx(
            file.as_raw_handle(),
            FLUSH_FLAGS_FILE_DATA_ONLY | FLUSH_FLAGS_NO_SYNC,
            std::ptr::null(),
            0,
            &mut status_block,
        )
    };
    if status >= 0 {
        Ok(())
    } else {
        // SAFETY: pure conversion of a status code, no pointer involved.
        let code = unsafe { RtlNtStatusToDosError(status) };
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
