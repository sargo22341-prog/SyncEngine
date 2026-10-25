//! Inventaire produit par un scan : entrées observées et couverture.

use std::collections::BTreeMap;
use std::fmt;

use connector_api::{ListingIssue, StorageCapabilities, StorageErrorKind};
use sync_core::{ConnectorId, EntryKind, FileEntry, HashAlgorithm, ResourcePath};
use sync_filter::{Exclusion, FilterRules};

/// État du listing d'un répertoire parcouru.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DirectoryState {
    /// Toutes les entrées du répertoire ont été vues : l'absence d'un nom
    /// y est un fait.
    Complete,
    /// Le listing est incomplet : l'absence d'un nom n'y prouve rien.
    Incomplete(IncompleteReason),
}

impl DirectoryState {
    /// Vrai si le listing est complet.
    #[must_use]
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Raison d'un listing incomplet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IncompleteReason {
    /// Le stockage ne garantit pas qu'un listing reflète les écritures terminées.
    UnreliableConsistency,
    /// Le listing a échoué (éventuellement après quelques pages).
    ListFailed(StorageErrorKind),
    /// Des entrées existent mais n'ont pas pu être représentées ou lues.
    OmittedEntries,
    /// Le stockage a renvoyé des entrées incohérentes (hors du répertoire,
    /// en double).
    InconsistentListing,
    /// Le répertoire n'a pas été parcouru (scan annulé avant).
    NotScanned,
}

/// Anomalie non fatale relevée pendant le scan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanIssueKind {
    /// Cohérence inconnue ou éventuelle : le listing ne prouve aucune absence.
    UnreliableConsistency,
    /// Le listing du répertoire a échoué.
    ListFailed(StorageErrorKind),
    /// Anomalie d'entrée signalée par le connecteur.
    Listing(ListingIssue),
    /// Le connecteur a renvoyé une entrée qui n'est pas un enfant direct du
    /// répertoire listé : elle est ignorée.
    UnexpectedEntry(ResourcePath),
    /// Le connecteur a renvoyé deux fois le même chemin : seule la première
    /// occurrence est gardée.
    DuplicateEntry,
    /// L'empreinte n'a pas pu être calculée.
    HashFailed(StorageErrorKind),
    /// Le fichier a changé pendant le calcul de son empreinte : empreinte
    /// écartée.
    ChangedDuringHash,
}

/// Anomalie non fatale, rattachée à un chemin logique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    path: ResourcePath,
    kind: ScanIssueKind,
}

impl ScanIssue {
    pub(crate) fn new(path: ResourcePath, kind: ScanIssueKind) -> Self {
        Self { path, kind }
    }

    /// Chemin concerné (le répertoire listé pour une anomalie de listing).
    #[must_use]
    pub fn path(&self) -> &ResourcePath {
        &self.path
    }

    /// Nature de l'anomalie.
    #[must_use]
    pub fn kind(&self) -> &ScanIssueKind {
        &self.kind
    }
}

impl fmt::Display for ScanIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = if self.path.is_root() {
            "<racine>"
        } else {
            self.path.as_str()
        };
        match &self.kind {
            ScanIssueKind::UnreliableConsistency => {
                write!(f, "{path} : cohérence du listing non garantie")
            }
            ScanIssueKind::ListFailed(kind) => write!(f, "{path} : listing impossible ({kind:?})"),
            ScanIssueKind::Listing(issue) => write!(f, "{path} : {issue}"),
            ScanIssueKind::UnexpectedEntry(other) => {
                write!(f, "{path} : entrée hors du répertoire ignorée ({other})")
            }
            ScanIssueKind::DuplicateEntry => write!(f, "{path} : entrée en double ignorée"),
            ScanIssueKind::HashFailed(kind) => {
                write!(f, "{path} : empreinte impossible ({kind:?})")
            }
            ScanIssueKind::ChangedDuringHash => {
                write!(f, "{path} : modifié pendant la lecture, empreinte écartée")
            }
        }
    }
}

