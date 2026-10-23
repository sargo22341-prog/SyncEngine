//! Implémentation de [`StorageConnector`] pour le système de fichiers local.

use cap_fs_ext::OpenOptionsFollowExt;
use cap_std::fs::{Dir, OpenOptions};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use connector_api::{
    CaseSensitivity, ClockSource, ContentReader, ContentWriter, ListPage, ListingConsistency,
    PageToken, Pagination, StorageCapabilities, StorageConnector, StorageError, StorageErrorKind,
    StorageFeature, StorageOperation, SymlinkSupport, WriteOptions, ensure_supported,
};
use sync_core::{
    CancellationToken, ConnectorId, EntryKind, FileEntry, ResourcePath, TimestampPrecision,
};

use crate::errors::{io_error, local_error};

use crate::write;

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
    mutations: bool,
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

    /// Autorise les mutations (création de répertoire, écriture en deux
    /// temps, suppression non récursive). **Désactivées par défaut** : une
    /// extrémité source s'ouvre sans cette option, ce qui rend toute
    /// mutation de la source structurellement impossible.
    #[must_use]
    pub fn with_mutations(mut self) -> Self {
        self.mutations = true;
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

/// Connecteur du système de fichiers local, **en lecture seule par défaut**.
///
/// - Les liens symboliques et jonctions sont rapportés comme
///   [`EntryKind::Symlink`] et jamais suivis, ni en listing, ni en lecture,
///   ni en écriture ; la racine elle-même peut être un lien (choix explicite
///   de l'utilisateur).
/// - Sans [`LocalOptions::with_mutations`], aucune méthode n'écrit, ne crée
///   ni ne supprime quoi que ce soit (`NotSupported`).
/// - Avec mutations : écriture dans un fichier temporaire du même dossier
///   (`.syncengine-….partial`), rendu durable (`fsync`) puis publié par
///   lien physique (création, sans écrasement possible) ou renommage
///   (remplacement) ; suppression d'un fichier ou d'un dossier **vide**
///   seulement ; un chemin traversant un lien est refusé, et la position
///   réelle du dossier parent sous la racine est revérifiée juste avant la
///   publication (voir `write.rs` pour la limite résiduelle).
/// - La construction ne fait aucun accès disque : une racine absente est
///   signalée par la première opération, comme `Disconnected`.
#[derive(Debug)]
pub struct LocalConnector {
    id: ConnectorId,
    root: PathBuf,
    anchor: Result<Arc<Dir>, io::Error>,
    capabilities: StorageCapabilities,
    precision: TimestampPrecision,
    pages: crate::paging::Pages,
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
        if options.mutations {
            capabilities = capabilities.with_features([
                StorageFeature::Write,
                StorageFeature::SetModifiedTime,
                StorageFeature::StreamingWrite,
                StorageFeature::CreateDirectory,
                StorageFeature::Delete,
            ]);
        }
        capabilities.case_sensitivity = options
            .case_sensitivity
            .unwrap_or_else(default_case_sensitivity);
        capabilities.timestamp_precision = precision;
        capabilities.clock = ClockSource::LocalSystem;
        capabilities.listing_consistency = ListingConsistency::Strong;
        capabilities.pagination = Pagination::Paginated {
            max_page_size: Some(256),
        };
        capabilities.symlinks = SymlinkSupport::Reported;
        Ok(Self {
            id: crate::connector_id(),
            anchor: Dir::open_ambient_dir(&root, cap_std::ambient_authority()).map(Arc::new),
            root,
            capabilities,
            precision,
            pages: crate::paging::Pages::default(),
        })
    }

    /// Racine OS de l'extrémité.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn anchor(&self, operation: StorageOperation) -> Result<&Arc<Dir>, StorageError> {
        self.anchor.as_ref().map_err(|e| {
            io_error(
                io::Error::new(e.kind(), "root handle unavailable; reconnect"),
                operation,
                &ResourcePath::root(),
            )
        })
    }

    /// Vérifie la racine : son absence est une indisponibilité de
    /// l'extrémité (montage absent), jamais un répertoire vide.
    fn check_root(&self, operation: StorageOperation) -> Result<(), StorageError> {
        let root = ResourcePath::root();
        match fs::metadata(&self.root) {
            Ok(metadata) if metadata.is_dir() => {
                use cap_fs_ext::MetadataExt;
                #[cfg(windows)]
                let metadata = Dir::open_ambient_dir(&self.root, cap_std::ambient_authority())
                    .and_then(|dir| dir.dir_metadata())
                    .map_err(|e| io_error(e, operation, &root))?;
                let granted = self
                    .anchor(operation)?
                    .dir_metadata()
                    .map_err(|e| io_error(e, operation, &root))?;
                if metadata.dev() != granted.dev() || metadata.ino() != granted.ino() {
                    return Err(local_error(
                        StorageErrorKind::Disconnected,
                        operation,
                        &root,
                        "root identity changed; reconnect required",
                    ));
                }
                Ok(())
            }
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

    /// Contrôles communs avant une mutation : capacité déclarée, pas
    /// d'annulation, racine disponible, chemin autre que la racine.
    fn before_mutation(
        &self,
        feature: StorageFeature,
        operation: StorageOperation,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        ensure_supported(self, feature, operation)
            .map_err(|error| error.with_path(path.clone()))?;
        if cancel.is_cancelled() {
            return Err(cancelled(operation, path));
        }
        self.check_root(operation)?;
        if path.is_root() {
            return Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                path,
                "la racine elle-même ne peut pas être créée, écrite ni supprimée",
            ));
        }
        Ok(())
    }
}

