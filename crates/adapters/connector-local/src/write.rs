//! Mutations du connecteur local : création de répertoire, écriture en deux
//! temps, suppression non récursive.
//!
//! # Durcissement contre les liens (faille signalée en phase 02)
//! Avant toute mutation : chaque ancêtre sous la racine doit être un vrai
//! répertoire (ni lien, ni jonction), puis la position **réelle** du dossier
//! parent (`canonicalize`) doit se trouver sous la position réelle de la
//! racine. Ce second contrôle est refait juste avant la publication d'un
//! fichier. Limite résiduelle : sans `openat`/`O_NOFOLLOW` (code `unsafe`
//! interdit et aucune dépendance système ajoutée), un tiers capable de
//! remplacer un dossier par un lien **entre** ce contrôle et l'appel système
//! qui suit n'est pas détecté ; la fenêtre est réduite à un appel système.
//! La dernière composante n'est jamais suivie : le fichier temporaire est
//! créé avec `create_new` (refus si un lien existe à ce nom) et
//! renommage, lien physique et suppression agissent sur l'entrée elle-même.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use connector_api::{
    CommitMode, ContentWriter, StorageError, StorageErrorKind, StorageOperation, WriteOptions,
};
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath, TimestampPrecision};

use crate::connector::{cancelled, entry_of};
use crate::errors::{io_error, local_error};
use crate::paths::{resolve, to_os_path};

/// Préfixe et suffixe des fichiers temporaires d'écriture.
pub(crate) const STAGING_PREFIX: &str = ".syncengine-";
/// Suffixe des fichiers temporaires d'écriture.
pub(crate) const STAGING_SUFFIX: &str = ".partial";

/// Compteur de noms temporaires, unique dans le processus.
static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

fn staging_name() -> String {
    let counter = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{STAGING_PREFIX}{}-{nanos}-{counter}{STAGING_SUFFIX}",
        std::process::id()
    )
}

/// Chemin OS du dossier parent de `os_path` (toujours présent : la racine
/// est refusée en amont).
fn parent_of(os_path: &Path) -> &Path {
    os_path.parent().unwrap_or(os_path)
}

/// Vérifie que la position réelle de `parent_os` est sous celle de la racine.
fn ensure_within_root(
    root: &Path,
    parent_os: &Path,
    path: &ResourcePath,
    operation: StorageOperation,
) -> Result<(), StorageError> {
    let real_root =
        fs::canonicalize(root).map_err(|e| io_error(e, operation, &ResourcePath::root()))?;
    let real_parent = fs::canonicalize(parent_os).map_err(|e| io_error(e, operation, path))?;
    if real_parent.starts_with(&real_root) {
        Ok(())
    } else {
        Err(local_error(
            StorageErrorKind::Permanent,
            operation,
            path,
            "le dossier parent n'est plus sous la racine (lien ou jonction introduit ?) : refus",
        ))
    }
}

/// Contrôles de chemin avant une mutation ; renvoie le chemin OS de la cible.
fn prepare(
    root: &Path,
    path: &ResourcePath,
    operation: StorageOperation,
) -> Result<PathBuf, StorageError> {
    let os_target = to_os_path(root, path, operation)?;
    let parent = path.parent().unwrap_or_default();
    if !parent.is_root() {
        let (_, metadata) = resolve(root, &parent, operation)?;
        if metadata.file_type().is_symlink() {
            return Err(local_error(
                StorageErrorKind::Permanent,
                operation,
                &parent,
                "lien symbolique ou jonction dans le chemin : non suivi",
            ));
        }
        if !metadata.is_dir() {
            return Err(local_error(
                StorageErrorKind::NotFound,
                operation,
                path,
                "le parent n'est pas un répertoire",
            ));
        }
    }
    ensure_within_root(root, parent_of(&os_target), path, operation)?;
    Ok(os_target)
}

/// Rend durable l'entrée du répertoire `dir` (création, renommage,
/// suppression). Sous Windows, NTFS journalise les métadonnées et un
/// répertoire ne peut pas être ouvert par `File::open` : rien à faire.
fn sync_directory(
    dir: &Path,
    path: &ResourcePath,
    operation: StorageOperation,
) -> Result<(), StorageError> {
    if cfg!(unix) {
        File::open(dir)
            .and_then(|handle| handle.sync_all())
            .map_err(|e| io_error(e, operation, path))?;
    }
    Ok(())
}

fn conflict(operation: StorageOperation, path: &ResourcePath, message: &str) -> StorageError {
    local_error(StorageErrorKind::Conflict, operation, path, message)
}

/// Crée un répertoire (parent existant, sans écrasement).
pub(crate) fn create_directory(
    root: &Path,
    path: &ResourcePath,
    precision: TimestampPrecision,
) -> Result<FileEntry, StorageError> {
    let operation = StorageOperation::CreateDirectory;
    let os_path = prepare(root, path, operation)?;
    match fs::create_dir(&os_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(conflict(
                operation,
                path,
                "une entrée existe déjà à ce chemin",
            ));
        }
        Err(error) => return Err(io_error(error, operation, path)),
    }
    sync_directory(parent_of(&os_path), path, operation)?;
    let metadata = fs::symlink_metadata(&os_path).map_err(|e| io_error(e, operation, path))?;
    Ok(entry_of(path.clone(), &metadata, precision))
}

