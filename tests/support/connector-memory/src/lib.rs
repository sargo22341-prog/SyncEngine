//! Double de test : stockage en mémoire avec injection de pannes.
//!
//! [`MemoryConnector`] implémente [`StorageConnector`] sur une arborescence
//! construite en mémoire. Il sert à simuler, de façon déterministe et sur
//! toutes les plateformes, ce qu'un vrai stockage rend difficile à
//! reproduire : accès refusé, déconnexion en cours de listing, de lecture
//! ou d'écriture, disque plein, corruption silencieuse, modification
//! concurrente, pagination, noms non représentables, réponses incohérentes,
//! annulation.
//!
//! Les mutations (phase 04) ne sont déclarées qu'après [`MemoryConnector::writable`] ;
//! l'écriture suit le contrat [`ContentWriter`] : dépôt temporaire visible
//! dans l'arborescence (`.syncengine-mem-N.partial`), puis validation.
//!
//! **Réservé aux tests** (dev-dependency) : ce n'est pas un connecteur
//! utilisable et il n'est enregistré nulle part.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use connector_api::{
    CaseSensitivity, ClockSource, CommitMode, ContentReader, ContentWriter, ListPage,
    ListingConsistency, ListingIssue, PageToken, Pagination, StorageCapabilities, StorageConnector,
    StorageError, StorageErrorKind, StorageFeature, StorageOperation, SymlinkSupport, WriteOptions,
    ensure_supported,
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

/// Panne injectée en cours de lecture ou d'écriture.
#[derive(Debug, Clone, Copy)]
struct StreamFault {
    after_bytes: usize,
    kind: StorageErrorKind,
}

/// Déclenchement d'une annulation au n-ième appel de `list`.
#[derive(Debug, Clone)]
struct CancelTrigger {
    at_call: usize,
    token: CancellationToken,
}

/// État mutable partagé avec les flux d'écriture.
#[derive(Debug, Default)]
struct State {
    nodes: BTreeMap<ResourcePath, Node>,
    /// Espace total disponible pour le contenu des fichiers, si limité.
    space_limit: Option<usize>,
    staging_counter: usize,
}

impl State {
    fn used_bytes(&self) -> usize {
        self.nodes.values().map(|node| node.content.len()).sum()
    }
}

