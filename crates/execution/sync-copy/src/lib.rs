//! Moteur de transfert de fichiers.
//!
//! [`stage_copy`] lit un fichier source **en flux** (tampon borné, jamais le
//! fichier entier en mémoire) et l'écrit dans le **dépôt temporaire** d'une
//! écriture en deux temps du connecteur destination
//! ([`connector_api::ContentWriter`]), en calculant l'empreinte BLAKE3 du
//! contenu transféré. Le résultat, [`StagedCopy`], n'est **pas encore
//! visible** à la destination : l'appelant (l'exécuteur) le vérifie,
//! revérifie ses préconditions, puis le valide ([`StagedCopy::commit`]) ou
//! l'abandonne ([`StagedCopy::abort`]).
//!
//! Ce crate ne décide de rien : ni s'il faut copier, ni s'il faut écraser,
//! et il ne supprime jamais. En cas d'échec, le dépôt temporaire est retiré
//! (et un échec de ce retrait est rapporté, jamais ignoré).
//!
//! # Exemple
//! ```
//! use connector_api::{CommitMode, WriteOptions};
//! use connector_memory::{MemoryConnector, path, ts};
//! use sync_copy::{CopyRequest, stage_copy};
//! use sync_core::CancellationToken;
//!
//! let source = MemoryConnector::new("source").file("a.txt", b"contenu", ts(10));
//! let destination = MemoryConnector::new("destination").writable();
//! let (from, to) = (path("a.txt"), path("a.txt"));
//! let request = CopyRequest::new(&source, &from, &destination, &to)
//!     .with_options(WriteOptions::new().with_modified(Some(ts(10))));
//! let staged = stage_copy(&request, &mut |_| {}, &CancellationToken::new())?;
//! assert_eq!(staged.bytes(), 7);
//! staged.commit(CommitMode::CreateNew)?;
//! assert_eq!(destination.content("a.txt").as_deref(), Some(&b"contenu"[..]));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use connector_api::{
    CommitMode, ContentWriter, StorageConnector, StorageError, StorageErrorKind, StorageOperation,
    WriteOptions,
};
use sync_core::{CancellationToken, ContentHash, FileEntry, HashAlgorithm, ResourcePath};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext};
use thiserror::Error;

/// Taille par défaut du tampon de transfert (256 Kio).
pub const DEFAULT_BUFFER_SIZE: usize = 256 * 1024;

/// Demande de transfert d'un fichier.
#[derive(Clone, Copy)]
pub struct CopyRequest<'a> {
    source: &'a dyn StorageConnector,
    source_path: &'a ResourcePath,
    destination: &'a dyn StorageConnector,
    destination_path: &'a ResourcePath,
    options: WriteOptions,
    buffer_size: usize,
}

impl fmt::Debug for CopyRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CopyRequest")
            .field("source", self.source.connector_id())
            .field("source_path", self.source_path)
            .field("destination", self.destination.connector_id())
            .field("destination_path", self.destination_path)
            .field("options", &self.options)
            .field("buffer_size", &self.buffer_size)
            .finish()
    }
}

impl<'a> CopyRequest<'a> {
    /// Copie de `source_path` (lu sur `source`) vers `destination_path`
    /// (écrit sur `destination`), tampon par défaut, sans option d'écriture.
    #[must_use]
    pub fn new(
        source: &'a dyn StorageConnector,
        source_path: &'a ResourcePath,
        destination: &'a dyn StorageConnector,
        destination_path: &'a ResourcePath,
    ) -> Self {
        Self {
            source,
            source_path,
            destination,
            destination_path,
            options: WriteOptions::new(),
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }

    /// Options transmises à l'ouverture de l'écriture (date à appliquer…).
    /// La date est omise si le stockage ne déclare pas `SetModifiedTime` ;
    /// cela n'affecte pas le contenu ni sa vérification.
    #[must_use]
    pub fn with_options(mut self, options: WriteOptions) -> Self {
        self.options = options;
        self
    }

    /// Taille du tampon (au moins 1 octet).
    #[must_use]
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size.max(1);
        self
    }
}

/// Étape du transfert où l'échec est survenu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CopyStage {
    /// Ouverture du fichier source.
    OpenSource,
    /// Ouverture de l'écriture à la destination.
    OpenDestination,
    /// Lecture de la source.
    Read,
    /// Écriture du dépôt temporaire.
    Write,
}

