//! A bounded native directory iterator, with single-use instance-bound tokens.
use crate::{
    confined,
    errors::{io_error, local_error},
    paths::is_ambiguous_here,
};
use cap_std::fs::{Dir, ReadDir};
use connector_api::{
    ListPage, ListingIssue, ListingIssueKind, NameIssue, PageToken, StorageError, StorageErrorKind,
    StorageOperation,
};
#[cfg(windows)]
use std::sync::atomic::AtomicBool;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use sync_core::{CancellationToken, ResourcePath, TimestampPrecision};
static GENERATION: AtomicU64 = AtomicU64::new(1);
const PAGE_SIZE: usize = 256;
#[derive(Debug, Default)]
pub(crate) struct Pages(Mutex<Option<Cursor>>, #[cfg(windows)] AtomicBool);
#[derive(Debug)]
struct Cursor {
    path: ResourcePath,
    token: String,
    reader: std::iter::Peekable<DirectoryReader>,
}

#[derive(Debug)]
enum DirectoryReader {
    Standard(ReadDir),
    #[cfg(windows)]
    Bulk(cap_primitives::windows_listing::HandleReadDir),
}
#[derive(Debug)]
enum ListedEntry {
    Standard(cap_std::fs::DirEntry),
    #[cfg(windows)]
    Bulk(cap_primitives::windows_listing::Entry),
}
impl Iterator for DirectoryReader {
    type Item = std::io::Result<ListedEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Standard(reader) => reader.next().map(|r| r.map(ListedEntry::Standard)),
            #[cfg(windows)]
            Self::Bulk(reader) => reader.next().map(|r| r.map(ListedEntry::Bulk)),
        }
    }
}
impl ListedEntry {
    fn file_name(&self) -> std::ffi::OsString {
        match self {
            Self::Standard(entry) => entry.file_name(),
            #[cfg(windows)]
            Self::Bulk(entry) => entry.file_name(),
        }
    }
    fn metadata(&self) -> std::io::Result<cap_std::fs::Metadata> {
        match self {
            Self::Standard(entry) => entry.metadata(),
            #[cfg(windows)]
            Self::Bulk(entry) => Ok(entry.metadata()),
        }
    }
}
impl Pages {
    pub(crate) fn list(
        &self,
        root: &Arc<Dir>,
        handles: Option<&confined::Handles>,
        path: &ResourcePath,
        token: Option<&PageToken>,
        precision: TimestampPrecision,
        cancel: &CancellationToken,
    ) -> Result<ListPage, StorageError> {
        let op = StorageOperation::List;
        let invalid = || {
            local_error(
                StorageErrorKind::Conflict,
                op,
                path,
                "invalid or consumed directory cursor",
            )
        };
        let mut slot = self.0.lock().map_err(|_| invalid())?;
        if let Some(token) = token {
            if !slot
                .as_ref()
                .is_some_and(|c| c.path == *path && c.token == token.as_str())
            {
                return Err(invalid());
            }
        } else {
            *slot = None;
            let dir = confined::directory_via(root, handles, path, op)?;
            #[cfg(windows)]
            let reader = if handles.is_some() {
                let owned = dir
                    .try_clone()
                    .map_err(|e| io_error(e, op, path))?
                    .into_std_file();
                let reader = if self.1.load(Ordering::Relaxed) {
                    cap_primitives::windows_listing::HandleReadDir::Legacy(
                        cap_primitives::windows_listing::ReadDir::new(owned)
                            .map_err(|e| io_error(e, op, path))?,
                    )
                } else {
                    cap_primitives::windows_listing::HandleReadDir::new(owned)
                        .map_err(|e| io_error(e, op, path))?
                };
                // One capability probe per connector, not one failed request
                // per directory on older SMB servers/filesystems.
                if !reader.uses_handle_batches() {
                    self.1.store(true, Ordering::Relaxed);
                }
                DirectoryReader::Bulk(reader)
            } else {
                DirectoryReader::Standard(dir.entries().map_err(|e| io_error(e, op, path))?)
            };
            #[cfg(not(windows))]
            let reader =
                DirectoryReader::Standard(dir.entries().map_err(|e| io_error(e, op, path))?);
            let reader = reader.peekable();
            *slot = Some(Cursor {
                path: path.clone(),
                token: String::new(),
                reader,
            });
        }
        // Remove the cursor while reading: failures and cancellations invalidate
        // it, never returning a successful truncated directory.
        let mut cursor = slot.take().ok_or_else(invalid)?;
        let mut entries = Vec::new();
        let mut issues = Vec::new();
        for item in cursor.reader.by_ref().take(PAGE_SIZE) {
            if cancel.is_cancelled() {
                return Err(crate::connector::cancelled(op, path));
            }
            let item = item.map_err(|e| io_error(e, op, path))?;
            let os_name = item.file_name();
            let Some(name) = os_name.to_str() else {
                issues.push(ListingIssue::new(
                    os_name.to_string_lossy(),
                    ListingIssueKind::UnrepresentableName(NameIssue::NonUtf8),
                ));
                continue;
            };
            if is_ambiguous_here(name) {
                issues.push(ListingIssue::new(
                    name,
                    ListingIssueKind::UnrepresentableName(NameIssue::PlatformAmbiguous),
                ));
                continue;
            }
            let child = match path.join(name) {
                Ok(child) => child,
                Err(e) => {
                    issues.push(ListingIssue::new(
                        name,
                        ListingIssueKind::UnrepresentableName(NameIssue::Invalid(e)),
                    ));
                    continue;
                }
            };
            let meta = item.metadata().map_err(|e| io_error(e, op, &child))?;
            entries.push(confined::entry(child, &meta, precision));
        }
        let next = if cursor.reader.peek().is_some() {
            cursor.token = format!("local:{}", GENERATION.fetch_add(1, Ordering::Relaxed));
            let token = PageToken::new(cursor.token.clone());
            *slot = Some(cursor);
            Some(token)
        } else {
            None
        };
        Ok(ListPage::new(entries, issues, next))
    }
}
