use crate::transport::{Endpoint, command, err, ftp_error};
use connector_api::{
    CommitMode, StorageConnector, StorageError, StorageErrorKind as Kind, StorageFeature,
    StorageOperation as Op, WriteOptions,
};
use connector_staging::RemoteStage;
use std::{
    fs::File,
    io::{Read, Write},
};
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath};

const LOCK: &str = ".syncengine-lock";
impl Endpoint {
    fn lock(&self, cancel: &CancellationToken) -> Result<(), StorageError> {
        let path = ResourcePath::root();
        let mut held = self
            .owns_lock
            .lock()
            .map_err(|_| err(Kind::Conflict, Op::Write, &path))?;
        if *held {
            return Err(err(Kind::Conflict, Op::Write, &path));
        }
        let mut ftp = self.connect(Op::Write, &path, cancel)?;
        ftp.mkdir(LOCK).map_err(|_| {
            err(Kind::Conflict, Op::Write, &path)
                .with_message("verrou distant occupé ou inaccessible ; diagnostic requis")
        })?;
        *held = true;
        Ok(())
    }
    fn unlock(&self) -> Result<(), StorageError> {
        let path = ResourcePath::root();
        let mut held = self
            .owns_lock
            .lock()
            .map_err(|_| err(Kind::Conflict, Op::Write, &path))?;
        if !*held {
            return Ok(());
        }
        let mut ftp = self.connect(Op::Delete, &path, &CancellationToken::new())?;
        ftp.rmdir(LOCK)
            .map_err(|e| ftp_error(e, Op::Delete, &path))?;
        *held = false;
        Ok(())
    }
    fn remove_unlocked(
        &self,
        path: &ResourcePath,
        kind: EntryKind,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        if path.is_root() || self.stat(path, cancel)?.kind() != kind {
            return Err(err(Kind::Conflict, Op::Delete, path));
        }
        let mut ftp = self.connect(Op::Delete, path, cancel)?;
        match kind {
            EntryKind::File => ftp.rm(command(path, Op::Delete)?),
            EntryKind::Directory => ftp.rmdir(command(path, Op::Delete)?),
            _ => return Err(err(Kind::NotSupported, Op::Delete, path)),
        }
        .map_err(|e| ftp_error(e, Op::Delete, path))
    }
}
pub(super) fn mkdir(
    c: &Endpoint,
    path: &ResourcePath,
    cancel: &CancellationToken,
) -> Result<FileEntry, StorageError> {
    connector_api::ensure_supported(c, StorageFeature::CreateDirectory, Op::CreateDirectory)?;
    command(path, Op::CreateDirectory)?;
    if path.is_root() || path.as_str().split('/').any(|s| s == LOCK) {
        return Err(err(Kind::Conflict, Op::CreateDirectory, path));
    }
    c.lock(cancel)?;
    let result = (|| {
        let mut ftp = c.connect(Op::CreateDirectory, path, cancel)?;
        ftp.mkdir(command(path, Op::CreateDirectory)?)
            .map_err(|e| ftp_error(e, Op::CreateDirectory, path))?;
        c.stat(path, cancel)
    })();
    let cleanup = c.unlock();
    match (result, cleanup) {
        (Ok(v), Ok(())) => Ok(v),
        (Err(e), _) | (_, Err(e)) => Err(e),
    }
}
pub(super) fn delete(
    c: &Endpoint,
    path: &ResourcePath,
    kind: EntryKind,
    cancel: &CancellationToken,
) -> Result<(), StorageError> {
    connector_api::ensure_supported(c, StorageFeature::Delete, Op::Delete)?;
    if path.as_str().split('/').any(|s| s == LOCK) {
        return Err(err(Kind::Conflict, Op::Delete, path));
    }
    c.lock(cancel)?;
    let result = c.remove_unlocked(path, kind, cancel);
    let cleanup = c.unlock();
    result.and(cleanup)
}
impl RemoteStage for Endpoint {
    fn reserve(&self, path: &ResourcePath, cancel: &CancellationToken) -> Result<(), StorageError> {
        command(path, Op::Write)?;
        self.lock(cancel)?;
        let mut ftp = self.connect(Op::Write, path, cancel)?;
        // If creation fails or its reply is lost, retain the lock for diagnosis.
        ftp.mkdir(command(path, Op::Write)?)
            .map_err(|e| ftp_error(e, Op::Write, path))
    }
    fn upload(
        &self,
        path: &ResourcePath,
        mut file: File,
        _options: WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        let mut ftp = self.connect(Op::Write, path, cancel)?;
        let mut out = ftp
            .put_with_stream(command(path, Op::Write)?)
            .map_err(|e| ftp_error(e, Op::Write, path))?;
        let mut buf = vec![0; 256 * 1024];
        loop {
            crate::paging::check(cancel, Op::Write, path)?;
            let n = file
                .read(&mut buf)
                .map_err(|_| err(Kind::Permanent, Op::Read, path))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|_| err(Kind::Disconnected, Op::Write, path))?;
        }
        out.finish().map_err(|e| ftp_error(e, Op::Write, path))
    }
    fn publish(
        &self,
        stage: &ResourcePath,
        target: &ResourcePath,
        mode: CommitMode,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        // FTP lacks conditional RNTO. This check is valid ONLY with the server
        // root lock AND the caller's explicit exclusion of non-cooperating writers.
        match (mode, self.stat(target, cancel)) {
            (CommitMode::CreateNew, Err(e)) if e.kind() == Kind::NotFound => {}
            (CommitMode::ReplaceExisting, Ok(e)) if e.kind() == EntryKind::File => {}
            (_, Err(e)) => return Err(e),
            _ => return Err(err(Kind::Conflict, Op::Move, target)),
        }
        let mut ftp = self.connect(Op::Move, target, cancel)?;
        let previous = if mode == CommitMode::ReplaceExisting {
            let previous = stage
                .parent()
                .ok_or_else(|| err(Kind::Permanent, Op::Move, target))?
                .join("previous")
                .map_err(|_| err(Kind::Permanent, Op::Move, target))?;
            ftp.rename(command(target, Op::Move)?, command(&previous, Op::Move)?)
                .map_err(|e| ftp_error(e, Op::Move, target))?;
            Some(previous)
        } else {
            None
        };
        ftp.rename(command(stage, Op::Move)?, command(target, Op::Move)?)
            .map_err(|e| ftp_error(e, Op::Move, target).with_message("publication incertaine ; original préservé dans dépôt/previous ; diagnostic requis"))?;
        let entry = self.stat(target, cancel)?;
        if let Some(previous) = previous {
            self.remove_unlocked(&previous, EntryKind::File, cancel)?;
        }
        Ok(entry)
    }
    fn cleanup(&self, stage: &ResourcePath, dir: &ResourcePath) -> Result<(), StorageError> {
        let cancel = CancellationToken::new();
        match self.remove_unlocked(stage, EntryKind::File, &cancel) {
            Ok(()) => {}
            Err(e) if e.kind() == Kind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.remove_unlocked(dir, EntryKind::Directory, &cancel)?;
        self.unlock()
    }
}