/// Raison pour laquelle la présence d'un chemin est inconnue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnknownReason {
    /// Le répertoire parent (ou un ancêtre) n'a pas été listé complètement.
    IncompleteDirectory(IncompleteReason),
    /// Un ancêtre est un lien symbolique ou une entrée spéciale, non suivi.
    BehindUnfollowedEntry,
}

/// Présence observée d'un chemin dans un inventaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence<'a> {
    /// L'entrée a été observée.
    Present(&'a FileEntry),
    /// L'absence est **établie** : le répertoire qui la contiendrait a été
    /// listé complètement (ou un ancêtre est un fichier ordinaire).
    Absent,
    /// Le chemin, ou un ancêtre, est hors du périmètre du filtre.
    Excluded,
    /// Rien ne peut être conclu.
    Unknown(UnknownReason),
}

/// Statistiques d'un scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    /// Directories whose listing consistency is not guaranteed by the provider.
    /// These are uncertainty warnings, not failed reads; absence remains unknown.
    pub consistency_warnings: u64,
    /// Fichiers ordinaires inventoriés.
    pub files: u64,
    /// Répertoires inventoriés (hors racine).
    pub directories: u64,
    /// Liens symboliques et entrées spéciales inventoriés.
    pub others: u64,
    /// Somme des tailles connues des fichiers.
    pub known_bytes: u64,
    /// Entrées écartées par le filtre (sans compter le contenu des
    /// répertoires exclus, qui n'est pas parcouru).
    pub excluded: u64,
    /// Empreintes calculées ou fournies.
    pub hashed: u64,
}

/// Inventaire d'une extrémité.
///
/// L'inventaire est un **constat** : il ne décide de rien. Sa couverture est
/// explicite : seule [`Inventory::presence`] permet de savoir si l'absence
/// d'un chemin est un fait ou une inconnue.
///
/// Limite : l'inventaire est tenu en mémoire (de l'ordre de quelques
/// centaines d'octets par entrée) ; un mode en flux est prévu en phase
/// performance.
#[derive(Debug, Clone)]
pub struct Inventory {
    pub(crate) connector: ConnectorId,
    pub(crate) capabilities: StorageCapabilities,
    pub(crate) filter: FilterRules,
    pub(crate) hash_algorithm: Option<HashAlgorithm>,
    pub(crate) entries: BTreeMap<ResourcePath, FileEntry>,
    pub(crate) directories: BTreeMap<ResourcePath, DirectoryState>,
    pub(crate) excluded: BTreeMap<ResourcePath, (EntryKind, Exclusion)>,
    pub(crate) issues: Vec<ScanIssue>,
    pub(crate) stats: ScanStats,
}

impl Inventory {
    pub(crate) fn new(
        connector: ConnectorId,
        capabilities: StorageCapabilities,
        filter: FilterRules,
        hash_algorithm: Option<HashAlgorithm>,
    ) -> Self {
        Self {
            connector,
            capabilities,
            filter,
            hash_algorithm,
            entries: BTreeMap::new(),
            directories: BTreeMap::new(),
            excluded: BTreeMap::new(),
            issues: Vec::new(),
            stats: ScanStats::default(),
        }
    }

    /// Connecteur inventorié.
    #[must_use]
    pub fn connector(&self) -> &ConnectorId {
        &self.connector
    }

    /// Capacités du connecteur au moment du scan.
    #[must_use]
    pub fn capabilities(&self) -> &StorageCapabilities {
        &self.capabilities
    }

    /// Règles de filtre appliquées.
    #[must_use]
    pub fn filter_rules(&self) -> &FilterRules {
        &self.filter
    }

    /// Algorithme d'empreinte demandé, s'il y en a un.
    #[must_use]
    pub fn hash_algorithm(&self) -> Option<HashAlgorithm> {
        self.hash_algorithm
    }

