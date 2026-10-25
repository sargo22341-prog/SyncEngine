//! Parcours d'une extrémité.

use std::error::Error;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use connector_api::{StorageConnector, StorageError, StorageErrorKind, StorageFeature};
use sync_core::{
    CancellationToken, ContentHash, EntryKind, FileEntry, HashAlgorithm, ResourcePath,
};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext};
use sync_filter::{Filter, FilterDecision};

use crate::inventory::{DirectoryState, IncompleteReason, Inventory, ScanIssue, ScanIssueKind};
mod parallel;

/// Taille des blocs lus pour le calcul d'empreinte.
const HASH_CHUNK: usize = 64 * 1024;

/// Lectures d'empreinte simultanées recommandées pour un scan local
/// interactif : sur 9 220 petits fichiers jamais lus (Windows 11, SSD NVMe,
/// antivirus actif), 46 s avec 1 lecture, 15 s avec 4, 11 s avec 8. Quatre
/// garde un nombre de fichiers ouverts modeste et limite la dispersion des
/// lectures sur disque mécanique. Valeur par défaut de [`ScanOptions`] : 1.
pub const RECOMMENDED_HASH_WORKERS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(workers) => workers,
    None => NonZeroUsize::MIN,
};

/// Options d'un scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    hash: Option<HashAlgorithm>,
    entry_limit: usize,
    hash_workers: NonZeroUsize,
    listing_workers: NonZeroUsize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            hash: None,
            entry_limit: 2_000_000,
            hash_workers: NonZeroUsize::MIN,
            listing_workers: NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN),
        }
    }
}
impl ScanOptions {
    /// Maximum concurrent directory listings (default 4), further bounded by
    /// the connector's advertised concurrency and an absolute limit of 16.
    /// Connectors which do not opt in remain sequential. No writes are made.
    #[must_use]
    pub fn with_listing_workers(mut self, workers: NonZeroUsize) -> Self {
        self.listing_workers = workers;
        self
    }

    /// Requested maximum number of simultaneous directory listings.
    #[must_use]
    pub fn listing_workers(&self) -> NonZeroUsize {
        self.listing_workers
    }

    /// Bounds all listed entries and anomalies, including excluded/duplicate
    /// names. Exceeding this budget marks the scan incomplete (never absent).
    #[must_use]
    pub fn with_entry_limit(mut self, limit: std::num::NonZeroUsize) -> Self {
        self.entry_limit = limit.get();
        self
    }

    /// Scan des métadonnées seules.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Demande une empreinte de contenu pour chaque fichier inclus.
    ///
    /// Une empreinte fournie par le stockage dans le listing est réutilisée ;
    /// sinon seul [`HashAlgorithm::Blake3`] est calculé, par lecture complète
    /// en flux (coûteux : tout le contenu est relu).
    #[must_use]
    pub fn with_hash(mut self, algorithm: HashAlgorithm) -> Self {
        self.hash = Some(algorithm);
        self
    }

    /// Algorithme d'empreinte demandé.
    #[must_use]
    pub fn hash(&self) -> Option<HashAlgorithm> {
        self.hash
    }

    /// Nombre de fichiers lus simultanément pour les empreintes (défaut : 1).
    ///
    /// Au-delà de 1, autant de fichiers sont ouverts en même temps sur ce
    /// connecteur : utile sur SSD ou stockage à forte latence par fichier,
    /// contre-productif sur un disque mécanique (lectures dispersées).
    /// Le résultat (empreintes, anomalies et leur ordre) est identique.
    #[must_use]
    pub fn with_hash_workers(mut self, workers: NonZeroUsize) -> Self {
        self.hash_workers = workers;
        self
    }

    /// Lectures d'empreinte simultanées.
    #[must_use]
    pub fn hash_workers(&self) -> NonZeroUsize {
        self.hash_workers
    }
}