/// Supprime un fichier ou un répertoire vide, sans suivre de lien.
pub(crate) fn delete(
    root: &Path,
    path: &ResourcePath,
    expected: EntryKind,
) -> Result<(), StorageError> {
    let operation = StorageOperation::Delete;
    let os_path = prepare(root, path, operation)?;
    let metadata = fs::symlink_metadata(&os_path).map_err(|e| io_error(e, operation, path))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(local_error(
            StorageErrorKind::NotSupported,
            operation,
            path,
            "lien symbolique ou jonction : jamais supprimé",
        ));
    }
    match expected {
        EntryKind::File if file_type.is_file() => {
            fs::remove_file(&os_path).map_err(|e| io_error(e, operation, path))?;
        }
        EntryKind::Directory if file_type.is_dir() => match fs::remove_dir(&os_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => {
                return Err(conflict(
                    operation,
                    path,
                    "répertoire non vide : jamais de suppression récursive",
                )
                .with_source(error));
            }
            Err(error) => return Err(io_error(error, operation, path)),
        },
        EntryKind::File | EntryKind::Directory => {
            return Err(conflict(
                operation,
                path,
                "la nature de l'entrée diffère de celle attendue",
            ));
        }
        _ => {
            return Err(local_error(
                StorageErrorKind::NotSupported,
                operation,
                path,
                "seuls un fichier ou un répertoire vide peuvent être supprimés",
            ));
        }
    }
    sync_directory(parent_of(&os_path), path, operation)
}

/// Vrai si l'échec d'un lien physique signifie que le volume n'en prend
/// pas en charge (FAT, certains partages réseau).
fn hard_links_unsupported(error: &io::Error) -> bool {
    #[cfg(windows)]
    const CODES: &[i32] = &[
        1,  // ERROR_INVALID_FUNCTION
        50, // ERROR_NOT_SUPPORTED
    ];
    #[cfg(unix)]
    const CODES: &[i32] = &[
        1,  // EPERM (FAT, systèmes sans liens physiques)
        18, // EXDEV
        95, // EOPNOTSUPP
    ];
    #[cfg(not(any(windows, unix)))]
    const CODES: &[i32] = &[];
    error.kind() == io::ErrorKind::Unsupported
        || error
            .raw_os_error()
            .is_some_and(|code| CODES.contains(&code))
}

