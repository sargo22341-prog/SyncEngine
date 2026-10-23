use crate::paging::{self, Pages};
use connector_api::{
    ClockSource, ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind as Kind, StorageFeature, StorageOperation as Op, WriteOptions,
};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use suppaftp::{FtpError, Mode, NativeTlsFtpStream, TlsStream, TransferStream, types::FileType};
use sync_core::{CancellationToken, ConnectorId, EntryKind, FileEntry, FileMetadata, ResourcePath};

/// Configuration runtime sans secret, POSIX et relative au compte FTP confiné.
#[derive(Clone)]
pub struct Options {
    /// Adresse résolue du serveur, sans DNS bloquant.
    pub address: SocketAddr,
    /// Racine POSIX absolue sous le compte distant confiné.
    pub root: String,
}
/// Identifiants uniquement en mémoire, jamais Debug/sérialisés ou recopiés dans une erreur.
pub struct Credentials {
    user: String,
    password: String,
}
impl Credentials {
    /// Construit des identifiants fournis par un service externe.
    /// # Errors
    /// `Permanent` sur champ vide ou contrôle CR/LF/NUL (injection de commande).
    pub fn new(user: &str, password: &str) -> Result<Self, StorageError> {
        if user.is_empty()
            || [user, password]
                .iter()
                .any(|s| s.contains(['\r', '\n', '\0']))
        {
            return Err(err(Kind::Permanent, Op::Connect, &ResourcePath::root()));
        }
        Ok(Self {
            user: user.into(),
            password: password.into(),
        })
    }
}
/// Implémentation interne partagée, ne pas utiliser comme choix de provider UI.
pub struct Endpoint {
    pub(crate) options: Options,
    credentials: Arc<Credentials>,
    domain: Option<String>,
    root_certificate: Option<suppaftp::native_tls::Certificate>,
    id: ConnectorId,
    pub(crate) caps: StorageCapabilities,
    pages: Pages,
    pub(crate) owns_lock: std::sync::Mutex<bool>,
}
pub(crate) fn err(kind: Kind, op: Op, path: &ResourcePath) -> StorageError {
    StorageError::new(kind, op).with_path(path.clone())
}
pub(crate) fn ftp_error(error: FtpError, op: Op, path: &ResourcePath) -> StorageError {
    let kind = match error {
        FtpError::ConnectionError(_) => Kind::Disconnected,
        FtpError::UnexpectedResponse(r) => match r.status.code() {
            421 | 425 | 426 => Kind::Disconnected,
            450 | 451 => Kind::Transient,
            452 | 552 => Kind::InsufficientSpace,
            530 | 532 => Kind::Unauthenticated,
            550 => Kind::PermissionDenied,
            500 | 502 | 504 => Kind::NotSupported,
            553 => Kind::Conflict,
            _ => Kind::Permanent,
        },
        _ => Kind::Permanent,
    };
    err(kind, op, path)
}
pub(crate) fn command(path: &ResourcePath, op: Op) -> Result<&str, StorageError> {
    if path.as_str().contains(['\r', '\n', '\0']) {
        return Err(err(Kind::Permanent, op, path));
    }
    Ok(if path.is_root() { "." } else { path.as_str() })
}
impl Endpoint {
    /// Crée le transport FTP clair après acquittement explicite du risque.
    /// # Errors
    /// Configuration invalide ou `acknowledge_plaintext=false`.
    pub fn ftp(
        options: Options,
        credentials: Credentials,
        acknowledge_plaintext: bool,
    ) -> Result<Self, StorageError> {
        if !acknowledge_plaintext {
            return Err(err(Kind::NotSupported, Op::Connect, &ResourcePath::root()));
        }
        Self::new(options, credentials, None, "ftp")
    }
    /// Crée exclusivement FTPS explicite ; aucune connexion claire de repli.
    /// # Errors
    /// Nom TLS absent ou configuration invalide.
    pub fn ftps(
        options: Options,
        credentials: Credentials,
        domain: String,
    ) -> Result<Self, StorageError> {
        if domain.is_empty() {
            return Err(err(Kind::Permanent, Op::Connect, &ResourcePath::root()));
        }
        Self::new(options, credentials, Some(domain), "ftps")
    }
    fn new(
        options: Options,
        credentials: Credentials,
        domain: Option<String>,
        name: &str,
    ) -> Result<Self, StorageError> {
        if !options.root.starts_with('/')
            || options.root.contains(['\r', '\n', '\0', '\\'])
            || options.root.contains("//")
            || options.root.split('/').any(|p| p == "." || p == "..")
        {
            return Err(err(Kind::Permanent, Op::Connect, &ResourcePath::root()));
        }
        let id = ConnectorId::new(name)
            .map_err(|_| err(Kind::Permanent, Op::Connect, &ResourcePath::root()))?;
        let mut caps = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Read,
            StorageFeature::StreamingRead,
            StorageFeature::Cancellation,
        ]);
        caps.clock = ClockSource::Server;
        caps.pagination = Pages::capability();
        Ok(Self {
            options,
            credentials: Arc::new(credentials),
            domain,
            root_certificate: None,
            id,
            caps,
            pages: Pages::default(),
            owns_lock: std::sync::Mutex::new(false),
        })
    }
    /// Autorise les mutations sous verrou serveur ; l'appelant exclut aussi les
    /// clients externes qui ne respectent pas `.syncengine-lock`.
    #[must_use]
    pub fn with_exclusive_mutations(mut self) -> Self {
        self.caps.features.extend([
            StorageFeature::Write,
            StorageFeature::StreamingWrite,
            StorageFeature::CreateDirectory,
            StorageFeature::Delete,
        ]);
        self
    }
    /// Ajoute une CA explicite sans désactiver validation TLS ou nom d'hôte.
    /// # Errors
    /// PEM invalide ou transport FTP clair.
    pub fn with_root_certificate(mut self, pem: &[u8]) -> Result<Self, StorageError> {
        if self.domain.is_none() {
            return Err(err(Kind::NotSupported, Op::Connect, &ResourcePath::root()));
        }
        self.root_certificate = Some(
            suppaftp::native_tls::Certificate::from_pem(pem)
                .map_err(|_| err(Kind::Permanent, Op::Connect, &ResourcePath::root()))?,
        );
        Ok(self)
    }
    pub(crate) fn writer(&self) -> Self {
        Self {
            options: self.options.clone(),
            credentials: self.credentials.clone(),
            domain: self.domain.clone(),
            root_certificate: self.root_certificate.clone(),
            id: self.id.clone(),
            caps: self.caps.clone(),
            pages: Pages::default(),
            owns_lock: std::sync::Mutex::new(false),
        }
    }
    pub(crate) fn connect(
        &self,
        op: Op,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<NativeTlsFtpStream, StorageError> {
        for attempt in 0..3 {
            paging::check(cancel, op, path)?;
            match self.connect_once(op, path) {
                Ok(c) => return Ok(c),
                Err(e) if e.kind().is_transient() && attempt < 2 => {
                    paging::backoff(cancel, op, path, Duration::from_millis(100 << attempt))?
                }
                Err(e) => return Err(e),
            }
        }
        Err(err(Kind::Disconnected, op, path))
    }
    fn connect_once(
        &self,
        op: Op,
        path: &ResourcePath,
    ) -> Result<NativeTlsFtpStream, StorageError> {
        let timeout = Duration::from_secs(10);
        let socket = TcpStream::connect_timeout(&self.options.address, timeout)
            .map_err(|_| err(Kind::Disconnected, op, path))?;
        socket
            .set_read_timeout(Some(timeout))
            .and_then(|()| socket.set_write_timeout(Some(timeout)))
            .map_err(|_| err(Kind::Permanent, op, path))?;
        let mut ftp =
            NativeTlsFtpStream::connect_with_stream(socket).map_err(|e| ftp_error(e, op, path))?;
        if let Some(domain) = &self.domain {
            let mut builder = suppaftp::native_tls::TlsConnector::builder();
            builder.min_protocol_version(Some(suppaftp::native_tls::Protocol::Tlsv12));
            if let Some(cert) = &self.root_certificate {
                builder.add_root_certificate(cert.clone());
            }
            let tls = builder
                .build()
                .map_err(|_| err(Kind::Permanent, op, path))?;
            ftp = ftp
                .into_secure(suppaftp::NativeTlsConnector::from(tls), domain)
                .map_err(|_| err(Kind::Unauthenticated, op, path))?;
        }
        let control_ip = self.options.address.ip();
        ftp = ftp.passive_stream_builder(move |addr| {
            if addr.ip() != control_ip {
                return Err(FtpError::BadResponse);
            }
            let socket =
                TcpStream::connect_timeout(&addr, timeout).map_err(FtpError::ConnectionError)?;
            socket
                .set_read_timeout(Some(timeout))
                .and_then(|()| socket.set_write_timeout(Some(timeout)))
                .map_err(FtpError::ConnectionError)?;
            Ok(socket)
        });
        ftp.set_mode(Mode::ExtendedPassive);
        ftp.login(&self.credentials.user, &self.credentials.password)
            .map_err(|_| err(Kind::Unauthenticated, op, path))?;
        ftp.cwd(&self.options.root)
            .map_err(|e| ftp_error(e, op, path))?;
        ftp.transfer_type(FileType::Binary)
            .map_err(|e| ftp_error(e, op, path))?;
        Ok(ftp)
    }
    pub(crate) fn entries(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Vec<FileEntry>, StorageError> {
        let mut ftp = self.connect(Op::List, path, cancel)?;
        let (_, mut data) = ftp
            .custom_data_command(
                format!("MLSD {}", command(path, Op::List)?),
                &[suppaftp::Status::AlreadyOpen, suppaftp::Status::AboutToSend],
            )
            .map_err(|e| ftp_error(e, Op::List, path))?;
        let mut bytes = Vec::new();
        let mut buf = [0; 16384];
        loop {
            paging::check(cancel, Op::List, path)?;
            let n = data
                .read(&mut buf)
                .map_err(|_| err(Kind::Disconnected, Op::List, path))?;
            if n == 0 {
                break;
            }
            if bytes.len() + n > 8 * 1024 * 1024 {
                return Err(err(Kind::NotSupported, Op::List, path));
            }
            bytes.extend_from_slice(&buf[..n]);
        }
        data.finish().map_err(|e| ftp_error(e, Op::List, path))?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| err(Kind::NotSupported, Op::List, path))?;
        let mut entries = Vec::new();
        for line in text.lines() {
            let (facts, name) = line
                .split_once(' ')
                .ok_or_else(|| err(Kind::Permanent, Op::List, path))?;
            if facts
                .split(';')
                .any(|f| f.eq_ignore_ascii_case("type=cdir") || f.eq_ignore_ascii_case("type=pdir"))
            {
                continue;
            }
            let child = path
                .join(name)
                .map_err(|_| err(Kind::NotSupported, Op::List, path))?;
            command(&child, Op::List)?;
            entries.push(parse_facts(facts, child)?);
            if entries.len() > 10000 {
                return Err(err(Kind::NotSupported, Op::List, path));
            }
        }
        entries.sort_by(|a, b| a.path().cmp(b.path()));
        if entries.windows(2).any(|w| w[0].path() == w[1].path()) {
            return Err(err(Kind::Conflict, Op::List, path));
        }
        Ok(entries)
    }
}
pub(crate) fn parse_facts(facts: &str, path: ResourcePath) -> Result<FileEntry, StorageError> {
    let mut kind = None;
    let mut meta = FileMetadata::unknown();
    for fact in facts.split(';').filter(|s| !s.is_empty()) {
        let (key, value) = fact
            .split_once('=')
            .ok_or_else(|| err(Kind::Permanent, Op::Stat, &path))?;
        if key.eq_ignore_ascii_case("type") {
            if kind.is_some() {
                return Err(err(Kind::Permanent, Op::Stat, &path));
            }
            kind = Some(match value.to_ascii_lowercase().as_str() {
                "dir" | "cdir" => EntryKind::Directory,
                "file" => EntryKind::File,
                "os.unix=slink" => EntryKind::Symlink,
                _ => EntryKind::Other,
            });
        } else if key.eq_ignore_ascii_case("size") {
            meta = meta.with_size(
                value
                    .parse()
                    .map_err(|_| err(Kind::Permanent, Op::Stat, &path))?,
            );
        }
    }
    Ok(FileEntry::new(
        path.clone(),
        kind.ok_or_else(|| err(Kind::Permanent, Op::Stat, &path))?,
        meta,
    ))
}
impl StorageConnector for Endpoint {
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
        paging::check(cancel, Op::List, path)?;
        if let Some(p) = page {
            return self.pages.next(path, p);
        }
        self.pages.clear(path)?;
        self.pages.start(path, self.entries(path, cancel)?)
    }
    fn stat(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        command(path, Op::Stat)?;
        if let Some(parent) = path.parent() {
            return self
                .entries(&parent, cancel)?
                .into_iter()
                .find(|e| e.path() == path)
                .ok_or_else(|| err(Kind::NotFound, Op::Stat, path));
        }
        let mut ftp = self.connect(Op::Stat, path, cancel)?;
        let line = ftp
            .mlst(Some("."))
            .map_err(|e| ftp_error(e, Op::Stat, path))?;
        parse_facts(
            line.split_once(' ')
                .map(|v| v.0)
                .ok_or_else(|| err(Kind::Permanent, Op::Stat, path))?,
            path.clone(),
        )
    }
    fn open_read(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        let entry = self.stat(path, cancel)?;
        if entry.kind() != EntryKind::File {
            return Err(err(Kind::NotSupported, Op::Read, path));
        }
        let mut ftp = self.connect(Op::Read, path, cancel)?;
        let data = ftp
            .retr_as_stream(command(path, Op::Read)?)
            .map_err(|e| ftp_error(e, Op::Read, path))?;
        Ok(Box::new(Reader {
            data: Some(data),
            path: path.clone(),
            cancel: cancel.clone(),
            remaining: entry.metadata().size(),
        }))
    }
    fn create_directory(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        crate::mutations::mkdir(self, path, cancel)
    }
    fn delete(
        &self,
        path: &ResourcePath,
        kind: EntryKind,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        crate::mutations::delete(self, path, kind, cancel)
    }
    fn open_write(
        &self,
        path: &ResourcePath,
        options: &WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn connector_api::ContentWriter>, StorageError> {
        connector_api::ensure_supported(self, StorageFeature::Write, Op::Write)?;
        if options.modified.is_some() {
            return Err(err(Kind::NotSupported, Op::Write, path));
        }
        connector_staging::open(Arc::new(self.writer()), path, *options, cancel)
    }
}
struct Reader<T: TlsStream> {
    data: Option<TransferStream<T>>,
    path: ResourcePath,
    cancel: CancellationToken,
    remaining: Option<u64>,
}
impl<T: TlsStream + Send> ContentReader for Reader<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        paging::check(&self.cancel, Op::Read, &self.path)?;
        let Some(data) = self.data.as_mut() else {
            return Ok(0);
        };
        let n = data
            .read(buf)
            .map_err(|_| err(Kind::Disconnected, Op::Read, &self.path))?;
        if let Some(left) = self.remaining.as_mut() {
            if n == 0 && !buf.is_empty() && *left != 0 {
                return Err(err(Kind::Disconnected, Op::Read, &self.path));
            }
            *left = left
                .checked_sub(n as u64)
                .ok_or_else(|| err(Kind::Conflict, Op::Read, &self.path))?;
        }
        if n == 0
            && !buf.is_empty()
            && let Some(data) = self.data.take()
        {
            data.finish()
                .map_err(|e| ftp_error(e, Op::Read, &self.path))?;
        }
        Ok(n)
    }
}