/// Échec d'un scan.
///
/// Les anomalies localisées (sous-répertoire illisible, empreinte
/// impossible) ne sont **pas** des échecs : elles figurent dans
/// l'inventaire, qui marque les zones incomplètes.
#[derive(Debug)]
#[non_exhaustive]
pub enum ScanError {
    /// La racine de l'extrémité n'a pas pu être listée : aucun inventaire
    /// n'existe, ce qui ne signifie **pas** que l'extrémité est vide.
    RootUnavailable(StorageError),
    /// Le scan a été annulé ; l'inventaire partiel est conservé, avec les
    /// répertoires non parcourus marqués incomplets.
    Cancelled {
        /// Inventaire partiel.
        partial: Box<Inventory>,
    },
    /// L'empreinte demandée ne peut pas être obtenue de ce connecteur.
    HashingNotSupported {
        /// Algorithme demandé.
        algorithm: HashAlgorithm,
    },
    /// A directory worker terminated unexpectedly; the partial inventory is
    /// diagnostic only and must never authorize mutations.
    WorkerFailed {
        /// Observations collected before the worker failure.
        partial: Box<Inventory>,
    },
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootUnavailable(error) => write!(f, "racine inaccessible : {error}"),
            Self::Cancelled { .. } => f.write_str("scan annulé (inventaire partiel conservé)"),
            Self::HashingNotSupported { algorithm } => {
                write!(f, "empreinte {algorithm:?} indisponible pour ce connecteur")
            }
            Self::WorkerFailed { .. } => f.write_str("directory listing worker failed"),
        }
    }
}

impl Error for ScanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RootUnavailable(error) => Some(error),
            _ => None,
        }
    }
}

impl ClassifiedError for ScanError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::RootUnavailable(error) => error.class(),
            Self::Cancelled { .. } => ErrorClass::Cancelled,
            Self::HashingNotSupported { .. } => ErrorClass::NotSupported,
            Self::WorkerFailed { .. } => ErrorClass::Internal,
        }
    }

    fn context(&self) -> Option<&ErrorContext> {
        match self {
            Self::RootUnavailable(error) => error.context(),
            _ => None,
        }
    }
}

/// Inventorie l'extrémité servie par `connector`, depuis sa racine.
///
/// - Ne fait **aucune** mutation : seules `list`, `stat` et `open_read` sont
///   appelées.
/// - Les liens symboliques et entrées spéciales sont inventoriés mais jamais
///   parcourus.
/// - Un sous-répertoire illisible (accès refusé, déconnexion) est marqué
///   incomplet et le scan continue ailleurs.
/// - Le parcours est itératif (pas de récursion) ; l'annulation est
///   consultée avant chaque page et chaque bloc lu.
///
/// # Errors
/// [`ScanError::RootUnavailable`] si la racine ne peut pas être listée,
/// [`ScanError::Cancelled`] (avec l'inventaire partiel) en cas d'annulation,
/// [`ScanError::HashingNotSupported`] si l'empreinte demandée est impossible.
pub fn scan(
    connector: &dyn StorageConnector,
    filter: &Filter,
    options: &ScanOptions,
    cancel: &CancellationToken,
) -> Result<Inventory, ScanError> {
    scan_observed(connector, filter, options, cancel, &|_, _| {})
}