/// Retire un fichier temporaire ; `true` si c'est fait (ou s'il n'existe plus).
fn remove_staging(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

/// Écriture en deux temps d'un fichier local.
pub(crate) struct LocalWriter {
    file: Option<File>,
    root: PathBuf,
    staging_os: PathBuf,
    staging: ResourcePath,
    target_os: PathBuf,
    target: ResourcePath,
    options: WriteOptions,
    precision: TimestampPrecision,
    cancel: CancellationToken,
    finished: bool,
}

impl LocalWriter {
    /// Crée le fichier temporaire dans le dossier de la cible.
    pub(crate) fn open(
        root: &Path,
        path: &ResourcePath,
        options: WriteOptions,
        precision: TimestampPrecision,
        cancel: &CancellationToken,
    ) -> Result<Self, StorageError> {
        let operation = StorageOperation::Write;
        let target_os = prepare(root, path, operation)?;
        let name = staging_name();
        let staging = path.parent().unwrap_or_default().join(&name).map_err(|_| {
            local_error(
                StorageErrorKind::Permanent,
                operation,
                path,
                "nom temporaire invalide",
            )
        })?;
        let staging_os = parent_of(&target_os).join(&name);
        // `create_new` : refuse un nom existant, lien compris (jamais suivi).
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging_os)
            .map_err(|e| io_error(e, operation, path))?;
        Ok(Self {
            file: Some(file),
            root: root.to_path_buf(),
            staging_os,
            staging,
            target_os,
            target: path.clone(),
            options,
            precision,
            cancel: cancel.clone(),
            finished: false,
        })
    }

    fn publish(&mut self, mode: CommitMode) -> Result<FileEntry, StorageError> {
        let operation = StorageOperation::Write;
        if self.cancel.is_cancelled() {
            return Err(cancelled(operation, &self.target));
        }
        let file = self.file.take().ok_or_else(|| {
            local_error(
                StorageErrorKind::Permanent,
                operation,
                &self.target,
                "écriture déjà close",
            )
        })?;
        if let Some(modified) = self.options.modified.and_then(|ts| ts.to_system_time()) {
            file.set_modified(modified)
                .map_err(|e| io_error(e, operation, &self.target))?;
        }
        file.sync_all()
            .map_err(|e| io_error(e, operation, &self.target))?;
        drop(file);

        // Revérification juste avant de rendre le contenu visible.
        let parent_os = parent_of(&self.target_os).to_path_buf();
        ensure_within_root(&self.root, &parent_os, &self.target, operation)?;
        match mode {
            CommitMode::CreateNew => self.publish_new()?,
            CommitMode::ReplaceExisting => self.publish_replace()?,
            _ => {
                return Err(local_error(
                    StorageErrorKind::NotSupported,
                    operation,
                    &self.target,
                    "mode de validation non pris en charge",
                ));
            }
        }
        sync_directory(&parent_os, &self.target, operation)?;
        let metadata = fs::symlink_metadata(&self.target_os)
            .map_err(|e| io_error(e, operation, &self.target))?;
        Ok(entry_of(self.target.clone(), &metadata, self.precision))
    }

    /// Création sans écrasement : un lien physique échoue si la cible existe
    /// (atomique). Repli sans lien physique : contrôle puis renommage
    /// (fenêtre résiduelle documentée).
    fn publish_new(&self) -> Result<(), StorageError> {
        let operation = StorageOperation::Write;
        let exists = || {
            conflict(
                operation,
                &self.target,
                "la cible existe déjà : rien n'est écrasé",
            )
        };
        match fs::hard_link(&self.staging_os, &self.target_os) {
            Ok(()) => {
                if remove_staging(&self.staging_os) {
                    Ok(())
                } else {
                    Err(local_error(
                        StorageErrorKind::Permanent,
                        operation,
                        &self.staging,
                        "fichier publié, mais le fichier temporaire n'a pas pu être retiré",
                    ))
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(exists()),
            Err(error) if hard_links_unsupported(&error) => {
                match fs::symlink_metadata(&self.target_os) {
                    Ok(_) => Err(exists()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::rename(&self.staging_os, &self.target_os)
                            .map_err(|e| io_error(e, operation, &self.target))
                    }
                    Err(error) => Err(io_error(error, operation, &self.target)),
                }
            }
            Err(error) => Err(io_error(error, operation, &self.target)),
        }
    }

    /// Remplacement d'un fichier ordinaire existant par renommage (atomique
    /// sur un même volume sous Linux et Windows).
    fn publish_replace(&self) -> Result<(), StorageError> {
        let operation = StorageOperation::Write;
        match fs::symlink_metadata(&self.target_os) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => {
                return Err(conflict(
                    operation,
                    &self.target,
                    "la cible n'est plus un fichier ordinaire : rien n'est remplacé",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(conflict(
                    operation,
                    &self.target,
                    "la cible à remplacer a disparu",
                ));
            }
            Err(error) => return Err(io_error(error, operation, &self.target)),
        }
        fs::rename(&self.staging_os, &self.target_os)
            .map_err(|e| io_error(e, operation, &self.target))
    }
}

impl ContentWriter for LocalWriter {
    fn write(&mut self, data: &[u8]) -> Result<(), StorageError> {
        let operation = StorageOperation::Write;
        if self.cancel.is_cancelled() {
            return Err(cancelled(operation, &self.target));
        }
        let file = self.file.as_mut().ok_or_else(|| {
            local_error(
                StorageErrorKind::Permanent,
                operation,
                &self.target,
                "écriture déjà close",
            )
        })?;
        file.write_all(data)
            .map_err(|e| io_error(e, operation, &self.target))
    }

    fn staging_path(&self) -> Option<&ResourcePath> {
        Some(&self.staging)
    }

    fn commit(mut self: Box<Self>, mode: CommitMode) -> Result<FileEntry, StorageError> {
        let result = self.publish(mode);
        self.finished = true;
        if result.is_err() {
            self.file = None;
            // Échec : le fichier temporaire est retiré au mieux ; s'il reste,
            // il a été consigné par l'appelant via `staging_path`.
            let _removed: bool = remove_staging(&self.staging_os);
        }
        result
    }

    fn abort(mut self: Box<Self>) -> Result<(), StorageError> {
        self.finished = true;
        self.file = None;
        if remove_staging(&self.staging_os) {
            Ok(())
        } else {
            Err(local_error(
                StorageErrorKind::Permanent,
                StorageOperation::Write,
                &self.staging,
                "fichier temporaire non retiré",
            ))
        }
    }
}

impl Drop for LocalWriter {
    fn drop(&mut self) {
        if !self.finished {
            self.file = None;
            // Abandon implicite : retrait au mieux (voir `ContentWriter`).
            let _removed: bool = remove_staging(&self.staging_os);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_names_are_unique_and_recognisable() {
        let a = staging_name();
        let b = staging_name();
        assert_ne!(a, b);
        assert!(a.starts_with(STAGING_PREFIX) && a.ends_with(STAGING_SUFFIX));
        assert!(!crate::paths::is_ambiguous_on_windows(&a));
        assert!(ResourcePath::root().join(&a).is_ok());
    }
}
