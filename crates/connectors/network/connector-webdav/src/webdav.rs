use std::io::Read;
use std::time::Duration;

use connector_api::{
    ClockSource, ContentReader, ListPage, PageToken, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind as Kind, StorageFeature, StorageOperation as Op,
};
use reqwest::{
    Method, Url,
    blocking::{Client, Response},
    header::{AUTHORIZATION, CONTENT_LENGTH, HeaderValue, RETRY_AFTER},
};
use sync_core::{CancellationToken, ConnectorId, EntryKind, FileEntry, ResourcePath};

use crate::paging::{self, Pages};
use crate::protocol::{decoded_path, parse};

const XML_LIMIT: u64 = 8 * 1024 * 1024;
const PROPERTIES: &str = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/></d:prop></d:propfind>"#;

/// Configuration non secrète WebDAV. La racine doit être une collection HTTPS.
pub struct WebDavOptions {
    /// URL HTTPS sans identifiants, query ni fragment, terminée par `/`.
    pub root: String,
}

/// Connecteur WebDAV, lecture seule par défaut. Pas de redirection, cookie, proxy implicite,
/// ni TLS non vérifié. Une instance ne conserve qu'un listing paginé actif.
pub struct WebDavConnector {
    pub(super) root: Url,
    pub(super) client: Client,
    pub(super) auth: HeaderValue,
    id: ConnectorId,
    pub(super) caps: StorageCapabilities,
    pages: Pages,
}

pub(super) fn error(kind: Kind, op: Op, path: &ResourcePath) -> StorageError {
    StorageError::new(kind, op).with_path(path.clone())
}

impl WebDavConnector {
    /// Active explicitement les dépôts et mutations de fichiers avec ETag fort.
    /// La suppression des collections utilisateur reste refusée (DELETE DAV récursif).
    #[must_use]
    pub fn with_mutations(mut self) -> Self {
        self.caps.features.extend([
            StorageFeature::Write,
            StorageFeature::StreamingWrite,
            StorageFeature::CreateDirectory,
            StorageFeature::Delete,
        ]);
        self
    }
    pub(super) fn for_writer(&self) -> Self {
        Self {
            root: self.root.clone(),
            client: self.client.clone(),
            auth: self.auth.clone(),
            id: self.id.clone(),
            caps: self.caps.clone(),
            pages: Pages::default(),
        }
    }
    /// Construit une source avec un jeton Bearer obtenu par l'appelant.
    /// Le jeton n'est jamais sérialisé, affiché ou inclus dans les erreurs.
    /// Appeler sur un thread bloquant, hors d'un exécuteur async.
    ///
    /// # Errors
    /// `Permanent` pour une URL ou un en-tête invalide ou un client indisponible.
    pub fn new(options: WebDavOptions, bearer: &str) -> Result<Self, StorageError> {
        let path = ResourcePath::root();
        let root =
            Url::parse(&options.root).map_err(|_| error(Kind::Permanent, Op::Connect, &path))?;
        if root.scheme() != "https"
            || root.host_str().is_none()
            || !root.username().is_empty()
            || root.password().is_some()
            || root.query().is_some()
            || root.fragment().is_some()
            || !root.path().ends_with('/')
            || bearer.is_empty()
            || decoded_path(root.path()).is_err()
        {
            return Err(error(Kind::Permanent, Op::Connect, &path));
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|_| error(Kind::Permanent, Op::Connect, &path))?;
        auth.set_sensitive(true);
        let client = Client::builder()
            .https_only(true)
            .tls_version_min(reqwest::tls::Version::TLS_1_2)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| error(Kind::Permanent, Op::Connect, &path))?;
        let id =
            ConnectorId::new("webdav").map_err(|_| error(Kind::Permanent, Op::Connect, &path))?;
        let mut caps = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Read,
            StorageFeature::StreamingRead,
            StorageFeature::Cancellation,
        ]);
        caps.clock = ClockSource::Server;
        caps.rate_limited = true;
        caps.pagination = Pages::capability();
        Ok(Self {
            root,
            client,
            auth,
            id,
            caps,
            pages: Pages::default(),
        })
    }

    pub(super) fn url(&self, path: &ResourcePath) -> Result<Url, StorageError> {
        let mut url = self.root.clone();
        if !path.is_root() {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| error(Kind::Permanent, Op::Connect, path))?;
            segments.pop_if_empty();
            for segment in path.segments() {
                segments.push(segment);
            }
        }
        Ok(url)
    }

    fn request(
        &self,
        path: &ResourcePath,
        depth: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Response, StorageError> {
        let op = if depth.is_some() { Op::List } else { Op::Read };
        let method = if depth.is_some() {
            Method::from_bytes(b"PROPFIND").map_err(|_| error(Kind::Permanent, op, path))?
        } else {
            Method::GET
        };
        for attempt in 0..3 {
            paging::check(cancel, op, path)?;
            let mut request = self
                .client
                .request(method.clone(), self.url(path)?)
                .header(AUTHORIZATION, self.auth.clone())
                .header("Accept-Encoding", "identity");
            if let Some(depth) = depth {
                request = request
                    .header("Depth", depth)
                    .header("Content-Type", "application/xml; charset=utf-8")
                    .body(PROPERTIES);
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(e) => {
                    // Never retain reqwest errors: they can contain URLs or signed queries.
                    let kind = if e.is_timeout() || e.is_connect() {
                        Kind::Disconnected
                    } else {
                        Kind::Permanent
                    };
                    if kind.is_transient() && attempt < 2 {
                        paging::backoff(cancel, op, path, Duration::from_millis(100 << attempt))?;
                        continue;
                    }
                    return Err(error(kind, op, path));
                }
            };
            paging::check(cancel, op, path)?;
            let expected = if depth.is_some() { 207 } else { 200 };
            if response.status().as_u16() == expected {
                return Ok(response);
            }
            let kind = status_kind(
                response.status().as_u16(),
                response.headers().get(RETRY_AFTER),
            );
            if attempt < 2 && kind.is_transient() {
                let delay = match kind {
                    Kind::Throttled {
                        retry_after: Some(delay),
                    } if delay <= Duration::from_secs(5) => delay,
                    Kind::Throttled { .. } => return Err(error(kind, op, path)),
                    _ => Duration::from_millis(100 << attempt),
                };
                drop(response);
                paging::backoff(cancel, op, path, delay)?;
            } else {
                return Err(error(kind, op, path));
            }
        }
        Err(error(Kind::Transient, op, path))
    }

    fn properties(
        &self,
        path: &ResourcePath,
        depth: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<FileEntry>, StorageError> {
        let mut response = self.request(path, Some(depth), cancel)?;
        let mut bytes = Vec::new();
        let mut block = [0; 16 * 1024];
        loop {
            paging::check(cancel, Op::List, path)?;
            let n = response
                .read(&mut block)
                .map_err(|_| error(Kind::Disconnected, Op::List, path))?;
            if n == 0 {
                break;
            }
            if bytes.len() as u64 + n as u64 > XML_LIMIT {
                return Err(error(Kind::NotSupported, Op::List, path));
            }
            bytes.extend_from_slice(&block[..n]);
        }
        paging::check(cancel, Op::List, path)?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| error(Kind::Permanent, Op::List, path))?;
        parse(&self.root, path, depth, text)
    }
}

