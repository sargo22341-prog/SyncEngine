//! Conservation des versions avant écrasement ou suppression.
//!
//! Un [`VersionStore`] est un **magasin séparé** (son propre connecteur, sa
//! propre racine, jamais à l'intérieur de la source ni de la destination,
//! ce que l'appelant vérifie) dans lequel l'exécuteur dépose, avant de
//! remplacer ou de supprimer un fichier de la destination, une copie
//! vérifiée de ce fichier : `<dossier d'exécution>/<chemin logique>`.
//!
//! - la copie passe par un dépôt temporaire ([`sync_copy`]) et n'est validée
//!   que si sa taille et son empreinte correspondent à l'observation
//!   enregistrée dans le plan ; sinon le fichier a changé et l'exécuteur doit
//!   renoncer à la mutation ;
//! - une version déjà présente n'est **jamais** écrasée (validation
//!   `CreateNew`) ;
//! - la date de modification d'origine est conservée.
//!
//! Limites (phase 04) : aucune politique de rétention (les versions
//! s'accumulent, un dossier par exécution) ; seuls les fichiers sont
//! conservés (un répertoire supprimé est vide par construction).
//!
//! # Exemple
//! ```
//! use connector_memory::{MemoryConnector, path, ts};
//! use sync_core::{CancellationToken, FileMetadata, TimestampPrecision};
//! use sync_versioning::VersionStore;
//!
//! let destination = MemoryConnector::new("destination").file("a.txt", b"ancien", ts(5));
//! let versions = MemoryConnector::new("versions").writable();
//! let store = VersionStore::new(&versions, path("exec-1"))?;
//! let observed = destination.entry_at("a.txt").unwrap();
//! let kept = store.preserve(&destination, &path("a.txt"), observed.metadata(), &CancellationToken::new())?;
//! assert_eq!(kept.location().as_str(), "exec-1/a.txt");
//! assert_eq!(versions.content("exec-1/a.txt").as_deref(), Some(&b"ancien"[..]));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use connector_api::{
    CommitMode, StorageConnector, StorageError, StorageErrorKind, StorageFeature, StorageOperation,
    WriteOptions, ensure_supported,
};
use sync_copy::{CopyError, CopyRequest, stage_copy};
use sync_core::{CancellationToken, ContentHash, EntryKind, FileMetadata, ResourcePath};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext};
use sync_verify::{Mismatch, check_transfer};
use thiserror::Error;

/// Erreur de conservation d'une version.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum VersioningError {
    /// Le dossier d'exécution doit être un sous-dossier (pas la racine du magasin).
    #[error("le dossier des versions ne peut pas être la racine du magasin")]
    RootFolder,
    /// Opération du magasin en échec (capacité absente, création de dossier…).
    #[error("magasin de versions : {0}")]
    Store(#[source] StorageError),
    /// Copie du fichier vers le magasin en échec.
    #[error("copie de la version : {0}")]
    Copy(#[source] CopyError),
    /// Le fichier lu ne correspond plus à l'observation : il a changé.
    #[error("le fichier a changé depuis l'observation : {0}")]
    Changed(Mismatch),
}

impl ClassifiedError for VersioningError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::RootFolder => ErrorClass::InvalidInput,
            Self::Store(error) => error.kind().class(),
            Self::Copy(error) => error.class(),
            Self::Changed(_) => ErrorClass::Conflict,
        }
    }

    fn context(&self) -> Option<&ErrorContext> {
        match self {
            Self::Store(error) => error.context(),
            Self::Copy(error) => error.context(),
            Self::RootFolder | Self::Changed(_) => None,
        }
    }
}

/// Version conservée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedVersion {
    location: ResourcePath,
    bytes: u64,
    hash: ContentHash,
}

impl PreservedVersion {
    /// Chemin de la version dans le magasin.
    #[must_use]
    pub fn location(&self) -> &ResourcePath {
        &self.location
    }

    /// Taille conservée.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Empreinte BLAKE3 du contenu conservé.
    #[must_use]
    pub fn hash(&self) -> &ContentHash {
        &self.hash
    }
}

/// Magasin de versions (voir la documentation du crate).
pub struct VersionStore<'a> {
    connector: &'a dyn StorageConnector,
    folder: ResourcePath,
}

impl fmt::Debug for VersionStore<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VersionStore")
            .field("connector", self.connector.connector_id())
            .field("folder", &self.folder)
            .finish()
    }
}

