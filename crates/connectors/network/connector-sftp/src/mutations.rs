use crate::sftp::{SftpConnector, error, ssh_error};
use connector_api::{
    CommitMode, StorageConnector, StorageError, StorageErrorKind as Kind, StorageFeature,
    StorageOperation as Op, WriteOptions,
};
use connector_staging::RemoteStage;
use ssh2::{FileStat, OpenFlags, OpenType, RenameFlags};
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath};

fn parent(
    c: &SftpConnector,
    s: &ssh2::Sftp,
    path: &ResourcePath,
    op: Op,
    cancel: &CancellationToken,
) -> Result<PathBuf, StorageError> {
    let p = path
        .parent()
        .ok_or_else(|| error(Kind::Conflict, op, path))?;
    c.remote(s, &p, op, false, cancel)?;
    Ok(PathBuf::from(format!(
        "{}/{}",
        c.options.root.trim_end_matches('/'),
        path.as_str()
    )))
}

pub(super) fn mkdir(
    c: &SftpConnector,
    path: &ResourcePath,
    cancel: &CancellationToken,
) -> Result<FileEntry, StorageError> {
    connector_api::ensure_supported(c, StorageFeature::CreateDirectory, Op::CreateDirectory)?;
    let s = c.connect(Op::CreateDirectory, path, cancel)?;
    let remote = parent(c, &s, path, Op::CreateDirectory, cancel)?;
    match s.lstat(&remote) {
        Ok(_) => return Err(error(Kind::Conflict, Op::CreateDirectory, path)),
        Err(e) if matches!(e.code(), ssh2::ErrorCode::SFTP(2 | 10)) => {}
        Err(e) => return Err(ssh_error(e, Op::CreateDirectory, path)),
    }
    s.mkdir(&remote, 0o700)
        .map_err(|e| ssh_error(e, Op::CreateDirectory, path))?;
    c.stat(path, cancel)
}

pub(super) fn delete(
    c: &SftpConnector,
    path: &ResourcePath,
    expected: EntryKind,
    cancel: &CancellationToken,
) -> Result<(), StorageError> {
    connector_api::ensure_supported(c, StorageFeature::Delete, Op::Delete)?;
    if path.is_root() {
        return Err(error(Kind::Conflict, Op::Delete, path));
    }
    let s = c.connect(Op::Delete, path, cancel)?;
    let remote = c.remote(&s, path, Op::Delete, false, cancel)?;
    let st = s
        .lstat(&remote)
        .map_err(|e| ssh_error(e, Op::Delete, path))?;
    match expected {
        EntryKind::File if st.is_file() => s.unlink(&remote),
        EntryKind::Directory if st.is_dir() => s.rmdir(&remote),
        _ => return Err(error(Kind::Conflict, Op::Delete, path)),
    }
    .map_err(|e| ssh_error(e, Op::Delete, path))
}

impl RemoteStage for SftpConnector {
    fn reserve(&self, path: &ResourcePath, cancel: &CancellationToken) -> Result<(), StorageError> {
        mkdir(self, path, cancel).map(|_| ())
    }
    fn upload(
        &self,
        path: &ResourcePath,
        mut file: File,
        options: WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        let s = self.connect(Op::Write, path, cancel)?;
        let remote = parent(self, &s, path, Op::Write, cancel)?;
        let mut out = s
            .open_mode(
                &remote,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
                0o600,
                OpenType::File,
            )
            .map_err(|e| ssh_error(e, Op::Write, path))?;
        let mut buf = vec![0; 256 * 1024];
        loop {
            crate::paging::check(cancel, Op::Write, path)?;
            let n = file
                .read(&mut buf)
                .map_err(|_| error(Kind::Permanent, Op::Read, path))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .map_err(|_| error(Kind::Disconnected, Op::Write, path))?;
        }
        out.close().map_err(|e| ssh_error(e, Op::Write, path))?;
        if let Some(ts) = options.modified {
            let seconds =
                u64::try_from(ts.secs()).map_err(|_| error(Kind::NotSupported, Op::Write, path))?;
            s.setstat(
                &remote,
                FileStat {
                    size: None,
                    uid: None,
                    gid: None,
                    perm: None,
                    atime: Some(seconds),
                    mtime: Some(seconds),
                },
            )
            .map_err(|e| ssh_error(e, Op::Write, path))?;
        }
        Ok(())
    }
    fn publish(
        &self,
        stage: &ResourcePath,
        target: &ResourcePath,
        mode: CommitMode,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        let s = self.connect(Op::Move, target, cancel)?;
        let from = self.remote(&s, stage, Op::Move, false, cancel)?;
        let to = parent(self, &s, target, Op::Move, cancel)?;
        let previous = match mode {
            CommitMode::CreateNew => None,
            CommitMode::ReplaceExisting => {
                if !s
                    .lstat(&to)
                    .map_err(|e| ssh_error(e, Op::Move, target))?
                    .is_file()
                {
                    return Err(error(Kind::Conflict, Op::Move, target));
                }
                // SFTP v3 rename cannot portably overwrite. Preserve the old
                // file in our reservation before publishing; never unlink it first.
                let previous = from
                    .parent()
                    .ok_or_else(|| error(Kind::Permanent, Op::Move, target))?
                    .join("previous");
                s.rename(&to, &previous, Some(RenameFlags::empty()))
                    .map_err(|e| ssh_error(e, Op::Move, target))?;
                Some(previous)
            }
            _ => return Err(error(Kind::NotSupported, Op::Move, target)),
        };
        s.rename(&from, &to, Some(RenameFlags::empty()))
            .map_err(|e| ssh_error(e, Op::Move, target).with_message("publication interrompue ; original conservé sous le dépôt/previous, diagnostic requis"))?;
        let result = self.stat(target, cancel)?;
        if let Some(previous) = previous {
            if !s
                .lstat(&previous)
                .map_err(|e| ssh_error(e, Op::Move, target))?
                .is_file()
            {
                return Err(error(Kind::Conflict, Op::Move, target));
            }
            s.unlink(&previous)
                .map_err(|e| ssh_error(e, Op::Delete, target))?;
        }
        Ok(result)
    }
    fn cleanup(&self, stage: &ResourcePath, dir: &ResourcePath) -> Result<(), StorageError> {
        let cancel = CancellationToken::new();
        match delete(self, stage, EntryKind::File, &cancel) {
            Ok(()) => {}
            Err(e) if e.kind() == Kind::NotFound => {}
            Err(e) => return Err(e),
        }
        delete(self, dir, EntryKind::Directory, &cancel)
    }
}
