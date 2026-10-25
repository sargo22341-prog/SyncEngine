use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use connector_api::{
    ClockSource, ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind as Kind, StorageFeature, StorageOperation as Op, SymlinkSupport,
};
use ssh2::{CheckResult, ErrorCode, FileStat, KnownHostFileKind, Session, Sftp};
use sync_core::{
    CancellationToken, ConnectorId, EntryKind, FileEntry, FileMetadata, ResourcePath, Timestamp,
    TimestampPrecision,
};

use crate::paging::Pages;

/// Configuration non secrète ; authentification séparée via agent SSH ou port externe.
/// Le socket doit correspondre à `host`, nom vérifié dans `known_hosts`.
#[derive(Clone)]
pub struct SftpOptions {
    /// Adresse déjà résolue ; évite un DNS bloquant non borné dans le connecteur.
    pub address: SocketAddr,
    /// Nom d'hôte attendu dans le fichier OpenSSH known_hosts.
    pub host: String,
    /// Compte distant, sans mot de passe.
    pub username: String,
    /// Fichier de confiance fourni par l'appelant, jamais modifié automatiquement.
    pub known_hosts: PathBuf,
    /// Racine POSIX absolue, sans lien symbolique dans ses composants.
    pub root: String,
}

/// Connecteur SFTP en lecture seule par défaut ; mutations par opt-in explicite.
/// Appels bloquants, timeout réseau de dix secondes.
/// Les clés restent dans l'agent SSH ; aucune acceptation automatique d'hôte.
pub struct SftpConnector {
    pub(super) options: SftpOptions,
    id: ConnectorId,
    pub(super) caps: StorageCapabilities,
    pages: Pages,
    listing_session: std::sync::Mutex<Option<Sftp>>,
    authentication: std::sync::Arc<dyn SftpAuthentication>,
}

/// Port d'authentification fourni par l'appelant, séparé des options persistables.
/// Il est invoqué seulement après validation stricte de la clé hôte.
/// Ne pas journaliser les secrets ou les erreurs serveur dans l'implémentation.
pub trait SftpAuthentication: Send + Sync {
    /// Authentifie la session déjà vérifiée pour ce compte.
    ///
    /// # Errors
    /// Erreur SSH ; le connecteur la remplace par `Unauthenticated`, sans texte serveur.
    fn authenticate(&self, session: &Session, username: &str) -> Result<(), ssh2::Error>;
}

struct AgentAuthentication;
impl SftpAuthentication for AgentAuthentication {
    fn authenticate(&self, session: &Session, username: &str) -> Result<(), ssh2::Error> {
        session.userauth_agent(username)
    }
}

pub(super) fn error(kind: Kind, op: Op, path: &ResourcePath) -> StorageError {
    StorageError::new(kind, op).with_path(path.clone())
}

pub(super) fn ssh_error(e: ssh2::Error, op: Op, path: &ResourcePath) -> StorageError {
    // Server text is untrusted and may echo credentials: retain only the numeric class.
    let kind = match e.code() {
        ErrorCode::SFTP(2 | 10) => Kind::NotFound,
        ErrorCode::SFTP(3) => Kind::PermissionDenied,
        ErrorCode::SFTP(6 | 7) => Kind::Disconnected,
        ErrorCode::SFTP(8) => Kind::NotSupported,
        ErrorCode::Session(-18 | -19) => Kind::Unauthenticated,
        ErrorCode::Session(-7 | -9 | -13 | -30 | -37 | -43) => Kind::Disconnected,
        _ => Kind::Permanent,
    };
    error(kind, op, path).with_message(format!("SSH/SFTP {:?}", e.code()))
}

impl SftpConnector {
    /// Autorise explicitement mkdir, dépôt temporaire et suppression non récursive.
    /// Le compte doit être confiné côté serveur et les écrivains concurrents exclus.
    #[must_use]
    pub fn with_mutations(mut self) -> Self {
        self.caps.features.extend([
            StorageFeature::Write,
            StorageFeature::StreamingWrite,
            StorageFeature::CreateDirectory,
            StorageFeature::Delete,
            StorageFeature::SetModifiedTime,
        ]);
        self
    }