impl<'a> VersionStore<'a> {
    /// Magasin sur `connector`, versions rangées sous `folder` (en général
    /// l'identifiant d'exécution).
    ///
    /// # Errors
    /// [`VersioningError::RootFolder`] si `folder` est la racine ;
    /// [`VersioningError::Store`] (`NotSupported`) si le connecteur ne
    /// déclare pas lecture de métadonnées, écriture et création de dossier.
    pub fn new(
        connector: &'a dyn StorageConnector,
        folder: ResourcePath,
    ) -> Result<Self, VersioningError> {
        if folder.is_root() {
            return Err(VersioningError::RootFolder);
        }
        for (feature, operation) in [
            (StorageFeature::Stat, StorageOperation::Stat),
            (StorageFeature::Write, StorageOperation::Write),
            (
                StorageFeature::CreateDirectory,
                StorageOperation::CreateDirectory,
            ),
        ] {
            ensure_supported(connector, feature, operation).map_err(VersioningError::Store)?;
        }
        Ok(Self { connector, folder })
    }

    /// Dossier des versions de cette exécution.
    #[must_use]
    pub fn folder(&self) -> &ResourcePath {
        &self.folder
    }

    /// Emplacement qu'aurait la version de `path`.
    ///
    /// # Errors
    /// Chemin non représentable (ne se produit pas pour un `ResourcePath` valide).
    pub fn location_of(&self, path: &ResourcePath) -> Result<ResourcePath, VersioningError> {
        ResourcePath::from_segments(self.folder.segments().chain(path.segments())).map_err(|_| {
            VersioningError::Store(
                StorageError::new(StorageErrorKind::Permanent, StorageOperation::Write)
                    .with_path(path.clone())
                    .with_message("chemin de version invalide"),
            )
        })
    }

    /// Copie le fichier `path` de `from` dans le magasin, après avoir
    /// vérifié qu'il correspond toujours à `observed` (taille, empreinte si
    /// enregistrée).
    ///
    /// # Errors
    /// [`VersioningError::Changed`] si le contenu lu ne correspond plus ;
    /// [`VersioningError::Copy`] ou [`VersioningError::Store`] (dont
    /// `Conflict` si une version existe déjà à cet emplacement).
    pub fn preserve(
        &self,
        from: &dyn StorageConnector,
        path: &ResourcePath,
        observed: &FileMetadata,
        cancel: &CancellationToken,
    ) -> Result<PreservedVersion, VersioningError> {
        let location = self.location_of(path)?;
        if let Some(parent) = location.parent() {
            self.ensure_directory(&parent, cancel)?;
        }
        let options = WriteOptions::new()
            .with_modified(observed.modified())
            .with_size_hint(observed.size());
        let request = CopyRequest::new(from, path, self.connector, &location).with_options(options);
        let staged = stage_copy(&request, &mut |_| {}, cancel).map_err(VersioningError::Copy)?;
        if let Err(mismatch) = check_transfer(observed, staged.bytes(), staged.hash()) {
            staged.abort().map_err(VersioningError::Store)?;
            return Err(VersioningError::Changed(mismatch));
        }
        let (bytes, hash) = (staged.bytes(), staged.hash().clone());
        staged
            .commit(CommitMode::CreateNew)
            .map_err(VersioningError::Store)?;
        Ok(PreservedVersion {
            location,
            bytes,
            hash,
        })
    }

    /// Crée `dir` et ses ancêtres manquants dans le magasin.
    fn ensure_directory(
        &self,
        dir: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<(), VersioningError> {
        let mut current = ResourcePath::root();
        for segment in dir.segments() {
            current = current.join(segment).map_err(|_| {
                VersioningError::Store(StorageError::new(
                    StorageErrorKind::Permanent,
                    StorageOperation::CreateDirectory,
                ))
            })?;
            match self.connector.stat(&current, cancel) {
                Ok(entry) if entry.kind() == EntryKind::Directory => continue,
                Ok(_) => {
                    return Err(VersioningError::Store(
                        StorageError::new(
                            StorageErrorKind::Conflict,
                            StorageOperation::CreateDirectory,
                        )
                        .with_path(current)
                        .with_message("une entrée qui n'est pas un dossier occupe ce chemin"),
                    ));
                }
                Err(error) if error.kind() == StorageErrorKind::NotFound => {
                    self.connector
                        .create_directory(&current, cancel)
                        .map_err(VersioningError::Store)?;
                }
                Err(error) => return Err(VersioningError::Store(error)),
            }
        }
        Ok(())
    }
}