    /// Entrées incluses, triées par chemin.
    pub fn entries(&self) -> impl Iterator<Item = &FileEntry> {
        self.entries.values()
    }

    /// Nombre d'entrées incluses.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Vrai si aucune entrée n'a été inventoriée.
    ///
    /// Attention : un inventaire vide n'est une preuve de vacuité que si
    /// [`Inventory::is_complete`] est vrai.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entrée observée en `path`.
    #[must_use]
    pub fn get(&self, path: &ResourcePath) -> Option<&FileEntry> {
        self.entries.get(path)
    }

    /// États des répertoires parcourus (racine comprise).
    pub fn directories(&self) -> impl Iterator<Item = (&ResourcePath, DirectoryState)> {
        self.directories.iter().map(|(path, state)| (path, *state))
    }

    /// Répertoires dont le listing est incomplet.
    pub fn incomplete_directories(
        &self,
    ) -> impl Iterator<Item = (&ResourcePath, IncompleteReason)> {
        self.directories
            .iter()
            .filter_map(|(path, state)| match state {
                DirectoryState::Incomplete(reason) => Some((path, *reason)),
                DirectoryState::Complete => None,
            })
    }

    /// Entrées écartées par le filtre (premier niveau seulement).
    pub fn excluded(&self) -> impl Iterator<Item = (&ResourcePath, EntryKind, Exclusion)> {
        self.excluded
            .iter()
            .map(|(path, (kind, exclusion))| (path, *kind, *exclusion))
    }

    /// Anomalies non fatales.
    #[must_use]
    pub fn issues(&self) -> &[ScanIssue] {
        &self.issues
    }

    /// Statistiques.
    #[must_use]
    pub fn stats(&self) -> ScanStats {
        self.stats
    }

    /// Vrai si tous les répertoires parcourus ont été listés complètement.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.directories.values().all(|state| state.is_complete())
    }

    /// Présence de `path` dans l'extrémité, telle que le scan permet de
    /// l'établir.
    ///
    /// Une absence n'est rapportée comme [`Presence::Absent`] que si le
    /// répertoire qui contiendrait `path` a été listé **complètement** ;
    /// sinon la réponse est [`Presence::Unknown`]. La racine est toujours
    /// présente si le scan a abouti, elle est donc rapportée comme inconnue
    /// ici (elle n'est pas une entrée).
    #[must_use]
    pub fn presence(&self, path: &ResourcePath) -> Presence<'_> {
        if let Some(entry) = self.entries.get(path) {
            return Presence::Present(entry);
        }
        if self.excluded.contains_key(path) {
            return Presence::Excluded;
        }
        let mut ancestor = path.parent();
        while let Some(dir) = ancestor {
            if self.excluded.contains_key(&dir) {
                return Presence::Excluded;
            }
            let kind = if dir.is_root() {
                Some(EntryKind::Directory)
            } else {
                self.entries.get(&dir).map(FileEntry::kind)
            };
            match kind {
                Some(EntryKind::Directory) => {
                    return match self.directories.get(&dir) {
                        Some(DirectoryState::Complete) => Presence::Absent,
                        Some(DirectoryState::Incomplete(reason)) => {
                            Presence::Unknown(UnknownReason::IncompleteDirectory(*reason))
                        }
                        None => Presence::Unknown(UnknownReason::IncompleteDirectory(
                            IncompleteReason::NotScanned,
                        )),
                    };
                }
                // Un fichier ordinaire ne contient rien : l'absence est établie.
                Some(EntryKind::File) => return Presence::Absent,
                Some(_) => return Presence::Unknown(UnknownReason::BehindUnfollowedEntry),
                None => ancestor = dir.parent(),
            }
        }
        Presence::Unknown(UnknownReason::IncompleteDirectory(
            IncompleteReason::NotScanned,
        ))
    }
}
