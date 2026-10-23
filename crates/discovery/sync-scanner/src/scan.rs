//! Parcours d'une extrémité.

use std::error::Error;
use std::fmt;

use connector_api::{StorageConnector, StorageError, StorageErrorKind, StorageFeature};
use sync_core::{
    CancellationToken, ContentHash, EntryKind, FileEntry, HashAlgorithm, ResourcePath,
};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext};
use sync_filter::{Filter, FilterDecision};

use crate::inventory::{DirectoryState, IncompleteReason, Inventory, ScanIssue, ScanIssueKind};

/// Taille des blocs lus pour le calcul d'empreinte.
const HASH_CHUNK: usize = 64 * 1024;

/// Options d'un scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    hash: Option<HashAlgorithm>,
    entry_limit: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            hash: None,
            entry_limit: 2_000_000,
        }
    }
}
impl ScanOptions {
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
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootUnavailable(error) => write!(f, "racine inaccessible : {error}"),
            Self::Cancelled { .. } => f.write_str("scan annulé (inventaire partiel conservé)"),
            Self::HashingNotSupported { algorithm } => {
                write!(f, "empreinte {algorithm:?} indisponible pour ce connecteur")
            }
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
        let native = capabilities.native_hashes.contains(&algorithm);
        let computable =
            algorithm == HashAlgorithm::Blake3 && capabilities.supports(StorageFeature::Read);
        if !native && !computable {
            return Err(ScanError::HashingNotSupported { algorithm });
        }
    }
    let mut inventory = Inventory::new(
        connector.connector_id().clone(),
        capabilities.clone(),
        filter.rules().clone(),
        options.hash,
    );

    let mut budget = options.entry_limit;
    let mut pending = vec![ResourcePath::root()];
    while let Some(directory) = pending.pop() {
        if cancel.is_cancelled() {
            pending.push(directory);
            return Err(cancelled(inventory, &pending));
        }
        match list_directory(
            connector,
            filter,
            &mut inventory,
            &directory,
            cancel,
            &mut budget,
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

    if let Some(algorithm) = options.hash
        && hash_files(connector, &mut inventory, algorithm, cancel, observe) == HashPass::Cancelled
    {
        return Err(cancelled(inventory, &[]));
    }
    observe(&inventory.stats, inventory.issues.len());
    Ok(inventory)
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
    budget: &mut usize,
) -> Result<Vec<ResourcePath>, StorageError> {
    let mut state = if connector.capabilities().listing_consistency
        == connector_api::ListingConsistency::Strong
    {
        DirectoryState::Complete
    } else {
        inventory.issues.push(ScanIssue::new(
            directory.clone(),
            ScanIssueKind::UnreliableConsistency,
        ));
        DirectoryState::Incomplete(IncompleteReason::UnreliableConsistency)
    };
    let mut children = Vec::new();
    let mut token = None;
    loop {
        let page = connector.list(directory, token.as_ref(), cancel)?;
        let (entries, issues, next) = page.into_parts();
        let cost = entries.len().saturating_add(issues.len()).max(1);
        *budget = budget.checked_sub(cost).ok_or_else(|| {
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
                if cancel.is_cancelled() {
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

/// Calcule les empreintes manquantes.
fn hash_files(
    connector: &dyn StorageConnector,
    inventory: &mut Inventory,
    algorithm: HashAlgorithm,
    cancel: &CancellationToken,
    observe: &dyn Fn(&crate::ScanStats, usize),
) -> HashPass {
    let targets: Vec<ResourcePath> = inventory
        .entries
        .values()
        .filter(|entry| {
            entry.kind() == EntryKind::File
                && entry.metadata().hash().map(ContentHash::algorithm) != Some(algorithm)
        })
        .map(|entry| entry.path().clone())
        .collect();
    let can_compute = algorithm == HashAlgorithm::Blake3
        && connector.capabilities().supports(StorageFeature::Read);
    let can_restat = connector.capabilities().supports(StorageFeature::Stat);
    let mut buffer = vec![0u8; HASH_CHUNK];

    for path in targets {
        if cancel.is_cancelled() {
            return HashPass::Cancelled;
        }
        if !can_compute {
            inventory.issues.push(ScanIssue::new(
                path,
                ScanIssueKind::HashFailed(StorageErrorKind::NotSupported),
            ));
            continue;
        }
        let Some(before) = inventory.entries.get(&path).cloned() else {
            continue;
        };
        let outcome = blake3_of(connector, &path, &mut buffer, cancel);
        let hash = match outcome {
            Ok((hash, read)) => {
                let size_matches = before.metadata().size().is_none_or(|size| size == read);
                let unchanged =
                    size_matches && (!can_restat || unchanged_since(connector, &before, cancel));
                if unchanged {
                    hash
                } else {
                    inventory
                        .issues
                        .push(ScanIssue::new(path, ScanIssueKind::ChangedDuringHash));
                    continue;
                }
            }
            Err(error) if error.kind() == StorageErrorKind::Cancelled => {
                return HashPass::Cancelled;
            }
            Err(error) => {
                inventory.issues.push(ScanIssue::new(
                    path,
                    ScanIssueKind::HashFailed(error.kind()),
                ));
                continue;
            }
        };
        let metadata = before.metadata().clone().with_hash(hash);
        if let Some(entry) = inventory.entries.get_mut(&path) {
            *entry = before.with_metadata(metadata);
            inventory.stats.hashed += 1;
        }
        observe(&inventory.stats, inventory.issues.len());
    }
    HashPass::Completed
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
