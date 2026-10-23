//! Implémentation de [`StorageConnector`] pour le système de fichiers local.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use connector_api::{
    CaseSensitivity, ClockSource, ContentReader, ListPage, ListingConsistency, ListingIssue,
    ListingIssueKind, NameIssue, PageToken, Pagination, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind, StorageFeature, StorageOperation, SymlinkSupport,
};
use sync_core::{
    CancellationToken, ConnectorId, EntryKind, FileEntry, FileMetadata, ResourcePath, Timestamp,
    TimestampPrecision,
};

use crate::errors::{io_error, local_error};
use crate::paths::{is_ambiguous_here, resolve};

/// Nombre d'entrées lues entre deux consultations du jeton d'annulation.
const CANCEL_CHECK_INTERVAL: usize = 256;

/// Options du connecteur local.
///
/// Les valeurs par défaut décrivent le système de fichiers **typique** de la
/// plateforme (NTFS sous Windows, ext4 sous Linux). Un volume FAT/exFAT, un
/// montage réseau ou un répertoire NTFS sensible à la casse ont d'autres
/// propriétés : elles doivent alors être déclarées explicitement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalOptions {
    timestamp_precision: Option<TimestampPrecision>,
    case_sensitivity: Option<CaseSensitivity>,
}

impl LocalOptions {
    /// Options par défaut de la plateforme.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Déclare la précision réelle des dates du volume (par exemple
    /// [`TimestampPrecision::TwoSeconds`] pour FAT).
    #[must_use]
    pub fn with_timestamp_precision(mut self, precision: TimestampPrecision) -> Self {
        self.timestamp_precision = Some(precision);
        self
    }

    /// Déclare la règle de casse réelle du volume.
    #[must_use]
    pub fn with_case_sensitivity(mut self, case: CaseSensitivity) -> Self {
        self.case_sensitivity = Some(case);
        self
    }
}

fn default_precision() -> TimestampPrecision {
    if cfg!(windows) {
        TimestampPrecision::HundredNanoseconds
    } else {
        TimestampPrecision::Nanosecond
    }
}

fn default_case_sensitivity() -> CaseSensitivity {
    if cfg!(windows) {
        CaseSensitivity::InsensitivePreserving
    } else {
        CaseSensitivity::Sensitive
    }
}

/// Erreur de configuration du connecteur local.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalConfigError {
    /// La racine doit être un chemin absolu (aucune dépendance au
    /// répertoire courant).
    RootNotAbsolute,
}

impl std::fmt::Display for LocalConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotAbsolute => f.write_str("la racine locale doit être un chemin absolu"),
        }
    }
}

impl std::error::Error for LocalConfigError {}

/// Connecteur du système de fichiers local, **en lecture seule**.
///
/// - Les liens symboliques et jonctions sont rapportés comme
///   [`EntryKind::Symlink`] et jamais suivis, ni en listing ni en lecture ;
///   la racine elle-même peut être un lien (choix explicite de l'utilisateur).
/// - Aucune méthode n'écrit, ne crée ni ne supprime quoi que ce soit.
/// - La construction ne fait aucun accès disque : une racine absente est
///   signalée par la première opération, comme `Disconnected`.
#[derive(Debug)]
pub struct LocalConnector {
    id: ConnectorId,
    root: PathBuf,
    capabilities: StorageCapabilities,
    precision: TimestampPrecision,
}

