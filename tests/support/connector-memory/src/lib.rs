//! Double de test : stockage en mémoire avec injection de pannes.
//!
//! [`MemoryConnector`] implémente [`StorageConnector`] sur une arborescence
//! construite en mémoire. Il sert à simuler, de façon déterministe et sur
//! toutes les plateformes, ce qu'un vrai stockage rend difficile à
//! reproduire : accès refusé, déconnexion en cours de listing ou de lecture,
//! pagination, noms non représentables, réponses incohérentes, annulation.
//!
//! **Réservé aux tests** (dev-dependency) : ce n'est pas un connecteur
//! utilisable et il n'est enregistré nulle part.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};

use connector_api::{
    CaseSensitivity, ClockSource, ContentReader, ListPage, ListingConsistency, ListingIssue,
    PageToken, Pagination, StorageCapabilities, StorageConnector, StorageError, StorageErrorKind,
    StorageFeature, StorageOperation, SymlinkSupport, ensure_supported,
};
use sync_core::{
    CancellationToken, ConnectorId, EntryKind, FileEntry, FileMetadata, ResourcePath, Timestamp,
    TimestampPrecision,
};

#[derive(Debug, Clone)]
struct Node {
    kind: EntryKind,
    metadata: FileMetadata,
    content: Vec<u8>,
}

/// Panne injectée en cours de lecture.
#[derive(Debug, Clone, Copy)]
struct ReadFault {
    after_bytes: usize,
    kind: StorageErrorKind,
}

/// Déclenchement d'une annulation au n-ième appel de `list`.
#[derive(Debug, Clone)]
struct CancelTrigger {
    at_call: usize,
    token: CancellationToken,
}

/// Stockage en mémoire configurable (voir la documentation du crate).
#[derive(Debug)]
pub struct MemoryConnector {
    id: ConnectorId,
    capabilities: StorageCapabilities,
    precision: TimestampPrecision,
    nodes: BTreeMap<ResourcePath, Node>,
    failures: HashMap<(ResourcePath, StorageOperation), StorageErrorKind>,
    read_faults: BTreeMap<ResourcePath, ReadFault>,
    issues: BTreeMap<ResourcePath, Vec<ListingIssue>>,
    extra_entries: BTreeMap<ResourcePath, Vec<FileEntry>>,
    page_size: Option<usize>,
    cancel_trigger: Option<CancelTrigger>,
    list_calls: AtomicUsize,
    read_opens: AtomicUsize,
}

/// Horodatage de test : `secs` secondes après l'époque.
#[must_use]
pub fn ts(secs: i64) -> Timestamp {
    Timestamp::new(secs, 0).unwrap_or(Timestamp::UNIX_EPOCH)
}

/// Chemin logique de test ; un chemin invalide devient la racine, ce qui
/// fait échouer le test appelant de façon visible.
#[must_use]
pub fn path(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap_or_default()
}

impl MemoryConnector {
    /// Stockage vide, sensible à la casse, dates à la nanoseconde, non paginé.
    #[must_use]
    pub fn new(id: &str) -> Self {
        let mut capabilities = StorageCapabilities::none().with_features([
            StorageFeature::List,
            StorageFeature::Stat,
            StorageFeature::Read,
            StorageFeature::StreamingRead,
            StorageFeature::Cancellation,
        ]);
        capabilities.case_sensitivity = CaseSensitivity::Sensitive;
        capabilities.timestamp_precision = TimestampPrecision::Nanosecond;
        capabilities.clock = ClockSource::LocalSystem;
        capabilities.listing_consistency = ListingConsistency::Strong;
        capabilities.symlinks = SymlinkSupport::Reported;
        Self {
            id: ConnectorId::new(id).unwrap_or_else(|_| fallback_id()),
            capabilities,
            precision: TimestampPrecision::Nanosecond,
            nodes: BTreeMap::new(),
            failures: HashMap::new(),
            read_faults: BTreeMap::new(),
            issues: BTreeMap::new(),
            extra_entries: BTreeMap::new(),
            page_size: None,
            cancel_trigger: None,
            list_calls: AtomicUsize::new(0),
            read_opens: AtomicUsize::new(0),
        }
    }

    /// Remplace les capacités déclarées (les opérations non déclarées
    /// échouent alors avec `NotSupported`).
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: StorageCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Règle de casse déclarée.
    #[must_use]
    pub fn with_case_sensitivity(mut self, case: CaseSensitivity) -> Self {
        self.capabilities.case_sensitivity = case;
        self
    }

    /// Précision des dates déclarée et appliquée aux fichiers ajoutés ensuite.
    #[must_use]
    pub fn with_precision(mut self, precision: TimestampPrecision) -> Self {
        self.capabilities.timestamp_precision = precision;
        self.precision = precision;
        self
    }