pub(crate) fn cancelled(operation: StorageOperation, path: &ResourcePath) -> StorageError {
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
        if cancel.is_cancelled() {
            return Err(cancelled(operation, directory));
        }
        self.check_root(operation)?;
        self.pages.list(
            self.anchor(operation)?,
            directory,
            page,
            self.precision,
            cancel,
        )
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
        let root = self.anchor(operation)?;
        let meta = if path.is_root() {
            root.dir_metadata()
                .map_err(|e| io_error(e, operation, path))?
        } else {
            crate::confined::parent(root, path, operation)?
                .symlink_metadata(path.segments().last().unwrap_or(""))
                .map_err(|e| io_error(e, operation, path))?
        };
        Ok(crate::confined::entry(path.clone(), &meta, self.precision))
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
        let parent = crate::confined::parent(self.anchor(operation)?.as_ref(), path, operation)?;
        let name = path.segments().last().unwrap_or("");
        let meta = parent
            .symlink_metadata(name)
            .map_err(|e| io_error(e, operation, path))?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                path,
                "not an ordinary file",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(cap_fs_ext::FollowSymlinks::No);
        let file = parent
            .open_with(name, &options)
            .map_err(|e| io_error(e, operation, path))?
            .into_std();
        if !file
            .metadata()
            .map_err(|e| io_error(e, operation, path))?
            .is_file()
        {
            return Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                path,
                "not an ordinary file",
            ));
        }
        Ok(Box::new(LocalReader {
            file,
            path: path.clone(),
            cancel: cancel.clone(),
        }))
    }

    fn create_directory(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        let operation = StorageOperation::CreateDirectory;
        self.before_mutation(StorageFeature::CreateDirectory, operation, path, cancel)?;
        write::create_directory(self.anchor(operation)?, path, self.precision)
    }

    fn open_write(
        &self,
        path: &ResourcePath,
        options: &WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentWriter>, StorageError> {
        let operation = StorageOperation::Write;
        self.before_mutation(StorageFeature::Write, operation, path, cancel)?;
        let writer = write::LocalWriter::open(
            self.anchor(operation)?,
            path,
            *options,
            self.precision,
            cancel,
        )?;
        Ok(Box::new(writer))
    }

    fn delete(
        &self,
        path: &ResourcePath,
        expected: EntryKind,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        let operation = StorageOperation::Delete;
        self.before_mutation(StorageFeature::Delete, operation, path, cancel)?;
        write::delete(self.anchor(operation)?, path, expected)
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