/// Scans with synchronous aggregate observations after directories and hashes.
/// The observer receives cumulative stats and issue count for this endpoint;
/// it must return promptly and must not mutate storage or authorize operations.
/// No observation queue is retained. Guarantees and errors are those of [`scan`].
/// # Errors
/// Same errors and partial-inventory semantics as [`scan`].
pub fn scan_observed(
    connector: &dyn StorageConnector,
    filter: &Filter,
    options: &ScanOptions,
    cancel: &CancellationToken,
    observe: &dyn Fn(&crate::ScanStats, usize),
) -> Result<Inventory, ScanError> {
    let capabilities = connector.capabilities();
    if let Some(algorithm) = options.hash {
        ensure_hashable(connector, algorithm)?;
    }
    let mut inventory = Inventory::new(
        connector.connector_id().clone(),
        capabilities.clone(),
        filter.rules().clone(),
        options.hash,
    );

    let budget = AtomicUsize::new(options.entry_limit);
    let workers = options
        .listing_workers
        .get()
        .min(connector.listing_concurrency().get())
        .min(16);
    if workers > 1 {
        inventory = parallel::list(
            connector, filter, options, cancel, observe, inventory, workers,
        )?;
    } else {
        let mut pending = vec![ResourcePath::root()];
        while let Some(directory) = pending.pop() {
            if cancel.checkpoint() {
                pending.push(directory);
                return Err(cancelled(inventory, &pending));
            }
            match list_directory(
                connector,
                filter,
                &mut inventory,
                &directory,
                cancel,
                &budget,
            ) {
                Ok(children) => {
                    // Ordre inverse : la pile dépile les enfants dans l'ordre trié.
                    pending.extend(children.into_iter().rev());
                }
                Err(error) if error.kind() == StorageErrorKind::Cancelled => {
                    pending.push(directory);
                    return Err(cancelled(inventory, &pending));
                }
                Err(error) if directory.is_root() => return Err(ScanError::RootUnavailable(error)),
                Err(error) => {
                    let kind = error.kind();
                    inventory.directories.insert(
                        directory.clone(),
                        DirectoryState::Incomplete(IncompleteReason::ListFailed(kind)),
                    );
                    inventory
                        .issues
                        .push(ScanIssue::new(directory, ScanIssueKind::ListFailed(kind)));
                }
            }
            observe(&inventory.stats, inventory.issues.len());
        }
    }

    inventory.issues.sort_by(|a, b| a.path().cmp(b.path()));
    if let Some(algorithm) = options.hash {
        let targets = missing_hashes(&inventory, algorithm, |_| true);
        let pass = hash_files(
            connector,
            &mut inventory,
            algorithm,
            targets,
            options.hash_workers,
            cancel,
            observe,
        );
        if pass == HashPass::Cancelled {
            return Err(cancelled(inventory, &[]));
        }
    }
    observe(&inventory.stats, inventory.issues.len());
    Ok(inventory)
}

/// Complète les empreintes de **certains** fichiers d'un inventaire déjà
/// produit par [`scan`] (comparaison de contenu paresseuse : seuls les
/// fichiers dont le contenu doit être départagé sont relus).
///
/// L'algorithme et le parallélisme sont ceux de `options`
/// ([`ScanOptions::with_hash`], [`ScanOptions::with_hash_workers`]).
/// Les chemins absents de l'inventaire, qui ne sont pas des fichiers
/// ordinaires ou déjà munis d'une empreinte de cet algorithme sont ignorés.
/// Mêmes règles que le scan : un fichier modifié pendant sa lecture ou
/// illisible reste **sans** empreinte, avec une anomalie
/// ([`ScanIssueKind::ChangedDuringHash`], [`ScanIssueKind::HashFailed`]).
/// [`Inventory::hash_algorithm`] reste inchangé : l'empreinte n'est pas
/// garantie pour tous les fichiers.
///
/// # Errors
/// [`ScanError::HashingNotSupported`] si `options` ne demande aucune
/// empreinte ou si celle-ci est impossible pour ce connecteur ;
/// [`ScanError::Cancelled`] avec l'inventaire (empreintes déjà vérifiées
/// conservées).
pub fn hash_selected(
    connector: &dyn StorageConnector,
    mut inventory: Inventory,
    paths: &[ResourcePath],
    options: &ScanOptions,
    cancel: &CancellationToken,
    observe: &dyn Fn(&crate::ScanStats, usize),
) -> Result<Inventory, ScanError> {
    let algorithm = options.hash.ok_or(ScanError::HashingNotSupported {
        algorithm: HashAlgorithm::Blake3,
    })?;
    ensure_hashable(connector, algorithm)?;
    let wanted: std::collections::BTreeSet<&ResourcePath> = paths.iter().collect();
    let targets = missing_hashes(&inventory, algorithm, |path| wanted.contains(path));
    let pass = hash_files(
        connector,
        &mut inventory,
        algorithm,
        targets,
        options.hash_workers,
        cancel,
        observe,
    );
    if pass == HashPass::Cancelled {
        return Err(ScanError::Cancelled {
            partial: Box::new(inventory),
        });
    }
    observe(&inventory.stats, inventory.issues.len());
    Ok(inventory)
}