/// Stockage en mémoire configurable (voir la documentation du crate).
#[derive(Debug)]
pub struct MemoryConnector {
    id: ConnectorId,
    capabilities: StorageCapabilities,
    precision: TimestampPrecision,
    state: Arc<Mutex<State>>,
    failures: HashMap<(ResourcePath, StorageOperation), StorageErrorKind>,
    read_faults: BTreeMap<ResourcePath, StreamFault>,
    write_faults: BTreeMap<ResourcePath, StreamFault>,
    corrupted_writes: Vec<ResourcePath>,
    touch_on_read: BTreeMap<ResourcePath, Timestamp>,
    issues: BTreeMap<ResourcePath, Vec<ListingIssue>>,
    extra_entries: BTreeMap<ResourcePath, Vec<FileEntry>>,
    page_size: Option<usize>,
    cancel_trigger: Option<CancelTrigger>,
    disconnected: AtomicBool,
    list_calls: AtomicUsize,
    read_opens: AtomicUsize,
    mutations: Arc<AtomicUsize>,
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

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // Double de test : un verrou empoisonné reste exploitable.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MemoryConnector {
    /// Stockage vide, sensible à la casse, dates à la nanoseconde, non
    /// paginé, **en lecture seule** (voir [`Self::writable`]).
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
            state: Arc::new(Mutex::new(State::default())),
            failures: HashMap::new(),
            read_faults: BTreeMap::new(),
            write_faults: BTreeMap::new(),
            corrupted_writes: Vec::new(),
            touch_on_read: BTreeMap::new(),
            issues: BTreeMap::new(),
            extra_entries: BTreeMap::new(),
            page_size: None,
            cancel_trigger: None,
            disconnected: AtomicBool::new(false),
            list_calls: AtomicUsize::new(0),
            read_opens: AtomicUsize::new(0),
            mutations: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Déclare les mutations (création de répertoire, écriture, suppression).
    #[must_use]
    pub fn writable(mut self) -> Self {
        self.capabilities = self.capabilities.with_features([
            StorageFeature::Write,
            StorageFeature::SetModifiedTime,
            StorageFeature::StreamingWrite,
            StorageFeature::CreateDirectory,
            StorageFeature::Delete,
        ]);
        self
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

    /// Limite l'espace total du contenu des fichiers (temporaires compris) :
    /// une écriture qui le dépasse échoue avec `InsufficientSpace`.
    #[must_use]
    pub fn with_space_limit(self, bytes: usize) -> Self {
        lock(&self.state).space_limit = Some(bytes);
        self
    }

    fn insert(&mut self, at: &str, kind: EntryKind, metadata: FileMetadata, content: &[u8]) {
        let at = path(at);
        let mut state = lock(&self.state);
        insert_parents(&mut state.nodes, &at);
        state.nodes.insert(
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
        let metadata = self.file_metadata(content.len(), modified);
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

    /// Fait échouer `operation` sur `at` avec `kind`. Pour l'écriture,
    /// l'échec survient à l'ouverture (`open_write`).
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
            .insert(path(at), StreamFault { after_bytes, kind });
        self
    }

    /// Fait échouer l'écriture **vers la cible** `at` après `after_bytes`
    /// octets déposés.
    #[must_use]
    pub fn fail_write_after(
        mut self,
        at: &str,
        after_bytes: usize,
        kind: StorageErrorKind,
    ) -> Self {
        self.write_faults
            .insert(path(at), StreamFault { after_bytes, kind });
        self
    }

    /// Corrompt silencieusement le contenu déposé pour la cible `at` (le
    /// premier octet est inversé) : seule une relecture le détecte.
    #[must_use]
    pub fn corrupt_writes_to(mut self, at: &str) -> Self {
        self.corrupted_writes.push(path(at));
        self
    }

    /// À l'ouverture en lecture de `at`, sa date de modification devient
    /// `modified` (écriture concurrente pendant un transfert).
    #[must_use]
    pub fn touch_on_read(mut self, at: &str, modified: Timestamp) -> Self {
        self.touch_on_read.insert(path(at), modified);
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

    /// Nombre de mutations **réussies** (répertoire créé, fichier validé,
    /// entrée supprimée).
    #[must_use]
    pub fn mutations(&self) -> usize {
        self.mutations.load(Ordering::SeqCst)
    }

    // --- Modifications pendant un test (après construction) ---------------

    /// Ajoute ou remplace un fichier (modification par un tiers).
    pub fn put_file(&self, at: &str, content: &[u8], modified: Timestamp) {
        let at = path(at);
        let metadata = self.file_metadata(content.len(), modified);
        let mut state = lock(&self.state);
        insert_parents(&mut state.nodes, &at);
        state.nodes.insert(
            at,
            Node {
                kind: EntryKind::File,
                metadata,
                content: content.to_vec(),
            },
        );
    }

    /// Retire une entrée et tout ce qu'elle contient (action d'un tiers).
    pub fn remove(&self, at: &str) {
        let at = path(at);
        lock(&self.state)
            .nodes
            .retain(|key, _| !key.starts_with(&at));
    }

    /// Simule un support débranché : toute opération échoue avec
    /// `Disconnected` tant que l'état n'est pas rétabli.
    pub fn set_disconnected(&self, disconnected: bool) {
        self.disconnected.store(disconnected, Ordering::SeqCst);
    }

    // --- Inspection ---------------------------------------------------------

    /// Contenu d'un fichier, `None` s'il n'existe pas.
    #[must_use]
    pub fn content(&self, at: &str) -> Option<Vec<u8>> {
        lock(&self.state)
            .nodes
            .get(&path(at))
            .filter(|node| node.kind == EntryKind::File)
            .map(|node| node.content.clone())
    }

    /// Entrée observée à `at`, `None` si elle n'existe pas.
    #[must_use]
    pub fn entry_at(&self, at: &str) -> Option<FileEntry> {
        let at = path(at);
        lock(&self.state)
            .nodes
            .get(&at)
            .map(|node| FileEntry::new(at.clone(), node.kind, node.metadata.clone()))
    }

    /// Tous les chemins présents, triés.
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        lock(&self.state)
            .nodes
            .keys()
            .map(|at| at.as_str().to_owned())
            .collect()
    }

    fn file_metadata(&self, len: usize, modified: Timestamp) -> FileMetadata {
        FileMetadata::unknown()
            .with_size(len as u64)
            .with_modified(modified, self.precision)
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
        if self.disconnected.load(Ordering::SeqCst) {
            return Err(self.error(StorageErrorKind::Disconnected, operation, at));
        }
        match self.failures.get(&(at.clone(), operation)) {
            Some(kind) => Err(self.error(*kind, operation, at)),
            None => Ok(()),
        }
    }

    fn before_mutation(
        &self,
        feature: StorageFeature,
        operation: StorageOperation,
        at: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        ensure_supported(self, feature, operation)?;
        if cancel.is_cancelled() {
            return Err(self.error(StorageErrorKind::Cancelled, operation, at));
        }
        self.injected(at, operation)?;
        if at.is_root() {
            return Err(self.error(StorageErrorKind::Permanent, operation, at));
        }
        Ok(())
    }

    /// Vérifie que le parent de `at` existe et est un répertoire.
    fn parent_exists(
        &self,
        nodes: &BTreeMap<ResourcePath, Node>,
        at: &ResourcePath,
        operation: StorageOperation,
    ) -> Result<(), StorageError> {
        let parent = at.parent().unwrap_or_default();
        if parent.is_root() {
            return Ok(());
        }
        match nodes.get(&parent) {
            Some(node) if node.kind == EntryKind::Directory => Ok(()),
            _ => Err(self.error(StorageErrorKind::NotFound, operation, at)),
        }
    }
}

fn insert_parents(nodes: &mut BTreeMap<ResourcePath, Node>, of: &ResourcePath) {
    let mut parent = of.parent();
    while let Some(dir) = parent {
        if dir.is_root() {
            break;
        }
        parent = dir.parent();
        nodes.entry(dir).or_insert_with(|| Node {
            kind: EntryKind::Directory,
            metadata: FileMetadata::unknown(),
            content: Vec::new(),
        });
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
        let state = lock(&self.state);
        if !directory.is_root() {
            match state.nodes.get(directory) {
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
        let mut children: Vec<FileEntry> = state
            .nodes
            .iter()
            .filter(|(at, _)| at.parent().as_ref() == Some(directory))
            .map(|(at, node)| FileEntry::new(at.clone(), node.kind, node.metadata.clone()))
            .collect();
        drop(state);
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
        lock(&self.state)
            .nodes
            .get(at)
            .map(|node| FileEntry::new(at.clone(), node.kind, node.metadata.clone()))
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
        let mut state = lock(&self.state);
        let node = state
            .nodes
            .get_mut(at)
            .ok_or_else(|| self.error(StorageErrorKind::NotFound, StorageOperation::Read, at))?;
        if node.kind != EntryKind::File {
            return Err(self.error(StorageErrorKind::Permanent, StorageOperation::Read, at));
        }
        let content = node.content.clone();
        if let Some(modified) = self.touch_on_read.get(at) {
            node.metadata = node
                .metadata
                .clone()
                .with_modified(*modified, self.precision);
        }
        drop(state);
        self.read_opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(MemoryReader {
            content,
            position: 0,
            fault: self.read_faults.get(at).copied(),
            at: at.clone(),
            connector: self.id.clone(),
            cancel: cancel.clone(),
        }))
    }

    fn create_directory(
        &self,
        at: &ResourcePath,
        cancel: &CancellationToken,
    ) -> Result<FileEntry, StorageError> {
        let operation = StorageOperation::CreateDirectory;
        self.before_mutation(StorageFeature::CreateDirectory, operation, at, cancel)?;
        let mut state = lock(&self.state);
        self.parent_exists(&state.nodes, at, operation)?;
        if state.nodes.contains_key(at) {
            return Err(self.error(StorageErrorKind::Conflict, operation, at));
        }
        let node = Node {
            kind: EntryKind::Directory,
            metadata: FileMetadata::unknown(),
            content: Vec::new(),
        };
        state.nodes.insert(at.clone(), node);
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(FileEntry::new(
            at.clone(),
            EntryKind::Directory,
            FileMetadata::unknown(),
        ))
    }

    fn open_write(
        &self,
        at: &ResourcePath,
        options: &WriteOptions,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn ContentWriter>, StorageError> {
        let operation = StorageOperation::Write;
        self.before_mutation(StorageFeature::Write, operation, at, cancel)?;
        let mut state = lock(&self.state);
        self.parent_exists(&state.nodes, at, operation)?;
        state.staging_counter += 1;
        let name = format!(".syncengine-mem-{}.partial", state.staging_counter);
        let staging = at
            .parent()
            .unwrap_or_default()
            .join(&name)
            .map_err(|_| self.error(StorageErrorKind::Permanent, operation, at))?;
        state.nodes.insert(
            staging.clone(),
            Node {
                kind: EntryKind::File,
                metadata: FileMetadata::unknown().with_size(0),
                content: Vec::new(),
            },
        );
        drop(state);
        Ok(Box::new(MemoryWriter {
            state: Arc::clone(&self.state),
            staging,
            target: at.clone(),
            connector: self.id.clone(),
            precision: self.precision,
            modified: options.modified,
            written: 0,
            fault: self.write_faults.get(at).copied(),
            corrupt: self.corrupted_writes.contains(at),
            cancel: cancel.clone(),
            finished: false,
            mutations: Arc::clone(&self.mutations),
        }))
    }

    fn delete(
        &self,
        at: &ResourcePath,
        expected: EntryKind,
        cancel: &CancellationToken,
    ) -> Result<(), StorageError> {
        let operation = StorageOperation::Delete;
        self.before_mutation(StorageFeature::Delete, operation, at, cancel)?;
        let mut state = lock(&self.state);
        let node = state
            .nodes
            .get(at)
            .ok_or_else(|| self.error(StorageErrorKind::NotFound, operation, at))?;
        match (expected, node.kind) {
            (_, EntryKind::Symlink | EntryKind::Other) => {
                return Err(self.error(StorageErrorKind::NotSupported, operation, at));
            }
            (EntryKind::File, EntryKind::File) => {}
            (EntryKind::Directory, EntryKind::Directory) => {
                let has_children = state
                    .nodes
                    .keys()
                    .any(|key| key.parent().as_ref() == Some(at));
                if has_children {
                    return Err(self.error(StorageErrorKind::Conflict, operation, at));
                }
            }
            _ => return Err(self.error(StorageErrorKind::Conflict, operation, at)),
        }
        state.nodes.remove(at);
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Flux de lecture en mémoire, par blocs d'au plus `buf.len()` octets.
struct MemoryReader {
    content: Vec<u8>,
    position: usize,
    fault: Option<StreamFault>,
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

/// Écriture en deux temps en mémoire : le dépôt temporaire est un nœud de
/// l'arborescence, publié (ou retiré) à la fin.
struct MemoryWriter {
    state: Arc<Mutex<State>>,
    staging: ResourcePath,
    target: ResourcePath,
    connector: ConnectorId,
    precision: TimestampPrecision,
    modified: Option<Timestamp>,
    written: usize,
    fault: Option<StreamFault>,
    corrupt: bool,
    cancel: CancellationToken,
    finished: bool,
    mutations: Arc<AtomicUsize>,
}

impl MemoryWriter {
    fn error(&self, kind: StorageErrorKind, at: &ResourcePath) -> StorageError {
        StorageError::new(kind, StorageOperation::Write)
            .with_connector(self.connector.clone())
            .with_path(at.clone())
    }

    fn discard(&mut self) {
        self.finished = true;
        lock(&self.state).nodes.remove(&self.staging);
    }
}

impl ContentWriter for MemoryWriter {
    fn write(&mut self, data: &[u8]) -> Result<(), StorageError> {
        if self.cancel.is_cancelled() {
            return Err(self.error(StorageErrorKind::Cancelled, &self.target));
        }
        if let Some(fault) = self.fault
            && self.written.saturating_add(data.len()) > fault.after_bytes
        {
            return Err(self.error(fault.kind, &self.target));
        }
        let mut state = lock(&self.state);
        if let Some(limit) = state.space_limit
            && state.used_bytes().saturating_add(data.len()) > limit
        {
            return Err(self.error(StorageErrorKind::InsufficientSpace, &self.target));
        }
        let node = state
            .nodes
            .get_mut(&self.staging)
            .ok_or_else(|| self.error(StorageErrorKind::NotFound, &self.staging))?;
        let start = node.content.len();
        node.content.extend_from_slice(data);
        if self.corrupt
            && start == 0
            && let Some(first) = node.content.first_mut()
        {
            *first = !*first;
        }
        node.metadata = FileMetadata::unknown().with_size(node.content.len() as u64);
        self.written += data.len();
        Ok(())
    }

    fn staging_path(&self) -> Option<&ResourcePath> {
        Some(&self.staging)
    }

    fn commit(mut self: Box<Self>, mode: CommitMode) -> Result<FileEntry, StorageError> {
        if self.cancel.is_cancelled() {
            self.discard();
            return Err(self.error(StorageErrorKind::Cancelled, &self.target));
        }
        let mut state = lock(&self.state);
        let target_kind = state.nodes.get(&self.target).map(|node| node.kind);
        let refused = !matches!(
            (mode, target_kind),
            (CommitMode::CreateNew, None) | (CommitMode::ReplaceExisting, Some(EntryKind::File))
        );
        if refused {
            state.nodes.remove(&self.staging);
            drop(state);
            self.finished = true;
            return Err(self.error(StorageErrorKind::Conflict, &self.target));
        }
        let Some(mut node) = state.nodes.remove(&self.staging) else {
            drop(state);
            self.finished = true;
            return Err(self.error(StorageErrorKind::NotFound, &self.staging));
        };
        let modified = self
            .modified
            .unwrap_or_else(|| Timestamp::now().unwrap_or(Timestamp::UNIX_EPOCH));
        node.metadata = FileMetadata::unknown()
            .with_size(node.content.len() as u64)
            .with_modified(modified, self.precision);
        let entry = FileEntry::new(self.target.clone(), EntryKind::File, node.metadata.clone());
        state.nodes.insert(self.target.clone(), node);
        drop(state);
        self.finished = true;
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(entry)
    }

    fn abort(mut self: Box<Self>) -> Result<(), StorageError> {
        self.discard();
        Ok(())
    }
}

impl Drop for MemoryWriter {
    fn drop(&mut self) {
        if !self.finished {
            self.discard();
        }
    }
}