impl LocalConnector {
    /// Crée un connecteur enraciné en `root` (chemin absolu de l'OS).
    ///
    /// # Errors
    /// [`LocalConfigError::RootNotAbsolute`] si `root` est relatif.
    pub fn new(root: impl Into<PathBuf>, options: LocalOptions) -> Result<Self, LocalConfigError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(LocalConfigError::RootNotAbsolute);
        }
        let precision = options
            .timestamp_precision
            .unwrap_or_else(default_precision);
        let mut capabilities = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Read,
            StorageFeature::StreamingRead,
            StorageFeature::Cancellation,
        ]);
        capabilities.case_sensitivity = options
            .case_sensitivity
            .unwrap_or_else(default_case_sensitivity);
        capabilities.timestamp_precision = precision;
        capabilities.clock = ClockSource::LocalSystem;
        capabilities.listing_consistency = ListingConsistency::Strong;
        capabilities.pagination = Pagination::Unpaginated;
        capabilities.symlinks = SymlinkSupport::Reported;
        Ok(Self {
            id: crate::connector_id(),
            root,
            capabilities,
            precision,
        })
    }

    /// Racine OS de l'extrémité.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn entry_from_metadata(&self, path: ResourcePath, metadata: &fs::Metadata) -> FileEntry {
        let file_type = metadata.file_type();
        let kind = kind_of(&file_type);
        let mut observed = FileMetadata::unknown();
        if kind == EntryKind::File {
            observed = observed.with_size(metadata.len());
        }
        // Une date illisible ou hors plage reste inconnue, jamais zéro.
        if let Some(modified) = metadata
            .modified()
            .ok()
            .and_then(|time| Timestamp::from_system_time(time).ok())
        {
            observed = observed.with_modified(modified, self.precision);
        }
        FileEntry::new(path, kind, observed)
    }

    /// Vérifie la racine : son absence est une indisponibilité de
    /// l'extrémité (montage absent), jamais un répertoire vide.
    fn check_root(&self, operation: StorageOperation) -> Result<(), StorageError> {
        let root = ResourcePath::root();
        match fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                &root,
                "la racine n'est pas un répertoire",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(local_error(
                StorageErrorKind::Disconnected,
                operation,
                &root,
                "racine introuvable (support ou montage absent ?)",
            )
            .with_source(error)),
            Err(error) => Err(io_error(error, operation, &root)),
        }
    }
}

fn kind_of(file_type: &fs::FileType) -> EntryKind {
    // `is_symlink` couvre aussi les jonctions et points de montage Windows.
    if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    }
}

fn cancelled(operation: StorageOperation, path: &ResourcePath) -> StorageError {
    local_error(StorageErrorKind::Cancelled, operation, path, "annulé")
}

impl StorageConnector for LocalConnector {
    fn connector_id(&self) -> &ConnectorId {
        &self.id
    }

    fn capabilities(&self) -> &StorageCapabilities {
        &self.capabilities
    }

    fn list(
        &self,
        directory: &ResourcePath,
        page: Option<&PageToken>,
        cancel: &CancellationToken,
    ) -> Result<ListPage, StorageError> {
        let operation = StorageOperation::List;
        if page.is_some() {
            return Err(local_error(
                StorageErrorKind::NotSupported,
                operation,
                directory,
                "listing non paginé : aucun jeton de page n'est émis",
            ));
        }
        if cancel.is_cancelled() {
            return Err(cancelled(operation, directory));
        }
        self.check_root(operation)?;
        let os_dir = if directory.is_root() {
            self.root.clone()
        } else {
            let (os_dir, metadata) = resolve(&self.root, directory, operation)?;
            if metadata.file_type().is_symlink() {
                return Err(local_error(
                    StorageErrorKind::Permanent,
                    operation,
                    directory,
                    "lien symbolique ou jonction : non suivi",
                ));
            }
            if !metadata.is_dir() {
                return Err(local_error(
                    StorageErrorKind::Permanent,
                    operation,
                    directory,
                    "pas un répertoire",
                ));
            }
            os_dir
        };

        let reader = fs::read_dir(&os_dir).map_err(|e| io_error(e, operation, directory))?;
        let mut entries = Vec::new();
        let mut issues = Vec::new();
        for (index, item) in reader.enumerate() {
            if index % CANCEL_CHECK_INTERVAL == 0 && cancel.is_cancelled() {
                return Err(cancelled(operation, directory));
            }
            // Une erreur au cours de l'itération rend tout le listing
            // incertain : elle est propagée, jamais tronquée en silence.
            let item = item.map_err(|e| io_error(e, operation, directory))?;
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
            let path = match directory.join(name) {
                Ok(path) => path,
                Err(error) => {
                    issues.push(ListingIssue::new(
                        name,
                        ListingIssueKind::UnrepresentableName(NameIssue::Invalid(error)),
                    ));
                    continue;
                }
            };
            // `DirEntry::metadata` ne suit pas les liens (lstat sous Unix,
            // données de l'énumération sous Windows).
            match item.metadata() {
                Ok(metadata) => entries.push(self.entry_from_metadata(path, &metadata)),
                Err(error) => {
                    let kind = crate::errors::classify(&error);
                    match item.file_type() {
                        Ok(file_type) => {
                            entries.push(FileEntry::new(
                                path,
                                kind_of(&file_type),
                                FileMetadata::unknown(),
                            ));
                            issues.push(ListingIssue::new(
                                name,
                                ListingIssueKind::MetadataUnavailable(kind),
                            ));
                        }
                        Err(_) => issues.push(ListingIssue::new(
                            name,
                            ListingIssueKind::EntryUnreadable(kind),
                        )),
                    }
                }
            }
        }
        Ok(ListPage::new(entries, issues, None))
    }

