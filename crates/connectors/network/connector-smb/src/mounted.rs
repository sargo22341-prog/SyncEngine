use connector_api::{
    ContentReader, ContentWriter, ListPage, PageToken, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind as Kind, StorageOperation as Op, WriteOptions,
};
use connector_local::{LocalConnector, LocalOptions};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use sync_core::{CancellationToken, ConnectorId, EntryKind, FileEntry, ResourcePath};

/// Partage SMB déjà authentifié par le système : UNC Windows ou CIFS Linux.
/// Aucun montage, mot de passe ou repli vers un répertoire local n'est effectué.
/// Linux est en lecture seule, afin qu'un démontage ne puisse rediriger un cleanup
/// de dépôt vers le répertoire local sous-jacent. Windows UNC permet les mutations.
pub struct SmbConnector {
    local: LocalConnector,
    root: PathBuf,
    id: ConnectorId,
    caps: StorageCapabilities,
    preview_scan: bool,
    #[cfg(target_os = "linux")]
    mount: String,
}
fn err(kind: Kind, op: Op, path: &ResourcePath) -> StorageError {
    StorageError::new(kind, op).with_path(path.clone())
}
impl SmbConnector {
    /// Ouvre un partage existant ; `mutations` est un opt-in explicite Windows.
    /// Le système doit imposer SMB signé/chiffré et gérer les identifiants.
    /// # Errors
    /// `NotSupported` pour chemin non UNC/CIFS ou mutations Linux ;
    /// `Disconnected` pour partage/montage absent, `Permanent` pour configuration invalide.
    pub fn new(root: impl Into<PathBuf>, mutations: bool) -> Result<Self, StorageError> {
        Self::open(root.into(), mutations, false)
    }

    /// Opens a read-only preview scanner with at most 64 cached directory handles.
    /// Execution must use `new` to re-walk paths before validating mutations.
    /// Listing uncertainty and mount checks are unchanged.
    /// # Errors
    /// Same root, platform and availability errors as `new`.
    pub fn for_scan(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        Self::open(root.into(), false, true)
    }

    fn open(root: PathBuf, mutations: bool, reuse_handles: bool) -> Result<Self, StorageError> {
        let path = ResourcePath::root();
        #[cfg(windows)]
        {
            use std::path::{Component, Prefix};
            if !matches!(root.components().next(),Some(Component::Prefix(p)) if matches!(p.kind(),Prefix::UNC(_,_)|Prefix::VerbatimUNC(_,_)))
            {
                return Err(err(Kind::NotSupported, Op::Connect, &path));
            }
        }
        #[cfg(target_os = "linux")]
        let mount = {
            if mutations {
                return Err(err(Kind::NotSupported, Op::Connect, &path));
            }
            mount_identity(&root)?
        };
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = mutations;
            return Err(err(Kind::NotSupported, Op::Connect, &path));
        }
        let options = local_options(mutations, reuse_handles);
        let local = LocalConnector::new(&root, options)
            .map_err(|_| err(Kind::Disconnected, Op::Connect, &path))?;
        let id = ConnectorId::new("smb").map_err(|_| err(Kind::Permanent, Op::Connect, &path))?;
        let mut caps = local.capabilities().clone();
        caps.case_sensitivity = connector_api::CaseSensitivity::Unknown;
        caps.timestamp_precision = sync_core::TimestampPrecision::Unknown;
        caps.clock = connector_api::ClockSource::Server;
        caps.listing_consistency = connector_api::ListingConsistency::Unknown;
        let result = Self {
            local,
            root,
            id,
            caps,
            preview_scan: reuse_handles,
            #[cfg(target_os = "linux")]
            mount,
        };
        result.guard(Op::Connect, &path)?;
        Ok(result)
    }
    fn guard(&self, op: Op, path: &ResourcePath) -> Result<(), StorageError> {
        match std::fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(err(Kind::Conflict, op, path)),
            Err(e) => {
                let kind = if e.kind() == std::io::ErrorKind::PermissionDenied {
                    Kind::PermissionDenied
                } else {
                    Kind::Disconnected
                };
                return Err(err(kind, op, path));
            }
        }
        #[cfg(target_os = "linux")]
        if mount_identity(&self.root).map_err(|_| err(Kind::Disconnected, op, path))? != self.mount
        {
            return Err(err(Kind::Disconnected, op, path));
        }
        Ok(())
    }

    fn read_guard(&self, op: Op, path: &ResourcePath) -> Result<(), StorageError> {
        // On Windows the local delegate already performs identity/availability
        // checks from fresh root handles. Do not repeat a second network stat.
        // Linux must still validate its CIFS mount identity on every access.
        if self.preview_scan && cfg!(windows) {
            Ok(())
        } else {
            self.guard(op, path)
        }
    }
}

fn local_options(mutations: bool, reuse_handles: bool) -> LocalOptions {
    if mutations {
        LocalOptions::new().with_mutations()
    } else if reuse_handles {
        LocalOptions::new().with_handle_reuse()
    } else {
        LocalOptions::new()
    }
}