    pub(super) fn for_writer(&self) -> Self {
        Self {
            options: self.options.clone(),
            id: self.id.clone(),
            caps: self.caps.clone(),
            pages: Pages::default(),
            listing_session: std::sync::Mutex::new(None),
            authentication: self.authentication.clone(),
        }
    }
    /// Valide la configuration sans ouvrir de connexion.
    ///
    /// # Errors
    /// `Permanent` si la racine POSIX, le compte ou l'hôte sont invalides.
    pub fn new(options: SftpOptions) -> Result<Self, StorageError> {
        Self::with_authentication(options, Box::new(AgentAuthentication))
    }

    /// Construit une source avec un service d'authentification externe.
    /// Aucun secret n'est copié dans les options ou les diagnostics.
    ///
    /// # Errors
    /// Identiques à [`Self::new`]. Aucun accès réseau à la construction.
    pub fn with_authentication(
        options: SftpOptions,
        authentication: Box<dyn SftpAuthentication>,
    ) -> Result<Self, StorageError> {
        if options.host.is_empty()
            || options.username.is_empty()
            || !options.root.starts_with('/')
            || options.root.contains(['\\', '\0'])
            || options.root.split('/').any(|s| s == "." || s == "..")
            || (options.root != "/" && options.root.ends_with('/'))
            || options.root.contains("//")
        {
            return Err(error(Kind::Permanent, Op::Connect, &ResourcePath::root()));
        }
        let id = ConnectorId::new("sftp")
            .map_err(|_| error(Kind::Permanent, Op::Connect, &ResourcePath::root()))?;
        let mut caps = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Read,
            StorageFeature::StreamingRead,
            StorageFeature::Cancellation,
        ]);
        caps.clock = ClockSource::Server;
        caps.symlinks = SymlinkSupport::Reported;
        caps.timestamp_precision = TimestampPrecision::Second;
        caps.pagination = Pages::capability();
        Ok(Self {
            options,
            id,
            caps,
            pages: Pages::default(),
            listing_session: std::sync::Mutex::new(None),
            authentication: authentication.into(),
        })
    }

    pub(super) fn connect(
        &self,
        op: Op,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Sftp, StorageError> {
        crate::paging::check(cancel, op, path)?;
        let mut last = None;
        for attempt in 0..3 {
            match self.connect_once(op, path) {
                Ok(sftp) => {
                    crate::paging::check(cancel, op, path)?;
                    return Ok(sftp);
                }
                Err(e) if e.kind().is_transient() && attempt < 2 => {
                    last = Some(e);
                    crate::paging::backoff(
                        cancel,
                        op,
                        path,
                        Duration::from_millis(100 << attempt),
                    )?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| error(Kind::Disconnected, op, path)))
    }

    fn connect_once(&self, op: Op, path: &ResourcePath) -> Result<Sftp, StorageError> {
        let timeout = Duration::from_secs(10);
        let tcp = TcpStream::connect_timeout(&self.options.address, timeout)
            .map_err(|_| error(Kind::Disconnected, op, path))?;
        tcp.set_read_timeout(Some(timeout))
            .map_err(|_| error(Kind::Permanent, op, path))?;
        tcp.set_write_timeout(Some(timeout))
            .map_err(|_| error(Kind::Permanent, op, path))?;
        let mut session = Session::new().map_err(|e| ssh_error(e, op, path))?;
        session.set_timeout(10_000);
        session.set_tcp_stream(tcp);
        session.handshake().map_err(|e| ssh_error(e, op, path))?;
        let mut hosts = session.known_hosts().map_err(|e| ssh_error(e, op, path))?;
        hosts
            .read_file(&self.options.known_hosts, KnownHostFileKind::OpenSSH)
            .map_err(|_| error(Kind::Unauthenticated, op, path))?;
        let (key, _) = session
            .host_key()
            .ok_or_else(|| error(Kind::Unauthenticated, op, path))?;
        if !matches!(
            hosts.check_port(&self.options.host, self.options.address.port(), key),
            CheckResult::Match
        ) {
            return Err(error(Kind::Unauthenticated, op, path)
                .with_message("clé hôte absente ou différente"));
        }
        self.authentication
            .authenticate(&session, &self.options.username)
            .map_err(|_| error(Kind::Unauthenticated, op, path))?;
        if !session.authenticated() {
            return Err(error(Kind::Unauthenticated, op, path));
        }
        session.sftp().map_err(|e| ssh_error(e, op, path))
    }

    pub(super) fn remote(
        &self,
        sftp: &Sftp,
        path: &ResourcePath,
        op: Op,
        allow_final_link: bool,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, StorageError> {
        // Check every root component too; PathBuf parsing on Windows is not POSIX parsing.
        let full = format!(
            "{}/{}",
            self.options.root.trim_end_matches('/'),
            path.as_str()
        );
        let full = if full.len() > 1 {
            full.trim_end_matches('/')
        } else {
            &full
        };
        let parts: Vec<_> = full.split('/').filter(|s| !s.is_empty()).collect();
        let mut checked = String::new();
        for (i, part) in parts.iter().enumerate() {
            crate::paging::check(cancel, op, path)?;
            checked.push('/');
            checked.push_str(part);
            let stat = sftp
                .lstat(Path::new(&checked))
                .map_err(|e| ssh_error(e, op, path))?;
            if stat.file_type().is_symlink()
                && !(allow_final_link && i + 1 == parts.len() && !path.is_root())
            {
                return Err(
                    error(Kind::NotSupported, op, path).with_message("lien symbolique refusé")
                );
            }
            if i + 1 < parts.len() && !stat.is_dir() {
                return Err(error(Kind::Conflict, op, path));
            }
        }
        Ok(PathBuf::from(full))
    }
}

pub(super) fn entry(path: ResourcePath, stat: FileStat) -> FileEntry {
    let kind = if stat.file_type().is_symlink() {
        EntryKind::Symlink
    } else if stat.is_dir() {
        EntryKind::Directory
    } else if stat.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    let mut meta = FileMetadata::unknown();
    if let Some(size) = stat.size {
        meta = meta.with_size(size);
    }
    if let Some(ts) = stat
        .mtime
        .and_then(|t| i64::try_from(t).ok())
        .and_then(|t| Timestamp::new(t, 0).ok())
    {
        meta = meta.with_modified(ts, TimestampPrecision::Second);
    }
    FileEntry::new(path, kind, meta)
}

impl StorageConnector for SftpConnector {
    fn create_directory(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        crate::mutations::mkdir(self, path, cancel)
    }
    fn open_write(
        &self,
        path: &ResourcePath,
        options: &connector_api::WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn connector_api::ContentWriter>, StorageError> {
        connector_api::ensure_supported(self, StorageFeature::Write, Op::Write)?;
        connector_staging::open(
            std::sync::Arc::new(self.for_writer()),
            path,
            *options,
            cancel,
        )
    }
    fn delete(
        &self,
        path: &ResourcePath,
        expected: EntryKind,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        crate::mutations::delete(self, path, expected, cancel)
    }
    fn connector_id(&self) -> &ConnectorId {
        &self.id
    }
    fn capabilities(&self) -> &StorageCapabilities {
        &self.caps
    }
    fn list(
        &self,
        path: &ResourcePath,
        page: Option<&PageToken>,
        cancel: &CancellationToken,
    ) -> Result<ListPage, StorageError> {
        crate::paging::check(cancel, Op::List, path)?;
        if let Some(token) = page {
            return self.pages.next(path, token);
        }
        self.pages.clear(path)?;
        let entries = self.metadata_request(Op::List, path, cancel, |session| {
            self.list_entries(session, path, cancel)
        })?;
        self.pages.start(path, entries)
    }

    fn stat(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        self.metadata_request(Op::Stat, path, cancel, |sftp| {
            let remote = self.remote(sftp, path, Op::Stat, true, cancel)?;
            let stat = sftp
                .lstat(&remote)
                .map_err(|e| ssh_error(e, Op::Stat, path))?;
            crate::paging::check(cancel, Op::Stat, path)?;
            Ok(entry(path.clone(), stat))
        })
    }

    fn open_read(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        let sftp = self.connect(Op::Read, path, cancel)?;
        let remote = self.remote(&sftp, path, Op::Read, false, cancel)?;
        let stat = sftp
            .lstat(&remote)
            .map_err(|e| ssh_error(e, Op::Read, path))?;
        if !stat.is_file() {
            return Err(error(Kind::NotSupported, Op::Read, path));
        }
        let file = sftp
            .open(&remote)
            .map_err(|e| ssh_error(e, Op::Read, path))?;
        Ok(Box::new(Reader {
            file,
            cancel: cancel.clone(),
            path: path.clone(),
            remaining: stat.size,
        }))
    }
}

struct Reader {
    file: ssh2::File,
    cancel: CancellationToken,
    path: ResourcePath,
    remaining: Option<u64>,
}
impl ContentReader for Reader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        crate::paging::check(&self.cancel, Op::Read, &self.path)?;
        let n = self
            .file
            .read(buf)
            .map_err(|_| error(Kind::Disconnected, Op::Read, &self.path))?;
        crate::paging::check(&self.cancel, Op::Read, &self.path)?;
        if let Some(left) = &mut self.remaining {
            if n == 0 && !buf.is_empty() && *left != 0 {
                return Err(error(Kind::Disconnected, Op::Read, &self.path));
            }
            *left = left
                .checked_sub(n as u64)
                .ok_or_else(|| error(Kind::Conflict, Op::Read, &self.path))?;
        }
        Ok(n)
    }
}

impl SftpConnector {
    fn list_entries(
        &self,
        sftp: &Sftp,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Vec<FileEntry>, StorageError> {
        let remote = self.remote(sftp, path, Op::List, false, cancel)?;
        let mut dir = sftp
            .opendir(&remote)
            .map_err(|e| ssh_error(e, Op::List, path))?;
        let mut entries = Vec::new();
        loop {
            if cancel.checkpoint() {
                return Err(error(Kind::Cancelled, Op::List, path));
            }
            // ssh2 0.9.6 uses from_utf8(...).unwrap() for filenames on Windows.
            // Contain that dependency panic; discard the listing/connection instead
            // of reporting a complete directory or losing the caller's job state.
            let item = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dir.readdir()))
                .map_err(|_| {
                    error(Kind::NotSupported, Op::List, path)
                        .with_message("nom distant non représentable")
                })?;
            match item {
                Ok((name, stat)) => {
                    let name = name
                        .to_str()
                        .ok_or_else(|| error(Kind::NotSupported, Op::List, path))?;
                    if name == "." || name == ".." {
                        continue;
                    }
                    let child = path
                        .join(name)
                        .map_err(|_| error(Kind::NotSupported, Op::List, path))?;
                    entries.push(entry(child, stat));
                    if entries.len() > 10_000 {
                        return Err(error(Kind::NotSupported, Op::List, path)
                            .with_message("limite de 10000 entrées"));
                    }
                }
                Err(e) if e.code() == ErrorCode::Session(-16) => break,
                Err(e) => return Err(ssh_error(e, Op::List, path)),
            }
        }
        Ok(entries)
    }
}

impl SftpConnector {
    fn metadata_request<T>(
        &self,
        op: Op,
        path: &ResourcePath,
        cancel: &CancellationToken,
        mut request: impl FnMut(&Sftp) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut slot = self
            .listing_session
            .lock()
            .map_err(|_| error(Kind::Permanent, op, path))?;
        crate::paging::check(cancel, op, path)?;
        for attempt in 0..2 {
            let session = match slot.take() {
                Some(session) => session,
                None => self.connect(op, path, cancel)?,
            };
            match request(&session) {
                Ok(entries) => {
                    *slot = Some(session);
                    crate::paging::check(cancel, op, path)?;
                    return Ok(entries);
                }
                Err(e) if e.kind().is_transient() && attempt == 0 => {
                    drop(session);
                    crate::paging::backoff(cancel, op, path, Duration::from_millis(100))?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(error(Kind::Disconnected, op, path))
    }
}