fn ensure_hashable(
    connector: &dyn StorageConnector,
    algorithm: HashAlgorithm,
) -> Result<(), ScanError> {
    let capabilities = connector.capabilities();
    let native = capabilities.native_hashes.contains(&algorithm);
    let computable =
        algorithm == HashAlgorithm::Blake3 && capabilities.supports(StorageFeature::Read);
    if native || computable {
        Ok(())
    } else {
        Err(ScanError::HashingNotSupported { algorithm })
    }
}

/// Fichiers inclus sans empreinte `algorithm`, triés par chemin.
fn missing_hashes(
    inventory: &Inventory,
    algorithm: HashAlgorithm,
    keep: impl Fn(&ResourcePath) -> bool,
) -> Vec<FileEntry> {
    inventory
        .entries
        .values()
        .filter(|entry| {
            entry.kind() == EntryKind::File
                && entry.metadata().hash().map(ContentHash::algorithm) != Some(algorithm)
                && keep(entry.path())
        })
        .cloned()
        .collect()
}

/// Marque les répertoires restants comme non parcourus et emballe l'inventaire.
fn cancelled(mut inventory: Inventory, pending: &[ResourcePath]) -> ScanError {
    for directory in pending {
        inventory.directories.insert(
            directory.clone(),
            DirectoryState::Incomplete(IncompleteReason::NotScanned),
        );
    }
    ScanError::Cancelled {
        partial: Box::new(inventory),
    }
}

/// Liste toutes les pages de `directory`, enregistre les entrées et renvoie
/// les sous-répertoires inclus à parcourir (triés).
///
/// En cas d'erreur après une ou plusieurs pages, les entrées déjà vues sont
/// gardées et l'erreur est renvoyée (le répertoire sera marqué incomplet).
fn list_directory(
    connector: &dyn StorageConnector,
    filter: &Filter,
    inventory: &mut Inventory,
    directory: &ResourcePath,
    cancel: &CancellationToken,
    budget: &AtomicUsize,
) -> Result<Vec<ResourcePath>, StorageError> {
    let mut state = if connector.capabilities().listing_consistency
        == connector_api::ListingConsistency::Strong
    {
        DirectoryState::Complete
    } else {
        inventory.stats.consistency_warnings =
            inventory.stats.consistency_warnings.saturating_add(1);
        inventory.issues.push(ScanIssue::new(
            directory.clone(),
            ScanIssueKind::UnreliableConsistency,
        ));
        DirectoryState::Incomplete(IncompleteReason::UnreliableConsistency)
    };
    let mut children = Vec::new();
    let mut token = None;
    let mut seen_tokens = std::collections::HashSet::new();
    loop {
        let page = connector.list(directory, token.as_ref(), cancel)?;
        let (entries, issues, next) = page.into_parts();
        let cost = entries.len().saturating_add(issues.len()).max(1);
        budget
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                left.checked_sub(cost)
            })
            .map_err(|_| {
                StorageError::new(
                    StorageErrorKind::Permanent,
                    connector_api::StorageOperation::List,
                )
                .with_path(directory.clone())
                .with_message("inventory entry/page budget exceeded; incomplete scan")
            })?;
        for issue in issues {
            if issue.omits_entry() {
                state = DirectoryState::Incomplete(IncompleteReason::OmittedEntries);
            }
            inventory.issues.push(ScanIssue::new(
                directory.clone(),
                ScanIssueKind::Listing(issue),
            ));
        }
        for entry in entries {
            if entry.path().parent().as_ref() != Some(directory) {
                state = DirectoryState::Incomplete(IncompleteReason::InconsistentListing);
                inventory.issues.push(ScanIssue::new(
                    directory.clone(),
                    ScanIssueKind::UnexpectedEntry(entry.path().clone()),
                ));
                continue;
            }
            if let Some(child) = record_entry(filter, inventory, entry, &mut state) {
                children.push(child);
            }
        }
        match next {
            Some(next) => {
                if !seen_tokens.insert(next.clone()) {
                    return Err(StorageError::new(
                        StorageErrorKind::Conflict,
                        connector_api::StorageOperation::List,
                    )
                    .with_path(directory.clone())
                    .with_message("repeated listing cursor; incomplete scan"));
                }
                if cancel.checkpoint() {
                    return Err(StorageError::new(
                        StorageErrorKind::Cancelled,
                        connector_api::StorageOperation::List,
                    ));
                }
                token = Some(next);
            }
            None => break,
        }
    }
    inventory.directories.insert(directory.clone(), state);
    children.sort();
    Ok(children)
}