    /// Active la pagination : au plus `size` entrées par page.
    #[must_use]
    pub fn with_page_size(mut self, size: usize) -> Self {
        self.page_size = Some(size.max(1));
        self.capabilities.pagination = Pagination::Paginated {
            max_page_size: u32::try_from(size).ok(),
        };
        self
    }

    fn insert_parents(&mut self, of: &ResourcePath) {
        let mut parent = of.parent();
        while let Some(dir) = parent {
            if dir.is_root() {
                break;
            }
            parent = dir.parent();
            self.nodes.entry(dir).or_insert_with(|| Node {
                kind: EntryKind::Directory,
                metadata: FileMetadata::unknown(),
                content: Vec::new(),
            });
        }
    }

    fn insert(&mut self, at: &str, kind: EntryKind, metadata: FileMetadata, content: &[u8]) {
        let at = path(at);
        self.insert_parents(&at);
        self.nodes.insert(
            at,
            Node {
                kind,
                metadata,
                content: content.to_vec(),
            },
        );
    }

    /// Ajoute un répertoire (et ses parents).
    #[must_use]
    pub fn dir(mut self, at: &str) -> Self {
        self.insert(at, EntryKind::Directory, FileMetadata::unknown(), &[]);
        self
    }

    /// Ajoute un fichier de contenu `content`, modifié à `modified`.
    #[must_use]
    pub fn file(mut self, at: &str, content: &[u8], modified: Timestamp) -> Self {
        let metadata = FileMetadata::unknown()
            .with_size(content.len() as u64)
            .with_modified(modified, self.precision);
        self.insert(at, EntryKind::File, metadata, content);
        self
    }

    /// Ajoute un fichier dont les métadonnées rapportées sont imposées
    /// (éventuellement incohérentes avec le contenu).
    #[must_use]
    pub fn file_with_metadata(mut self, at: &str, content: &[u8], metadata: FileMetadata) -> Self {
        self.insert(at, EntryKind::File, metadata, content);
        self
    }

    /// Ajoute un lien symbolique (rapporté, jamais suivi).
    #[must_use]
    pub fn symlink(mut self, at: &str) -> Self {
        self.insert(at, EntryKind::Symlink, FileMetadata::unknown(), &[]);
        self
    }

    /// Fait échouer `operation` sur `at` avec `kind`.
    #[must_use]
    pub fn fail(mut self, at: &str, operation: StorageOperation, kind: StorageErrorKind) -> Self {
        self.failures.insert((path(at), operation), kind);
        self
    }

    /// Fait échouer la lecture de `at` après `after_bytes` octets
    /// (déconnexion en cours de transfert, lecture partielle).
    #[must_use]
    pub fn fail_read_after(mut self, at: &str, after_bytes: usize, kind: StorageErrorKind) -> Self {
        self.read_faults
            .insert(path(at), ReadFault { after_bytes, kind });
        self
    }

    /// Ajoute une anomalie au listing du répertoire `dir`.
    #[must_use]
    pub fn listing_issue(mut self, dir: &str, issue: ListingIssue) -> Self {
        self.issues.entry(path(dir)).or_default().push(issue);
        self
    }

    /// Ajoute au listing de `dir` une entrée arbitraire (réponse incohérente
    /// d'un stockage défaillant).
    #[must_use]
    pub fn extra_entry(mut self, dir: &str, entry: FileEntry) -> Self {
        self.extra_entries.entry(path(dir)).or_default().push(entry);
        self
    }

    /// Annule `token` lors du `at_call`-ième appel de `list` (à partir de 1).
    #[must_use]
    pub fn cancel_on_list_call(mut self, at_call: usize, token: CancellationToken) -> Self {
        self.cancel_trigger = Some(CancelTrigger { at_call, token });
        self
    }

    /// Nombre d'appels de `list` reçus.
    #[must_use]
    pub fn list_calls(&self) -> usize {
        self.list_calls.load(Ordering::SeqCst)
    }

    /// Nombre de flux de lecture ouverts.
    #[must_use]
    pub fn read_opens(&self) -> usize {
        self.read_opens.load(Ordering::SeqCst)
    }

    fn error(
        &self,
        kind: StorageErrorKind,
        operation: StorageOperation,
        at: &ResourcePath,
    ) -> StorageError {
        StorageError::new(kind, operation)
            .with_connector(self.id.clone())
            .with_path(at.clone())
    }

    fn injected(&self, at: &ResourcePath, operation: StorageOperation) -> Result<(), StorageError> {
        match self.failures.get(&(at.clone(), operation)) {
            Some(kind) => Err(self.error(*kind, operation, at)),
            None => Ok(()),
        }
    }

    fn entry(&self, at: &ResourcePath, node: &Node) -> FileEntry {
        FileEntry::new(at.clone(), node.kind, node.metadata.clone())
    }
}

fn fallback_id() -> ConnectorId {
    ConnectorId::new("memory").unwrap_or_else(|_| unreachable!())
}