impl fmt::Display for CopyStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OpenSource => "ouverture de la source",
            Self::OpenDestination => "ouverture de la destination",
            Self::Read => "lecture de la source",
            Self::Write => "écriture à la destination",
        })
    }
}

/// Échec d'un transfert. Le dépôt temporaire a été abandonné ; si ce
/// retrait a lui-même échoué, [`CopyError::cleanup_error`] le dit et
/// [`CopyError::staging_path`] indique ce qui reste.
#[derive(Debug, Error)]
#[error("transfert interrompu ({stage}) : {error}")]
pub struct CopyError {
    stage: CopyStage,
    #[source]
    error: StorageError,
    staging: Option<ResourcePath>,
    cleanup: Option<StorageError>,
}

impl CopyError {
    fn new(stage: CopyStage, error: StorageError) -> Self {
        Self {
            stage,
            error,
            staging: None,
            cleanup: None,
        }
    }

    /// Étape en échec.
    #[must_use]
    pub fn stage(&self) -> CopyStage {
        self.stage
    }

    /// Erreur du stockage.
    #[must_use]
    pub fn storage_error(&self) -> &StorageError {
        &self.error
    }

    /// Catégorie de l'erreur du stockage.
    #[must_use]
    pub fn kind(&self) -> StorageErrorKind {
        self.error.kind()
    }

    /// Dépôt temporaire concerné, s'il avait été créé.
    #[must_use]
    pub fn staging_path(&self) -> Option<&ResourcePath> {
        self.staging.as_ref()
    }

    /// Échec du retrait du dépôt temporaire, le cas échéant.
    #[must_use]
    pub fn cleanup_error(&self) -> Option<&StorageError> {
        self.cleanup.as_ref()
    }
}

impl ClassifiedError for CopyError {
    fn class(&self) -> ErrorClass {
        self.error.kind().class()
    }

    fn context(&self) -> Option<&ErrorContext> {
        self.error.context()
    }
}

/// Fichier transféré dans un dépôt temporaire, **pas encore validé**.
///
/// L'abandon de cette valeur sans [`commit`](Self::commit) ni
/// [`abort`](Self::abort) retire le dépôt au mieux (sans pouvoir signaler
/// d'échec) : préférer `abort`.
pub struct StagedCopy {
    writer: Box<dyn ContentWriter>,
    bytes: u64,
    hash: ContentHash,
}

impl fmt::Debug for StagedCopy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StagedCopy")
            .field("staging", &self.writer.staging_path())
            .field("bytes", &self.bytes)
            .field("hash", &self.hash)
            .finish()
    }
}

impl StagedCopy {
    /// Octets transférés.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Empreinte BLAKE3 du contenu **lu à la source** et écrit.
    #[must_use]
    pub fn hash(&self) -> &ContentHash {
        &self.hash
    }

    /// Chemin du dépôt temporaire à la destination, s'il en a un.
    #[must_use]
    pub fn staging_path(&self) -> Option<&ResourcePath> {
        self.writer.staging_path()
    }

    /// Valide le dépôt à la cible (voir [`connector_api::ContentWriter::commit`]).
    ///
    /// # Errors
    /// `Conflict` si la cible existe en mode [`CommitMode::CreateNew`] ou
    /// n'est plus un fichier en mode [`CommitMode::ReplaceExisting`] ; autre
    /// erreur du stockage.
    pub fn commit(self, mode: CommitMode) -> Result<FileEntry, StorageError> {
        self.writer.commit(mode)
    }

    /// Abandonne le dépôt.
    ///
    /// # Errors
    /// Le dépôt temporaire n'a pas pu être retiré.
    pub fn abort(self) -> Result<(), StorageError> {
        self.writer.abort()
    }
}

fn cancelled(operation: StorageOperation, path: &ResourcePath) -> StorageError {
    StorageError::new(StorageErrorKind::Cancelled, operation).with_path(path.clone())
}