/// Enregistre une entrée (ou son exclusion) ; renvoie le chemin à parcourir
/// si c'est un répertoire inclus.
fn record_entry(
    filter: &Filter,
    inventory: &mut Inventory,
    entry: FileEntry,
    state: &mut DirectoryState,
) -> Option<ResourcePath> {
    let path = entry.path().clone();
    if inventory.entries.contains_key(&path) || inventory.excluded.contains_key(&path) {
        *state = DirectoryState::Incomplete(IncompleteReason::InconsistentListing);
        inventory
            .issues
            .push(ScanIssue::new(path, ScanIssueKind::DuplicateEntry));
        return None;
    }
    let kind = entry.kind();
    match filter.evaluate(&path, kind) {
        FilterDecision::Exclude(exclusion) => {
            inventory.stats.excluded += 1;
            inventory.excluded.insert(path, (kind, exclusion));
            None
        }
        FilterDecision::Include => {
            let stats = &mut inventory.stats;
            match kind {
                EntryKind::File => {
                    stats.files += 1;
                    stats.known_bytes = stats
                        .known_bytes
                        .saturating_add(entry.metadata().size().unwrap_or(0));
                }
                EntryKind::Directory => stats.directories += 1,
                _ => stats.others += 1,
            }
            if entry.metadata().hash().is_some() {
                stats.hashed += 1;
            }
            inventory.entries.insert(path.clone(), entry);
            (kind == EntryKind::Directory).then_some(path)
        }
    }
}

/// Issue de la passe de calcul des empreintes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashPass {
    Completed,
    Cancelled,
}

/// Issue du calcul d'une empreinte.
enum Hashed {
    Ok(ContentHash),
    Issue(ScanIssueKind),
    Cancelled,
}