    fn stat(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        let operation = StorageOperation::Stat;
        if cancel.is_cancelled() {
            return Err(cancelled(operation, path));
        }
        self.check_root(operation)?;
        if path.is_root() {
            let metadata = fs::metadata(&self.root).map_err(|e| io_error(e, operation, path))?;
            return Ok(self.entry_from_metadata(ResourcePath::root(), &metadata));
        }
        let (_, metadata) = resolve(&self.root, path, operation)?;
        Ok(self.entry_from_metadata(path.clone(), &metadata))
    }

    fn open_read(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        let operation = StorageOperation::Read;
        if cancel.is_cancelled() {
            return Err(cancelled(operation, path));
        }
        self.check_root(operation)?;
        if path.is_root() {
            return Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                path,
                "la racine n'est pas un fichier",
            ));
        }
        let (os_path, metadata) = resolve(&self.root, path, operation)?;
        if !metadata.file_type().is_file() {
            return Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                path,
                "pas un fichier ordinaire (les liens ne sont pas suivis)",
            ));
        }
        let file = File::open(&os_path).map_err(|e| io_error(e, operation, path))?;
        Ok(Box::new(LocalReader {
            file,
            path: path.clone(),
            cancel: cancel.clone(),
        }))
    }
}

/// Flux de lecture d'un fichier local.
struct LocalReader {
    file: File,
    path: ResourcePath,
    cancel: CancellationToken,
}

impl ContentReader for LocalReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(cancelled(StorageOperation::Read, &self.path));
            }
            match self.file.read(buf) {
                Ok(read) => return Ok(read),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(io_error(error, StorageOperation::Read, &self.path)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_root() {
        assert_eq!(
            LocalConnector::new("relatif", LocalOptions::new()).err(),
            Some(LocalConfigError::RootNotAbsolute)
        );
    }

    #[test]
    fn declares_read_only_capabilities() {
        let root = std::env::temp_dir();
        let connector = LocalConnector::new(root, LocalOptions::new()).unwrap();
        let caps = connector.capabilities();
        assert!(caps.is_read_only());
        assert!(caps.supports(StorageFeature::List));
        assert!(caps.supports(StorageFeature::Read));
        assert!(!caps.supports(StorageFeature::Delete));
        assert_eq!(caps.symlinks, SymlinkSupport::Reported);
        assert_eq!(connector.connector_id().as_str(), "local");
    }

    #[test]
    fn options_override_platform_defaults() {
        let options = LocalOptions::new()
            .with_timestamp_precision(TimestampPrecision::TwoSeconds)
            .with_case_sensitivity(CaseSensitivity::Unknown);
        let connector = LocalConnector::new(std::env::temp_dir(), options).unwrap();
        assert_eq!(
            connector.capabilities().timestamp_precision,
            TimestampPrecision::TwoSeconds
        );
        assert_eq!(
            connector.capabilities().case_sensitivity,
            CaseSensitivity::Unknown
        );
    }
}
