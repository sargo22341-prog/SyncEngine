//! Un upload ne doit pas être remis à l'exécuteur avant sa finalisation réussie.
use connector_api::{
    CommitMode, ContentReader, ContentWriter, ListPage, PageToken, StorageCapabilities,
    StorageConnector, StorageError, StorageErrorKind as Kind, StorageFeature,
    StorageOperation as Op, WriteOptions,
};
use connector_memory::{MemoryConnector, path, ts};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use sync_copy::{CopyRequest, CopyStage, stage_copy};
use sync_core::{CancellationToken, ConnectorId, FileEntry, ResourcePath};
fn error(kind: Kind) -> StorageError {
    StorageError::new(kind, Op::Write)
}
struct Destination {
    id: ConnectorId,
    caps: StorageCapabilities,
    aborted: Arc<AtomicBool>,
    published: Arc<AtomicBool>,
}
struct Upload {
    aborted: Arc<AtomicBool>,
    published: Arc<AtomicBool>,
    stage: ResourcePath,
}
impl ContentWriter for Upload {
    fn write(&mut self, _: &[u8]) -> Result<(), StorageError> {
        Ok(())
    }
    fn staging_path(&self) -> Option<&ResourcePath> {
        Some(&self.stage)
    }
    fn finish(&mut self) -> Result<(), StorageError> {
        Err(error(Kind::Disconnected))
    }
    fn commit(self: Box<Self>, _: CommitMode) -> Result<FileEntry, StorageError> {
        self.published.store(true, Ordering::SeqCst);
        Err(error(Kind::Permanent))
    }
    fn abort(self: Box<Self>) -> Result<(), StorageError> {
        self.aborted.store(true, Ordering::SeqCst);
        Ok(())
    }
}
impl StorageConnector for Destination {
    fn connector_id(&self) -> &ConnectorId {
        &self.id
    }
    fn capabilities(&self) -> &StorageCapabilities {
        &self.caps
    }
    fn list(
        &self,
        _: &ResourcePath,
        _: Option<&PageToken>,
        _: &CancellationToken,
    ) -> Result<ListPage, StorageError> {
        Err(error(Kind::NotSupported))
    }
    fn stat(&self, _: &ResourcePath, _: &CancellationToken) -> Result<FileEntry, StorageError> {
        Err(error(Kind::NotFound))
    }
    fn open_read(
        &self,
        _: &ResourcePath,
        _: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        Err(error(Kind::NotSupported))
    }
    fn open_write(
        &self,
        p: &ResourcePath,
        o: &WriteOptions,
        _: &CancellationToken,
    ) -> Result<Box<dyn ContentWriter>, StorageError> {
        if o.modified.is_some() {
            return Err(error(Kind::NotSupported));
        }
        Ok(Box::new(Upload {
            aborted: self.aborted.clone(),
            published: self.published.clone(),
            stage: p.clone(),
        }))
    }
}
#[test]
fn failed_finalization_aborts_before_exposing_or_publishing_the_stage()
-> Result<(), Box<dyn std::error::Error>> {
    let aborted = Arc::new(AtomicBool::new(false));
    let published = Arc::new(AtomicBool::new(false));
    let destination = Destination {
        id: ConnectorId::new("requires-finalization")?,
        caps: StorageCapabilities::none().with_features([StorageFeature::Write]),
        aborted: aborted.clone(),
        published: published.clone(),
    };
    let source = MemoryConnector::new("source").file("file", b"payload", ts(1));
    let at = path("file");
    let request = CopyRequest::new(&source, &at, &destination, &at)
        .with_options(WriteOptions::new().with_modified(Some(ts(1))));
    let err = stage_copy(&request, &mut |_| {}, &CancellationToken::new())
        .err()
        .ok_or("unfinished upload exposed as successful stage")?;
    assert_eq!(err.stage(), CopyStage::Write);
    assert_eq!(err.kind(), Kind::Disconnected);
    assert!(aborted.load(Ordering::SeqCst));
    assert!(!published.load(Ordering::SeqCst));
    assert!(err.cleanup_error().is_none());
    Ok(())
}