/// Calcule les empreintes manquantes de `targets` (triés), avec au plus
/// `workers` lectures simultanées. Les résultats sont appliqués dans l'ordre
/// des chemins, quel que soit l'ordre d'achèvement : inventaire et anomalies
/// sont identiques en séquentiel et en parallèle.
fn hash_files(
    connector: &dyn StorageConnector,
    inventory: &mut Inventory,
    algorithm: HashAlgorithm,
    targets: Vec<FileEntry>,
    workers: NonZeroUsize,
    cancel: &CancellationToken,
    observe: &dyn Fn(&crate::ScanStats, usize),
) -> HashPass {
    let can_compute = algorithm == HashAlgorithm::Blake3
        && connector.capabilities().supports(StorageFeature::Read);
    if !can_compute {
        for entry in targets {
            inventory.issues.push(ScanIssue::new(
                entry.path().clone(),
                ScanIssueKind::HashFailed(StorageErrorKind::NotSupported),
            ));
        }
        return HashPass::Completed;
    }
    let can_restat = connector.capabilities().supports(StorageFeature::Stat);
    let workers = workers.get().min(targets.len()).max(1);
    let mut results: Vec<Option<Hashed>> = Vec::with_capacity(targets.len());
    results.resize_with(targets.len(), || None);
    let issues = inventory.issues.len();
    let stats = &mut inventory.stats;
    let mut record = |slot: &mut Option<Hashed>, outcome: Hashed| {
        if matches!(outcome, Hashed::Ok(_)) {
            stats.hashed += 1;
            observe(stats, issues);
        }
        *slot = Some(outcome);
    };
    if workers == 1 {
        let mut buffer = vec![0u8; HASH_CHUNK];
        for (slot, entry) in results.iter_mut().zip(&targets) {
            if cancel.checkpoint() {
                break;
            }
            let outcome = hash_one(connector, entry, can_restat, &mut buffer, cancel);
            let stop = matches!(outcome, Hashed::Cancelled);
            record(slot, outcome);
            if stop {
                break;
            }
        }
    } else {
        let next = AtomicUsize::new(0);
        // Bounded hand-off: workers block instead of queueing results.
        let (sender, receiver) = mpsc::sync_channel::<(usize, Hashed)>(workers * 4);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let sender = sender.clone();
                let (next, targets) = (&next, &targets);
                scope.spawn(move || {
                    let mut buffer = vec![0u8; HASH_CHUNK];
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(entry) = targets.get(index) else {
                            break;
                        };
                        if cancel.checkpoint() {
                            break;
                        }
                        let outcome = hash_one(connector, entry, can_restat, &mut buffer, cancel);
                        if sender.send((index, outcome)).is_err() {
                            break;
                        }
                    }
                });
            }
            drop(sender);
            for (index, outcome) in receiver {
                if let Some(slot) = results.get_mut(index) {
                    record(slot, outcome);
                }
            }
        });
    }
    let mut pass = HashPass::Completed;
    for (entry, outcome) in targets.into_iter().zip(results) {
        match outcome {
            Some(Hashed::Ok(hash)) => {
                let path = entry.path().clone();
                let metadata = entry.metadata().clone().with_hash(hash);
                inventory
                    .entries
                    .insert(path, entry.with_metadata(metadata));
            }
            Some(Hashed::Issue(kind)) => inventory
                .issues
                .push(ScanIssue::new(entry.path().clone(), kind)),
            Some(Hashed::Cancelled) | None => pass = HashPass::Cancelled,
        }
    }
    if cancel.checkpoint() {
        pass = HashPass::Cancelled;
    }
    pass
}

/// Empreinte d'un fichier, écartée s'il a changé pendant sa lecture.
fn hash_one(
    connector: &dyn StorageConnector,
    before: &FileEntry,
    can_restat: bool,
    buffer: &mut [u8],
    cancel: &CancellationToken,
) -> Hashed {
    match blake3_of(connector, before.path(), buffer, cancel) {
        Ok((hash, read)) => {
            let size_matches = before.metadata().size().is_none_or(|size| size == read);
            if size_matches && (!can_restat || unchanged_since(connector, before, cancel)) {
                Hashed::Ok(hash)
            } else {
                Hashed::Issue(ScanIssueKind::ChangedDuringHash)
            }
        }
        Err(error) if error.kind() == StorageErrorKind::Cancelled => Hashed::Cancelled,
        Err(error) => Hashed::Issue(ScanIssueKind::HashFailed(error.kind())),
    }
}

/// Lit tout le contenu en flux et renvoie l'empreinte BLAKE3 et le nombre
/// d'octets lus.
fn blake3_of(
    connector: &dyn StorageConnector,
    path: &ResourcePath,
    buffer: &mut [u8],
    cancel: &CancellationToken,
) -> Result<(ContentHash, u64), StorageError> {
    let mut reader = connector.open_read(path, cancel)?;
    let mut hasher = blake3::Hasher::new();
    let mut total: u64 = 0;
    loop {
        let read = reader.read(buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
        total = total.saturating_add(read as u64);
    }
    let hash = ContentHash::new(HashAlgorithm::Blake3, hasher.finalize().as_bytes().to_vec())
        .map_err(|_| {
            StorageError::new(
                StorageErrorKind::Permanent,
                connector_api::StorageOperation::Read,
            )
        })?;
    Ok((hash, total))
}

/// Vrai si l'entrée a les mêmes nature, taille et date qu'avant la lecture.
fn unchanged_since(
    connector: &dyn StorageConnector,
    before: &FileEntry,
    cancel: &CancellationToken,
) -> bool {
    match connector.stat(before.path(), cancel) {
        Ok(after) => {
            after.kind() == before.kind()
                && after.metadata().size() == before.metadata().size()
                && after.metadata().modified() == before.metadata().modified()
        }
        Err(_) => false,
    }
}
