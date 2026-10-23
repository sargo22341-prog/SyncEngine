use crate::{
    model::{Item, Page},
    transport::{Http, check, client, error},
};
use connector_api::{
    ContentReader, ListPage, PageToken, Pagination, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind as Kind, StorageFeature, StorageOperation as Op,
};
use reqwest::{Url, blocking::Response, header::LOCATION};
use std::{
    collections::BTreeSet,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use sync_core::{CancellationToken, ConnectorId, EntryKind, FileEntry, ResourcePath};
use sync_credentials::AccessTokenProvider;

static GENERATION: AtomicU64 = AtomicU64::new(1);
const MAX_ENTRIES: usize = 100_000;
const MAX_PAGES: usize = 1_000;

/// Configuration non secrète. Microsoft Graph global v1.0 uniquement.
pub struct OneDriveOptions {
    /// Identifiant explicite du drive, pas `/me` ni une URL.
    pub drive_id: String,
    /// Hôtes HTTPS exacts autorisés pour les URL préauthentifiées renvoyées
    /// par Graph. Vide = téléchargement refusé. Pas de wildcard ni IP.
    /// L'administrateur valide ces hôtes Microsoft pour son tenant.
    pub download_hosts: Vec<String>,
}

/// Source OneDrive expérimentale. Aucun OAuth interactif, écriture ou delta.
/// Une énumération active par instance ; la pagination n'est pas un snapshot.
pub struct OneDriveConnector {
    http: Http,
    drive: String,
    id: ConnectorId,
    caps: StorageCapabilities,
    pages: Mutex<Option<Cursor>>,
}
struct Cursor {
    directory: ResourcePath,
    parent: String,
    next: Url,
    token: String,
    names: BTreeSet<String>,
    ids: BTreeSet<String>,
    urls: BTreeSet<String>,
    pages: usize,
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 1024
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!_-".contains(&b))
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && !name
            .chars()
            .any(|c| c.is_control() || "\\/*<>?:|".contains(c))
}
impl OneDriveConnector {
    /// Construit le transport sans requête réseau. Le provider est lié au compte
    /// et à l'audience Graph par l'appelant ; son travail doit être borné.
    /// Appeler [`Self::connect`] pour vérifier effectivement l'accès.
    /// # Errors
    /// `Permanent` pour configuration invalide ou client TLS indisponible.
    pub fn new(
        options: OneDriveOptions,
        tokens: Arc<dyn AccessTokenProvider>,
    ) -> Result<Self, StorageError> {
        let root = ResourcePath::root();
        if !valid_id(&options.drive_id)
            || options.download_hosts.iter().any(|host| {
                !Url::parse(&format!("https://{host}/"))
                    .ok()
                    .is_some_and(|u| {
                        u.host_str() == Some(host.as_str())
                            && host.parse::<std::net::IpAddr>().is_err()
                    })
                    || host.contains(['/', ':', '@', '?', '#', '*'])
                    || host != &host.to_ascii_lowercase()
            })
        {
            return Err(error(Kind::Permanent, Op::Connect, &root));
        }
        let mut caps = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Cancellation,
        ]);
        if !options.download_hosts.is_empty() {
            caps.features
                .extend([StorageFeature::Read, StorageFeature::StreamingRead]);
        }
        caps.pagination = Pagination::Paginated {
            max_page_size: Some(1000),
        };
        caps.rate_limited = true;
        Ok(Self {
            http: Http {
                client: client()?,
                base: Url::parse("https://graph.microsoft.com/v1.0/")
                    .map_err(|_| error(Kind::Permanent, Op::Connect, &root))?,
                tokens,
                download_hosts: options.download_hosts,
            },
            drive: options.drive_id,
            id: ConnectorId::new("onedrive")
                .map_err(|_| error(Kind::Permanent, Op::Connect, &root))?,
            caps,
            pages: Mutex::new(None),
        })
    }
    /// Vérifie par GET que la racine du drive est un dossier accessible.
    /// # Errors
    /// Authentification, permission, réseau, quota, annulation ou réponse invalide.
    pub fn connect(&self, cancel: &CancellationToken) -> Result<(), StorageError> {
        self.resolve(&ResourcePath::root(), Op::Connect, cancel)
            .map(|_| ())
    }
    fn url(&self, parts: &[&str]) -> Result<Url, StorageError> {
        let mut url = self.http.base.clone();
        url.path_segments_mut()
            .map_err(|_| error(Kind::Permanent, Op::Connect, &ResourcePath::root()))?
            .pop_if_empty()
            .extend(["drives", &self.drive])
            .extend(parts.iter().copied());
        Ok(url)
    }
    fn resolve(
        &self,
        path: &ResourcePath,
        op: Op,
        cancel: &CancellationToken,
    ) -> Result<Item, StorageError> {
        if path.as_str().len() > 4096 || path.as_str().split('/').count() > 256 {
            return Err(error(Kind::NotSupported, op, path));
        }
        let mut item: Item = self.http.json(&self.url(&["root"])?, op, path, cancel)?;
        let root = item.entry(&ResourcePath::root(), op)?;
        if item
            .parent
            .as_ref()
            .and_then(|p| p.drive.as_deref())
            .is_some_and(|drive| drive != self.drive)
        {
            return Err(error(Kind::Conflict, op, path));
        }
        if root.kind() != EntryKind::Directory || !valid_id(&item.id) {
            return Err(error(Kind::Conflict, op, path));
        }
        let mut current = ResourcePath::root();
        for name in path.as_str().split('/').filter(|n| !n.is_empty()) {
            check(cancel, op, path)?;
            if !valid_name(name) {
                return Err(error(Kind::NotSupported, op, path));
            }
            if item.entry(&current, op)?.kind() != EntryKind::Directory {
                return Err(error(Kind::NotFound, op, path));
            }
            current = current
                .join(name)
                .map_err(|_| error(Kind::Permanent, op, path))?;
            let next: Item = self.http.json(
                &self.url(&["items", &format!("{}:", item.id), name])?,
                op,
                path,
                cancel,
            )?;
            if next.name != name || !valid_id(&next.id) {
                return Err(error(Kind::Conflict, op, path));
            }
            next.child(&item.id, &self.drive, &current, op)?;
            item = next;
        }
        Ok(item)
    }
    fn page(
        &self,
        cursor: &mut Cursor,
        cancel: &CancellationToken,
    ) -> Result<(Vec<FileEntry>, Option<Url>), StorageError> {
        let path = &cursor.directory;
        if cursor.pages >= MAX_PAGES || !cursor.urls.insert(cursor.next.to_string()) {
            return Err(error(Kind::Conflict, Op::List, path));
        }
        let page: Page = self.http.json(&cursor.next, Op::List, path, cancel)?;
        if page.value.len() > 1000 {
            return Err(error(Kind::Permanent, Op::List, path));
        }
        let mut entries = Vec::new();
        for item in page.value {
            if !valid_name(&item.name) {
                return Err(error(Kind::NotSupported, Op::List, path));
            }
            let child = path
                .join(&item.name)
                .map_err(|_| error(Kind::Permanent, Op::List, path))?;
            if !valid_id(&item.id)
                || !cursor.names.insert(item.name.to_lowercase())
                || !cursor.ids.insert(item.id.clone())
                || cursor.ids.len() > MAX_ENTRIES
            {
                return Err(error(Kind::Conflict, Op::List, path));
            }
            entries.push(item.child(&cursor.parent, &self.drive, &child, Op::List)?);
        }
        let next = page
            .next
            .map(|s| {
                if s.len() > 8192 {
                    return Err(error(Kind::Permanent, Op::List, path));
                }
                let url = Url::parse(&s).map_err(|_| error(Kind::Permanent, Op::List, path))?;
                if !self.http.graph_url(&url, cursor.next.path()) {
                    return Err(error(Kind::Permanent, Op::List, path));
                }
                Ok(url)
            })
            .transpose()?;
        cursor.pages += 1;
        check(cancel, Op::List, path)?;
        Ok((entries, next))
    }
}
impl StorageConnector for OneDriveConnector {
    fn connector_id(&self) -> &ConnectorId {
        &self.id
    }
    fn capabilities(&self) -> &StorageCapabilities {
        &self.caps
    }
    fn stat(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        self.resolve(path, Op::Stat, cancel)?.entry(path, Op::Stat)
    }
    fn list(
        &self,
        path: &ResourcePath,
        token: Option<&PageToken>,
        cancel: &CancellationToken,
    ) -> Result<ListPage, StorageError> {
        check(cancel, Op::List, path)?;
        let mut slot = self
            .pages
            .lock()
            .map_err(|_| error(Kind::Permanent, Op::List, path))?;
        let mut cursor = if let Some(token) = token {
            if !slot
                .as_ref()
                .is_some_and(|c| c.directory == *path && c.token == token.as_str())
            {
                return Err(error(Kind::Conflict, Op::List, path));
            }
            slot.take()
                .ok_or_else(|| error(Kind::Conflict, Op::List, path))?
        } else {
            *slot = None;
            let item = self.resolve(path, Op::List, cancel)?;
            if item.entry(path, Op::List)?.kind() != EntryKind::Directory {
                return Err(error(Kind::NotSupported, Op::List, path));
            }
            Cursor {
                directory: path.clone(),
                next: self.url(&["items", &item.id, "children"])?,
                parent: item.id,
                token: String::new(),
                names: BTreeSet::new(),
                ids: BTreeSet::new(),
                urls: BTreeSet::new(),
                pages: 0,
            }
        };
        let (entries, next) = self.page(&mut cursor, cancel)?;
        let token = if let Some(next) = next {
            cursor.next = next;
            cursor.token = format!("onedrive:{}", GENERATION.fetch_add(1, Ordering::Relaxed));
            let token = PageToken::new(cursor.token.clone());
            *slot = Some(cursor);
            Some(token)
        } else {
            None
        };
        Ok(ListPage::new(entries, vec![], token))
    }
    fn open_read(
        &self,
        path: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        check(cancel, Op::Read, path)?;
        if !self.caps.supports(StorageFeature::Read) {
            return Err(error(Kind::NotSupported, Op::Read, path));
        }
        let item = self.resolve(path, Op::Read, cancel)?;
        let entry = item.entry(path, Op::Read)?;
        if entry.kind() != EntryKind::File {
            return Err(error(Kind::NotSupported, Op::Read, path));
        }
        let etag = item
            .etag
            .filter(|e| !e.is_empty())
            .ok_or_else(|| error(Kind::NotSupported, Op::Read, path))?;
        let response = self.http.get(
            &self.url(&["items", &item.id, "content"])?,
            true,
            Op::Read,
            path,
            cancel,
        )?;
        if response.status().as_u16() != 302 {
            return Err(error(Kind::Permanent, Op::Read, path));
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| Url::parse(v).ok())
            .ok_or_else(|| error(Kind::Permanent, Op::Read, path))?;
        if !self.http.download_url(&location) {
            return Err(error(Kind::NotSupported, Op::Read, path));
        }
        // No Authorization, cookies or Referer sent to the preauthenticated URL.
        let response = self.http.get(&location, false, Op::Read, path, cancel)?;
        let size = entry
            .metadata()
            .size()
            .ok_or_else(|| error(Kind::Permanent, Op::Read, path))?;
        if response.content_length().is_some_and(|n| n != size) {
            return Err(error(Kind::Disconnected, Op::Read, path));
        }
        Ok(Box::new(Reader {
            response,
            http: self.http.clone(),
            drive: self.drive.clone(),
            path: path.clone(),
            id: item.id,
            etag,
            size,
            read: 0,
            done: false,
            failed: false,
            cancel: cancel.clone(),
        }))
    }
}
struct Reader {
    response: Response,
    http: Http,
    drive: String,
    path: ResourcePath,
    id: String,
    etag: String,
    size: u64,
    read: u64,
    done: bool,
    failed: bool,
    cancel: CancellationToken,
}
impl Reader {
    fn block(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        check(&self.cancel, Op::Read, &self.path)?;
        if self.failed {
            return Err(error(Kind::Disconnected, Op::Read, &self.path));
        }
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let n = self
            .response
            .read(buf)
            .map_err(|_| error(Kind::Disconnected, Op::Read, &self.path))?;
        check(&self.cancel, Op::Read, &self.path)?;
        self.read = self
            .read
            .checked_add(n as u64)
            .ok_or_else(|| error(Kind::Permanent, Op::Read, &self.path))?;
        if self.read > self.size || (n == 0 && self.read != self.size) {
            return Err(error(Kind::Disconnected, Op::Read, &self.path));
        }
        if n == 0 {
            let connector = OneDriveConnector {
                http: self.http.clone(),
                drive: self.drive.clone(),
                id: ConnectorId::new("onedrive")
                    .map_err(|_| error(Kind::Permanent, Op::Read, &self.path))?,
                caps: StorageCapabilities::none(),
                pages: Mutex::new(None),
            };
            let now = connector.resolve(&self.path, Op::Read, &self.cancel)?;
            if now.id != self.id
                || now.etag.as_deref() != Some(self.etag.as_str())
                || now.size != Some(self.size)
            {
                return Err(error(Kind::Conflict, Op::Read, &self.path));
            }
            self.done = true;
        }
        Ok(n)
    }
}
impl ContentReader for Reader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        let result = self.block(buf);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

#[cfg(test)]
mod tests;