impl StorageConnector for MemoryConnector {
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
        let call = self.list_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(trigger) = &self.cancel_trigger
            && trigger.at_call == call
        {
            trigger.token.cancel();
        }
        ensure_supported(self, StorageFeature::List, StorageOperation::List)?;
        if cancel.is_cancelled() {
            return Err(self.error(
                StorageErrorKind::Cancelled,
                StorageOperation::List,
                directory,
            ));
        }
        self.injected(directory, StorageOperation::List)?;
        if !directory.is_root() {
            match self.nodes.get(directory) {
                Some(node) if node.kind == EntryKind::Directory => {}
                Some(_) => {
                    return Err(self.error(
                        StorageErrorKind::Permanent,
                        StorageOperation::List,
                        directory,
                    ));
                }
                None => {
                    return Err(self.error(
                        StorageErrorKind::NotFound,
                        StorageOperation::List,
                        directory,
                    ));
                }
            }
        }
        let mut children: Vec<FileEntry> = self
            .nodes
            .iter()
            .filter(|(at, _)| at.parent().as_ref() == Some(directory))
            .map(|(at, node)| self.entry(at, node))
            .collect();
        children.extend(
            self.extra_entries
                .get(directory)
                .cloned()
                .unwrap_or_default(),
        );
        let issues = self.issues.get(directory).cloned().unwrap_or_default();

        let Some(size) = self.page_size else {
            return Ok(ListPage::new(children, issues, None));
        };
        let offset = match page {
            None => 0,
            Some(token) => token.as_str().parse::<usize>().map_err(|_| {
                self.error(
                    StorageErrorKind::Permanent,
                    StorageOperation::List,
                    directory,
                )
            })?,
        };
        let end = offset.saturating_add(size).min(children.len());
        let slice = children
            .get(offset..end)
            .map(<[FileEntry]>::to_vec)
            .unwrap_or_default();
        let next = (end < children.len()).then(|| PageToken::new(end.to_string()));
        // Les anomalies sont rapportées avec la première page.
        let issues = if offset == 0 { issues } else { Vec::new() };
        Ok(ListPage::new(slice, issues, next))
    }

    fn stat(
        &self,
        at: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        ensure_supported(self, StorageFeature::Stat, StorageOperation::Stat)?;
        if cancel.is_cancelled() {
            return Err(self.error(StorageErrorKind::Cancelled, StorageOperation::Stat, at));
        }
        self.injected(at, StorageOperation::Stat)?;
        if at.is_root() {
            return Ok(FileEntry::new(
                ResourcePath::root(),
                EntryKind::Directory,
                FileMetadata::unknown(),
            ));
        }
        self.nodes
            .get(at)
            .map(|node| self.entry(at, node))
            .ok_or_else(|| self.error(StorageErrorKind::NotFound, StorageOperation::Stat, at))
    }

    fn open_read(
        &self,
        at: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentReader>, StorageError> {
        ensure_supported(self, StorageFeature::Read, StorageOperation::Read)?;
        if cancel.is_cancelled() {
            return Err(self.error(StorageErrorKind::Cancelled, StorageOperation::Read, at));
        }
        self.injected(at, StorageOperation::Read)?;
        let node = self
            .nodes
            .get(at)
            .ok_or_else(|| self.error(StorageErrorKind::NotFound, StorageOperation::Read, at))?;
        if node.kind != EntryKind::File {
            return Err(self.error(StorageErrorKind::Permanent, StorageOperation::Read, at));
        }
        self.read_opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MemoryReader {
            content: node.content.clone(),
            position: 0,
            fault: self.read_faults.get(at).copied(),
            at: at.clone(),
            connector: self.id.clone(),
            cancel: cancel.clone(),
        }))
    }
}

/// Flux de lecture en mémoire, par blocs d'au plus `buf.len()` octets.
struct MemoryReader {
    content: Vec<u8>,
    position: usize,
    fault: Option<ReadFault>,
    at: ResourcePath,
    connector: ConnectorId,
    cancel: CancellationToken,
}

impl ContentReader for MemoryReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StorageError> {
        let error = |kind| {
            StorageError::new(kind, StorageOperation::Read)
                .with_connector(self.connector.clone())
                .with_path(self.at.clone())
        };
        if self.cancel.is_cancelled() {
            return Err(error(StorageErrorKind::Cancelled));
        }
        let mut limit = self.content.len();
        if let Some(fault) = self.fault {
            if self.position >= fault.after_bytes {
                return Err(error(fault.kind));
            }
            limit = limit.min(fault.after_bytes);
        }
        let end = limit.min(self.position.saturating_add(buf.len()));
        let chunk = self.content.get(self.position..end).unwrap_or_default();
        let count = chunk.len();
        if let Some(target) = buf.get_mut(..count) {
            target.copy_from_slice(chunk);
        }
        self.position = end;
        Ok(count)
    }
}
