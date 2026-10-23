use std::collections::VecDeque;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use connector_api::{
    ListPage, PageToken, Pagination, StorageError, StorageErrorKind as Kind, StorageOperation as Op,
};
use sync_core::{CancellationToken, FileEntry, ResourcePath};

static GENERATION: AtomicU64 = AtomicU64::new(1);
const PAGE_SIZE: usize = 256;

#[derive(Default)]
pub(super) struct Pages(Mutex<Option<Snapshot>>);
struct Snapshot {
    path: ResourcePath,
    token: String,
    entries: VecDeque<FileEntry>,
}

fn error(path: &ResourcePath) -> StorageError {
    StorageError::new(Kind::Conflict, Op::List).with_path(path.clone())
}

impl Pages {
    pub(super) fn clear(&self, path: &ResourcePath) -> Result<(), StorageError> {
        *self.0.lock().map_err(|_| error(path))? = None;
        Ok(())
    }
    pub(super) fn capability() -> Pagination {
        Pagination::Paginated {
            max_page_size: Some(PAGE_SIZE as u32),
        }
    }
    pub(super) fn start(
        &self,
        path: &ResourcePath,
        mut entries: Vec<FileEntry>,
    ) -> Result<ListPage, StorageError> {
        entries.sort_by(|a, b| a.path().cmp(b.path()));
        if entries.windows(2).any(|w| w[0].path() == w[1].path()) {
            return Err(error(path));
        }
        let mut slot = self.0.lock().map_err(|_| error(path))?;
        *slot = Some(Snapshot {
            path: path.clone(),
            token: String::new(),
            entries: entries.into(),
        });
        Self::take(&mut slot, path)
    }
    pub(super) fn next(
        &self,
        path: &ResourcePath,
        token: &PageToken,
    ) -> Result<ListPage, StorageError> {
        let mut slot = self.0.lock().map_err(|_| error(path))?;
        let valid = slot
            .as_ref()
            .is_some_and(|s| s.path == *path && s.token == token.as_str());
        if !valid {
            return Err(error(path));
        }
        Self::take(&mut slot, path)
    }
    fn take(slot: &mut Option<Snapshot>, path: &ResourcePath) -> Result<ListPage, StorageError> {
        let state = slot.as_mut().ok_or_else(|| error(path))?;
        let count = PAGE_SIZE.min(state.entries.len());
        let entries = state.entries.drain(..count).collect();
        let next = if state.entries.is_empty() {
            *slot = None;
            None
        } else {
            state.token = format!("sftp:{}", GENERATION.fetch_add(1, Ordering::Relaxed));
            Some(PageToken::new(state.token.clone()))
        };
        Ok(ListPage::new(entries, vec![], next))
    }
}

pub(super) fn check(
    cancel: &CancellationToken,
    op: Op,
    path: &ResourcePath,
) -> Result<(), StorageError> {
    if cancel.is_cancelled() {
        Err(StorageError::new(Kind::Cancelled, op).with_path(path.clone()))
    } else {
        Ok(())
    }
}

pub(super) fn backoff(
    cancel: &CancellationToken,
    op: Op,
    path: &ResourcePath,
    delay: Duration,
) -> Result<(), StorageError> {
    let start = std::time::Instant::now();
    while start.elapsed() < delay {
        check(cancel, op, path)?;
        std::thread::sleep(Duration::from_millis(20).min(delay.saturating_sub(start.elapsed())));
    }
    check(cancel, op, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sync_core::{EntryKind, FileMetadata};
    #[test]
    fn pages_are_bound_single_use_and_sorted() -> Result<(), Box<dyn std::error::Error>> {
        let pages = Pages::default();
        let root = ResourcePath::root();
        let entries = (0..600)
            .rev()
            .map(|n| {
                Ok(FileEntry::new(
                    ResourcePath::parse(&format!("f{n:04}"))?,
                    EntryKind::File,
                    FileMetadata::unknown(),
                ))
            })
            .collect::<Result<Vec<_>, sync_core::PathError>>()?;
        let first = pages.start(&root, entries)?;
        assert_eq!(first.entries().len(), 256);
        assert_eq!(first.entries()[0].path().as_str(), "f0000");
        let token = first.next_page().ok_or("missing token")?;
        assert!(pages.next(&ResourcePath::parse("other")?, token).is_err());
        assert!(Pages::default().next(&root, token).is_err());
        let second = pages.next(&root, token)?;
        assert!(pages.next(&root, token).is_err());
        let last = pages.next(&root, second.next_page().ok_or("missing token")?)?;
        assert_eq!(last.entries().len(), 88);
        assert!(last.next_page().is_none());
        Ok(())
    }
    #[test]
    fn empty_duplicates_and_cancel() -> Result<(), Box<dyn std::error::Error>> {
        let root = ResourcePath::root();
        assert!(Pages::default().start(&root, vec![])?.entries().is_empty());
        let e = FileEntry::new(
            ResourcePath::parse("a")?,
            EntryKind::File,
            FileMetadata::unknown(),
        );
        assert!(Pages::default().start(&root, vec![e.clone(), e]).is_err());
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            backoff(&cancel, Op::List, &root, Duration::from_secs(30))
                .err()
                .ok_or("missing error")?
                .kind(),
            Kind::Cancelled
        );
        Ok(())
    }
}