/// Transfère le fichier décrit par `request` dans un dépôt temporaire.
///
/// `on_progress` reçoit le nombre d'octets de chaque bloc transféré (un
/// appel par bloc, jamais par octet). L'annulation est consultée avant
/// chaque bloc.
///
/// # Préconditions
/// Le parent de la cible existe à la destination ; la source est un fichier
/// ordinaire. Ce moteur ne vérifie pas que la source correspond à une
/// observation antérieure : c'est le rôle de l'exécuteur (et de
/// `sync-verify`), avec la taille et l'empreinte rendues ici.
///
/// # Errors
/// [`CopyError`] avec l'étape en échec ; le dépôt temporaire éventuel a été
/// abandonné.
pub fn stage_copy(
    request: &CopyRequest<'_>,
    on_progress: &mut dyn FnMut(u64),
    cancel: &CancellationToken,
) -> Result<StagedCopy, CopyError> {
    stage_copy_observed(request, on_progress, &mut |_| Ok(()), cancel)
}

/// Transfers like [`stage_copy`], recording the staging identity immediately
/// after reservation and before the first content write. A failed observer
/// aborts the writer and reports cleanup failure. The reservation itself may
/// precede observation; durable operation intent must already exist.
/// # Errors
/// Transfer or observer failure, with explicit cleanup diagnostics.
pub fn stage_copy_observed(
    request: &CopyRequest<'_>,
    on_progress: &mut dyn FnMut(u64),
    on_staging: &mut dyn FnMut(Option<&ResourcePath>) -> Result<(), StorageError>,
    cancel: &CancellationToken,
) -> Result<StagedCopy, CopyError> {
    if cancel.is_cancelled() {
        return Err(CopyError::new(
            CopyStage::OpenSource,
            cancelled(StorageOperation::Read, request.source_path),
        ));
    }
    let mut reader = request
        .source
        .open_read(request.source_path, cancel)
        .map_err(|error| CopyError::new(CopyStage::OpenSource, error))?;
    let mut writer = request
        .destination
        .open_write(
            request.destination_path,
            &if request
                .destination
                .capabilities()
                .supports(connector_api::StorageFeature::SetModifiedTime)
            {
                request.options
            } else {
                request.options.with_modified(None)
            },
            cancel,
        )
        .map_err(|error| CopyError::new(CopyStage::OpenDestination, error))?;

    if let Err(error) = on_staging(writer.staging_path()) {
        return Err(abandon(
            writer,
            CopyError::new(CopyStage::OpenDestination, error),
        ));
    }
    let mut buffer = vec![0u8; request.buffer_size];
    let mut hasher = blake3::Hasher::new();
    let mut bytes: u64 = 0;
    let failure = loop {
        if cancel.is_cancelled() {
            break CopyError::new(
                CopyStage::Read,
                cancelled(StorageOperation::Read, request.source_path),
            );
        }
        let read = match reader.read(&mut buffer) {
            Ok(0) => {
                return finish(writer, bytes, &hasher, request);
            }
            Ok(read) => read,
            Err(error) => break CopyError::new(CopyStage::Read, error),
        };
        let chunk = buffer.get(..read).unwrap_or_default();
        hasher.update(chunk);
        if let Err(error) = writer.write(chunk) {
            break CopyError::new(CopyStage::Write, error);
        }
        bytes = bytes.saturating_add(read as u64);
        on_progress(read as u64);
    };
    Err(abandon(writer, failure))
}

fn finish(
    mut writer: Box<dyn ContentWriter>,
    bytes: u64,
    hasher: &blake3::Hasher,
    request: &CopyRequest<'_>,
) -> Result<StagedCopy, CopyError> {
    if let Err(error) = writer.finish() {
        return Err(abandon(writer, CopyError::new(CopyStage::Write, error)));
    }
    match ContentHash::new(HashAlgorithm::Blake3, hasher.finalize().as_bytes().to_vec()) {
        Ok(hash) => Ok(StagedCopy {
            writer,
            bytes,
            hash,
        }),
        Err(_) => {
            let error = StorageError::new(StorageErrorKind::Permanent, StorageOperation::Read)
                .with_path(request.source_path.clone())
                .with_message("empreinte BLAKE3 de longueur inattendue");
            Err(abandon(writer, CopyError::new(CopyStage::Read, error)))
        }
    }
}

/// Abandonne le dépôt et complète l'erreur avec ce qui reste éventuellement.
fn abandon(writer: Box<dyn ContentWriter>, mut failure: CopyError) -> CopyError {
    failure.staging = writer.staging_path().cloned();
    if let Err(cleanup) = writer.abort() {
        failure.cleanup = Some(cleanup);
    }
    failure
}