#[cfg(target_os = "linux")]
fn mount_identity(root: &Path) -> Result<String, StorageError> {
    let path = ResourcePath::root();
    let canonical =
        std::fs::canonicalize(root).map_err(|_| err(Kind::Disconnected, Op::Connect, &path))?;
    let table = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|_| err(Kind::Disconnected, Op::Connect, &path))?;
    let mut selected: Option<(usize, String)> = None;
    for line in table.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        if !matches!(right.split_whitespace().next(), Some("cifs" | "smb3")) {
            continue;
        }
        let Some(raw) = left.split_whitespace().nth(4) else {
            continue;
        };
        let mount = PathBuf::from(
            raw.replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\012", "\n")
                .replace("\\134", "\\"),
        );
        if canonical.starts_with(&mount)
            && selected
                .as_ref()
                .is_none_or(|(n, _)| mount.as_os_str().len() > *n)
        {
            selected = Some((
                mount.as_os_str().len(),
                format!(
                    "{}:{raw}",
                    left.split_whitespace().next().unwrap_or_default()
                ),
            ));
        }
    }
    selected
        .map(|(_, id)| id)
        .ok_or_else(|| err(Kind::Disconnected, Op::Connect, &path))
}
impl StorageConnector for SmbConnector {
    fn connector_id(&self) -> &ConnectorId {
        &self.id
    }
    fn capabilities(&self) -> &StorageCapabilities {
        &self.caps
    }
    fn list(
        &self,
        p: &ResourcePath,
        t: Option<&PageToken>,
        c: &CancellationToken,
    ) -> Result<ListPage, StorageError> {
        self.read_guard(Op::List, p)?;
        self.local.list(p, t, c)
    }
    fn stat(&self, p: &ResourcePath, c: &CancellationToken) -> Result<FileEntry, StorageError> {
        self.read_guard(Op::Stat, p)?;
        self.local.stat(p, c)
    }
    fn open_read(
        &self,
        p: &ResourcePath,
        c: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        self.read_guard(Op::Read, p)?;
        self.local.open_read(p, c)
    }
    fn open_write(
        &self,
        p: &ResourcePath,
        o: &WriteOptions,
        c: &CancellationToken,
    ) -> Result<Box<dyn ContentWriter>, StorageError> {
        self.guard(Op::Write, p)?;
        self.local.open_write(p, o, c)
    }
    fn create_directory(
        &self,
        p: &ResourcePath,
        c: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        self.guard(Op::CreateDirectory, p)?;
        self.local.create_directory(p, c)
    }
    fn delete(
        &self,
        p: &ResourcePath,
        k: EntryKind,
        c: &CancellationToken,
    ) -> Result<(), StorageError> {
        self.guard(Op::Delete, p)?;
        self.local.delete(p, k, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_directory_is_never_accepted_as_an_smb_share() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        assert!(SmbConnector::new(directory.path(), false).is_err());
        assert!(SmbConnector::new(directory.path(), true).is_err());
        Ok(())
    }
    #[cfg(windows)]
    #[test]
    fn absent_unc_share_is_not_an_empty_source() {
        assert!(SmbConnector::new(r"\\127.0.0.1\syncengine-nonexistent-fixture", false).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn delegate_staging_and_detect_disappeared_share_root() -> Result<(), Box<dyn std::error::Error>>
    {
        // Private test substitution, not a successful SMB negotiation claim.
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("share");
        std::fs::create_dir(&root)?;
        let local = LocalConnector::new(&root, LocalOptions::new().with_mutations())?;
        let caps = local.capabilities().clone();
        let connector = SmbConnector {
            local,
            root: root.clone(),
            id: ConnectorId::new("smb")?,
            caps,
            preview_scan: false,
        };
        let path = ResourcePath::parse("file")?;
        let cancel = CancellationToken::new();
        let mut writer = connector.open_write(&path, &WriteOptions::new(), &cancel)?;
        writer.write(b"data")?;
        assert!(connector.stat(&path, &cancel).is_err());
        writer.finish()?;
        writer.commit(connector_api::CommitMode::CreateNew)?;
        assert_eq!(connector.stat(&path, &cancel)?.metadata().size(), Some(4));
        connector.delete(&path, EntryKind::File, &cancel)?;
        // Live directory handles pin Windows roots. Model a disconnected share
        // between clients, then verify the guard refuses an unavailable root.
        assert!(std::fs::remove_dir(&root).is_err());
        drop(connector);
        std::fs::remove_dir(&root)?;
        let local = LocalConnector::new(&root, LocalOptions::new().with_mutations())?;
        let connector = SmbConnector {
            caps: local.capabilities().clone(),
            local,
            root: root.clone(),
            id: ConnectorId::new("smb")?,
            preview_scan: false,
        };
        assert_eq!(
            connector
                .list(&ResourcePath::root(), None, &cancel)
                .err()
                .ok_or("root disappearance hidden")?
                .kind(),
            Kind::Disconnected
        );
        Ok(())
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn only_preview_reuses_handles_and_never_enables_writes() {
        assert_eq!(
            local_options(false, true),
            LocalOptions::new().with_handle_reuse()
        );
        assert_eq!(local_options(false, false), LocalOptions::new());
        assert_eq!(
            local_options(true, true),
            LocalOptions::new().with_mutations()
        );
        assert_eq!(
            local_options(true, false),
            LocalOptions::new().with_mutations()
        );
    }
}