pub(super) fn status_kind(status: u16, retry: Option<&HeaderValue>) -> Kind {
    match status {
        401 => Kind::Unauthenticated,
        403 => Kind::PermissionDenied,
        404 => Kind::NotFound,
        409 | 412 => Kind::Conflict,
        429 | 503 => Kind::Throttled {
            retry_after: retry
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(Duration::from_secs),
        },
        408 | 500 | 502 | 504 => Kind::Transient,
        405 | 501 => Kind::NotSupported,
        507 => Kind::InsufficientSpace,
        _ => Kind::Permanent,
    }
}

impl StorageConnector for WebDavConnector {
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
        if options.modified.is_some() {
            return Err(error(Kind::NotSupported, Op::Write, path));
        }
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
        paging::check(cancel, Op::List, path)?;
        if let Some(token) = page {
            return self.pages.next(path, token);
        }
        self.pages.clear(path)?;
        let entries = self
            .properties(path, "1", cancel)?
            .into_iter()
            .filter(|e| e.path() != path)
            .collect();
        self.pages.start(path, entries)
    }
    fn stat(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        self.properties(path, "0", cancel)
            .map_err(|e| error(e.kind(), Op::Stat, path))?
            .pop()
            .ok_or_else(|| error(Kind::Permanent, Op::Stat, path))
    }
    fn open_read(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        if self.stat(path, cancel)?.kind() != EntryKind::File {
            return Err(error(Kind::NotSupported, Op::Read, path));
        }
        let response = self.request(path, None, cancel)?;
        if response.headers().contains_key("Content-Encoding") {
            return Err(error(Kind::NotSupported, Op::Read, path));
        }
        let remaining = match response.headers().get(CONTENT_LENGTH) {
            Some(h) => Some(
                h.to_str()
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| error(Kind::Permanent, Op::Read, path))?,
            ),
            None => None,
        };
        Ok(Box::new(Reader {
            response,
            remaining,
            cancel: cancel.clone(),
            path: path.clone(),
        }))
    }
}

struct Reader {
    response: Response,
    remaining: Option<u64>,
    cancel: CancellationToken,
    path: ResourcePath,
}
impl ContentReader for Reader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        paging::check(&self.cancel, Op::Read, &self.path)?;
        let n = self
            .response
            .read(buf)
            .map_err(|_| error(Kind::Disconnected, Op::Read, &self.path))?;
        paging::check(&self.cancel, Op::Read, &self.path)?;
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
