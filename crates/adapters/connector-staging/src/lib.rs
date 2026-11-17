//! Dépôt en deux temps pour les protocoles qui doivent fermer l'upload avant vérification.
use connector_api::{
    CommitMode, ContentWriter, StorageError, StorageErrorKind as Kind, StorageOperation as Op,
    WriteOptions,
};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::sync::Arc;
use sync_core::{CancellationToken, FileEntry, ResourcePath};

/// Opérations de transport ; aucune règle de synchronisation dans ce port.
pub trait RemoteStage: Send + Sync {
    /// Réserve un répertoire neuf, atomiquement ; une collision est une erreur.
    /// # Errors
    /// Erreur distante ; ne pas revendiquer une réservation après une réponse perdue.
    fn reserve(
        &self,
        directory: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError>;
    /// Transfère vers `directory/content` dans la réservation appartenant à l'appelant.
    /// # Errors
    /// Erreur de transport/annulation ; ne modifie jamais la cible finale.
    fn upload(
        &self,
        path: &ResourcePath,
        file: File,
        options: WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError>;
    /// Publie sans suppression préalable de la cible ; aucun retry de mutation ambiguë.
    /// # Errors
    /// Conflit ou résultat distant incertain, à diagnostiquer avant reprise.
    fn publish(
        &self,
        stage: &ResourcePath,
        target: &ResourcePath,
        mode: CommitMode,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError>;
    /// Retire le contenu réservé puis le répertoire vide ; jamais récursivement.
    /// # Errors
    /// Retrait impossible ; le chemin de dépôt reste diagnostiquable.
    fn cleanup(&self, stage: &ResourcePath, directory: &ResourcePath) -> Result<(), StorageError>;
}

fn err(kind: Kind, path: &ResourcePath) -> StorageError {
    StorageError::new(kind, Op::Write).with_path(path.clone())
}

/// Variable qui impose le dossier de dépôt (chemin absolu).
pub const SPOOL_DIR_VARIABLE: &str = "SYNCENGINE_SPOOL_DIR";

/// Dossier des dépôts d'envoi, selon les variables d'environnement lues par
/// `var` (séparé pour les tests) :
/// 1. `SYNCENGINE_SPOOL_DIR` s'il est absolu ;
/// 2. hors Windows : `$XDG_CACHE_HOME/SyncEngine/spool`, sinon
///    `~/.cache/SyncEngine/spool` — sur disque, alors que `/tmp` est souvent
///    un tmpfs en RAM (Debian 13, Fedora, Arch) ;
/// 3. sinon le dossier temporaire du système (Windows : `%TEMP%`, sur disque).
#[must_use]
pub fn spool_dir_with(var: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> std::path::PathBuf {
    use std::path::PathBuf;
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    if let Some(dir) = absolute(SPOOL_DIR_VARIABLE) {
        return dir;
    }
    if !cfg!(windows)
        && let Some(cache) =
            absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|home| home.join(".cache")))
    {
        return cache.join("SyncEngine").join("spool");
    }
    std::env::temp_dir()
}

/// [`spool_dir_with`] pour l'environnement de ce processus.
#[must_use]
pub fn spool_dir() -> std::path::PathBuf {
    spool_dir_with(&|name| std::env::var_os(name))
}

/// Marge gardée libre en plus du fichier (journaux, système).
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// Refuse un dépôt de `needed` octets si l'espace libre connu de `dir` ne le
/// permet pas (Linux : `statvfs` ; ailleurs : non vérifié, l'écriture
/// signalera `InsufficientSpace`).
fn check_space(
    dir: &std::path::Path,
    needed: u64,
    target: &ResourcePath,
) -> Result<(), StorageError> {
    #[cfg(target_os = "linux")]
    if let Ok(stats) = rustix::fs::statvfs(dir) {
        let free = stats.f_bavail.saturating_mul(stats.f_frsize);
        if free < needed.saturating_add(SPACE_MARGIN) {
            return Err(err(Kind::InsufficientSpace, target).with_message(format!(
                "dossier de dépôt {} : {} Mio libres pour {} Mio à envoyer (réglable par {SPOOL_DIR_VARIABLE})",
                dir.display(),
                free / (1024 * 1024),
                needed.div_ceil(1024 * 1024)
            )));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (dir, needed, target, SPACE_MARGIN);
    Ok(())
}

/// Crée le dossier de dépôt (privé `0700` sous Unix).
fn ensure_spool_dir(dir: &std::path::Path, target: &ResourcePath) -> Result<(), StorageError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|e| {
        err(Kind::InsufficientSpace, target)
            .with_message(format!("dossier de dépôt {} : {e}", dir.display()))
    })
}

/// Ouvre un spool disque (dans [`spool_dir`], après contrôle de l'espace libre
/// quand la taille est annoncée) et réserve un répertoire distant unique de
/// même parent.
/// Le fichier final reste intact jusqu'à `commit`. Mémoire constante, espace disque
/// local égal à la taille transférée ; aucun secret dans le spool.
/// # Errors
/// Erreur locale, annulation, cible racine ou réservation distante refusée.
pub fn open(
    backend: Arc<dyn RemoteStage>,
    target: &ResourcePath,
    options: WriteOptions,
    cancel: &CancellationToken,
) -> Result<Box<dyn ContentWriter>, StorageError> {
    if cancel.is_cancelled() {
        return Err(err(Kind::Cancelled, target));
    }
    let parent = target.parent().ok_or_else(|| err(Kind::Conflict, target))?;
    let dir = spool_dir();
    ensure_spool_dir(&dir, target)?;
    if let Some(size) = options.size_hint {
        check_space(&dir, size, target)?;
    }
    let spool = tempfile::Builder::new()
        .prefix(".syncengine-")
        .suffix(".partial")
        .rand_bytes(16)
        .tempfile_in(&dir)
        .map_err(|_| err(Kind::InsufficientSpace, target))?;
    let name = spool
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| err(Kind::Permanent, target))?;
    let directory = parent
        .join(name)
        .map_err(|_| err(Kind::Permanent, target))?;
    let stage = directory
        .join("content")
        .map_err(|_| err(Kind::Permanent, target))?;
    backend.reserve(&directory, cancel)?;
    Ok(Box::new(Writer {
        backend,
        target: target.clone(),
        directory,
        stage,
        options,
        cancel: cancel.clone(),
        spool,
        state: State::Writing,
        owned: true,
    }))
}

enum State {
    Writing,
    Finished,
    Failed,
}
struct Writer {
    backend: Arc<dyn RemoteStage>,
    target: ResourcePath,
    directory: ResourcePath,
    stage: ResourcePath,
    options: WriteOptions,
    cancel: CancellationToken,
    spool: tempfile::NamedTempFile,
    state: State,
    owned: bool,
}
impl Writer {
    fn check(&self) -> Result<(), StorageError> {
        if self.cancel.is_cancelled() {
            Err(err(Kind::Cancelled, &self.target))
        } else {
            Ok(())
        }
    }
}
impl ContentWriter for Writer {
    fn write(&mut self, bytes: &[u8]) -> Result<(), StorageError> {
        self.check()?;
        if !matches!(self.state, State::Writing) {
            return Err(err(Kind::Conflict, &self.stage));
        }
        self.spool.write_all(bytes).map_err(|_| {
            self.state = State::Failed;
            err(Kind::InsufficientSpace, &self.stage)
        })
    }
    fn staging_path(&self) -> Option<&ResourcePath> {
        Some(&self.stage)
    }
    fn finish(&mut self) -> Result<(), StorageError> {
        self.check()?;
        if matches!(self.state, State::Finished) {
            return Ok(());
        }
        if matches!(self.state, State::Failed) {
            return Err(err(Kind::Conflict, &self.stage));
        }
        self.state = State::Failed;
        self.spool
            .flush()
            .and_then(|()| self.spool.as_file_mut().seek(SeekFrom::Start(0)))
            .map_err(|_| err(Kind::InsufficientSpace, &self.stage))?;
        let file = self
            .spool
            .as_file()
            .try_clone()
            .map_err(|_| err(Kind::Permanent, &self.stage))?;
        self.backend
            .upload(&self.stage, file, self.options, &self.cancel)?;
        self.state = State::Finished;
        Ok(())
    }
    fn commit(mut self: Box<Self>, mode: CommitMode) -> Result<FileEntry, StorageError> {
        self.finish()?;
        self.check()?;
        let entry = self
            .backend
            .publish(&self.stage, &self.target, mode, &self.cancel)?;
        // A successful publish followed by failed cleanup is still an error;
        // the final file may already exist and must be diagnosed, never replayed.
        self.owned = false;
        self.backend.cleanup(&self.stage, &self.directory)?;
        Ok(entry)
    }
    fn abort(mut self: Box<Self>) -> Result<(), StorageError> {
        self.owned = false;
        self.backend.cleanup(&self.stage, &self.directory)
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if self.owned {
            // Drop is best effort by contract; explicit abort preserves errors.
            if self.backend.cleanup(&self.stage, &self.directory).is_err() {
                self.owned = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use sync_core::{EntryKind, FileMetadata};
    struct Backend {
        uploaded: Mutex<Vec<u8>>,
        published: AtomicUsize,
        cleaned: AtomicUsize,
        fail_upload: bool,
        fail_cleanup: bool,
    }
    impl Backend {
        fn new() -> Self {
            Self {
                uploaded: Mutex::new(vec![]),
                published: AtomicUsize::new(0),
                cleaned: AtomicUsize::new(0),
                fail_upload: false,
                fail_cleanup: false,
            }
        }
    }
    impl RemoteStage for Backend {
        fn reserve(&self, _: &ResourcePath, _: &CancellationToken) -> Result<(), StorageError> {
            Ok(())
        }
        fn upload(
            &self,
            path: &ResourcePath,
            mut file: File,
            _: WriteOptions,
            _: &CancellationToken,
        ) -> Result<(), StorageError> {
            if self.fail_upload {
                return Err(err(Kind::Disconnected, path));
            }
            let mut content = self
                .uploaded
                .lock()
                .map_err(|_| err(Kind::Permanent, path))?;
            file.read_to_end(&mut content)
                .map_err(|_| err(Kind::Permanent, path))?;
            Ok(())
        }
        fn publish(
            &self,
            _: &ResourcePath,
            target: &ResourcePath,
            _: CommitMode,
            _: &CancellationToken,
        ) -> Result<FileEntry, StorageError> {
            self.published.fetch_add(1, Ordering::SeqCst);
            Ok(FileEntry::new(
                target.clone(),
                EntryKind::File,
                FileMetadata::unknown(),
            ))
        }
        fn cleanup(&self, path: &ResourcePath, _: &ResourcePath) -> Result<(), StorageError> {
            self.cleaned.fetch_add(1, Ordering::SeqCst);
            if self.fail_cleanup {
                Err(err(Kind::Disconnected, path))
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn the_spool_lives_on_disk_under_the_user_cache_unless_configured() {
        use std::ffi::OsString;
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        // Absolute on the running platform (`/srv` is not absolute on Windows).
        const FORCED: &str = if cfg!(windows) {
            r"C:\spool"
        } else {
            "/srv/spool"
        };
        const FORCED_ENV: &[(&str, &str)] =
            &[("SYNCENGINE_SPOOL_DIR", FORCED), ("HOME", "/home/u")];
        assert_eq!(
            spool_dir_with(&env(FORCED_ENV)),
            std::path::PathBuf::from(FORCED)
        );
        if !cfg!(windows) {
            assert_eq!(
                spool_dir_with(&env(&[("XDG_CACHE_HOME", "/c"), ("HOME", "/home/u")])),
                std::path::PathBuf::from("/c/SyncEngine/spool")
            );
            assert_eq!(
                spool_dir_with(&env(&[
                    ("HOME", "/home/u"),
                    ("SYNCENGINE_SPOOL_DIR", "relatif")
                ])),
                std::path::PathBuf::from("/home/u/.cache/SyncEngine/spool")
            );
        }
        assert_eq!(spool_dir_with(&env(&[])), std::env::temp_dir());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_spool_larger_than_the_free_space_is_refused_before_writing() {
        let dir = std::env::temp_dir();
        let target = ResourcePath::parse("big.bin").unwrap_or_default();
        let error = check_space(&dir, u64::MAX / 2, &target)
            .err()
            .map(|e| (e.kind(), e.to_string()));
        assert!(
            matches!(&error, Some((Kind::InsufficientSpace, text)) if text.contains("SYNCENGINE_SPOOL_DIR")),
            "{error:?}"
        );
        assert!(check_space(&dir, 1, &target).is_ok());
    }

    #[test]
    fn finish_uploads_once_without_publishing_and_closes_writes()
    -> Result<(), Box<dyn std::error::Error>> {
        let backend = Arc::new(Backend::new());
        let target = ResourcePath::parse("file")?;
        let mut writer = open(
            backend.clone(),
            &target,
            WriteOptions::new(),
            &CancellationToken::new(),
        )?;
        writer.write(b"abc")?;
        assert_eq!(backend.published.load(Ordering::SeqCst), 0);
        writer.finish()?;
        writer.finish()?;
        assert_eq!(&*backend.uploaded.lock().map_err(|_| "poison")?, b"abc");
        assert!(writer.write(b"late").is_err());
        assert_eq!(backend.published.load(Ordering::SeqCst), 0);
        writer.commit(CommitMode::CreateNew)?;
        assert_eq!(backend.published.load(Ordering::SeqCst), 1);
        assert_eq!(backend.cleaned.load(Ordering::SeqCst), 1);
        Ok(())
    }
    #[test]
    fn failed_upload_or_cancellation_never_publishes() -> Result<(), Box<dyn std::error::Error>> {
        let mut failed = Backend::new();
        failed.fail_upload = true;
        let backend = Arc::new(failed);
        let target = ResourcePath::parse("file")?;
        let cancel = CancellationToken::new();
        let mut writer = open(backend.clone(), &target, WriteOptions::new(), &cancel)?;
        writer.write(b"partial")?;
        assert_eq!(
            writer.finish().err().ok_or("no error")?.kind(),
            Kind::Disconnected
        );
        assert!(writer.commit(CommitMode::CreateNew).is_err());
        assert_eq!(backend.published.load(Ordering::SeqCst), 0);
        assert_eq!(backend.cleaned.load(Ordering::SeqCst), 1);
        let healthy = Arc::new(Backend::new());
        let writer = open(healthy.clone(), &target, WriteOptions::new(), &cancel)?;
        cancel.cancel();
        assert!(writer.commit(CommitMode::CreateNew).is_err());
        assert_eq!(healthy.published.load(Ordering::SeqCst), 0);
        Ok(())
    }
    #[test]
    fn explicit_abort_reports_cleanup_failure() -> Result<(), Box<dyn std::error::Error>> {
        let mut failed = Backend::new();
        failed.fail_cleanup = true;
        let backend = Arc::new(failed);
        let target = ResourcePath::parse("file")?;
        let writer = open(
            backend.clone(),
            &target,
            WriteOptions::new(),
            &CancellationToken::new(),
        )?;
        assert!(writer.staging_path().is_some());
        assert_eq!(
            writer.abort().err().ok_or("hidden cleanup failure")?.kind(),
            Kind::Disconnected
        );
        assert_eq!(backend.cleaned.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
