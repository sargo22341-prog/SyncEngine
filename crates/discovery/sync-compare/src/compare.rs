//! Comparaison d'entrées et d'inventaires.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use connector_api::CaseSensitivity;
use sync_core::{ConnectorId, EntryKind, FileEntry, FileMetadata, ResourcePath, Timestamp};
use sync_errors::{ClassifiedError, ErrorClass};
use sync_filter::FilterRules;
use sync_scanner::{Inventory, Presence};
use thiserror::Error;

use crate::outcome::{
    ComparisonOutcome, Difference, EquivalenceBasis, OutcomeCategory, PathComparison, Side,
    Uncertainty,
};

/// Options de comparaison.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompareOptions {
    modified_tolerance: Duration,
}

impl CompareOptions {
    /// Options par défaut : aucune tolérance au-delà de la précision déclarée.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tolérance supplémentaire sur les dates (par exemple pour un volume
    /// dont la précision réelle est plus grossière que celle déclarée).
    /// Elle s'ajoute à la précision commune, sans jamais la réduire.
    #[must_use]
    pub fn with_modified_tolerance(mut self, tolerance: Duration) -> Self {
        self.modified_tolerance = tolerance;
        self
    }

    /// Tolérance supplémentaire sur les dates.
    #[must_use]
    pub fn modified_tolerance(&self) -> Duration {
        self.modified_tolerance
    }
}

/// Erreur de comparaison.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CompareError {
    /// Les deux inventaires n'ont pas été filtrés avec les mêmes règles :
    /// un chemin exclu d'un côté paraîtrait absent.
    #[error("les deux inventaires n'ont pas été filtrés avec les mêmes règles")]
    FilterMismatch,
}

impl ClassifiedError for CompareError {
    fn class(&self) -> ErrorClass {
        ErrorClass::InvalidInput
    }
}

/// Résumé des constats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComparisonSummary {
    /// Présents à gauche seulement.
    pub left_only: u64,
    /// Présents à droite seulement.
    pub right_only: u64,
    /// Équivalents.
    pub equivalent: u64,
    /// Équivalents dont le contenu a été vérifié par empreinte.
    pub content_verified: u64,
    /// Différents.
    pub different: u64,
    /// Incertains.
    pub uncertain: u64,
}

/// Résultat d'une comparaison d'inventaires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonReport {
    rows: Vec<PathComparison>,
    left_complete: bool,
    right_complete: bool,
    summary: ComparisonSummary,
    filter: FilterRules,
    left_connector: ConnectorId,
    right_connector: ConnectorId,
}

impl ComparisonReport {
    /// Constats, triés par chemin.
    #[must_use]
    pub fn rows(&self) -> &[PathComparison] {
        &self.rows
    }

    /// Constats d'une catégorie donnée.
    pub fn rows_in(&self, category: OutcomeCategory) -> impl Iterator<Item = &PathComparison> {
        self.rows
            .iter()
            .filter(move |row| row.outcome.category() == category)
    }

    /// Constat pour `path`, s'il existe.
    #[must_use]
    pub fn get(&self, path: &ResourcePath) -> Option<&PathComparison> {
        self.rows
            .binary_search_by(|row| row.path.cmp(path))
            .ok()
            .and_then(|index| self.rows.get(index))
    }

    /// Vrai si l'inventaire de gauche était complet.
    #[must_use]
    pub fn left_complete(&self) -> bool {
        self.left_complete
    }

    /// Vrai si l'inventaire de droite était complet.
    #[must_use]
    pub fn right_complete(&self) -> bool {
        self.right_complete
    }

    /// Résumé.
    #[must_use]
    pub fn summary(&self) -> ComparisonSummary {
        self.summary
    }

    /// Règles de filtre communes aux deux inventaires (ajout phase 03 : le
    /// plan les enregistre pour que preview et exécution filtrent pareil).
    #[must_use]
    pub fn filter_rules(&self) -> &FilterRules {
        &self.filter
    }

    /// Connecteur de l'inventaire de gauche (ajout phase 03).
    #[must_use]
    pub fn left_connector(&self) -> &ConnectorId {
        &self.left_connector
    }

    /// Connecteur de l'inventaire de droite (ajout phase 03).
    #[must_use]
    pub fn right_connector(&self) -> &ConnectorId {
        &self.right_connector
    }
}

/// Compare deux entrées présentes des deux côtés (même chemin logique).
///
/// Règles, dans l'ordre :
/// 1. natures différentes ⇒ [`Difference::Kind`] ;
/// 2. deux répertoires ⇒ équivalents ; deux liens ou entrées spéciales ⇒
///    incertain (cible non comparée) ;
/// 3. deux fichiers avec empreintes du même algorithme ⇒ le contenu tranche ;
/// 4. sinon taille, puis date à la précision commune près
///    ([`sync_core::TimestampPrecision::coarsest`]) ; une taille ou une date
///    inconnue, ou une précision inconnue, rend le constat incertain.
#[must_use]
pub fn compare_entries(
    left: &FileEntry,
    right: &FileEntry,
    options: &CompareOptions,
) -> ComparisonOutcome {
    if left.kind() != right.kind() {
        return ComparisonOutcome::Different(vec![Difference::Kind {
            left: left.kind(),
            right: right.kind(),
        }]);
    }
    match left.kind() {
        EntryKind::Directory => ComparisonOutcome::Equivalent(EquivalenceBasis::Directory),
        EntryKind::File => compare_files(left.metadata(), right.metadata(), options),
        kind => ComparisonOutcome::Uncertain(vec![Uncertainty::NotComparable { kind }]),
    }
}

fn compare_files(
    left: &FileMetadata,
    right: &FileMetadata,
    options: &CompareOptions,
) -> ComparisonOutcome {
    let sizes = left.size().zip(right.size());
    let modified = compare_modified(left, right, options);

    if let (Some(lh), Some(rh)) = (left.hash(), right.hash())
        && lh.algorithm() == rh.algorithm()
    {
        if lh == rh {
            return match sizes {
                Some((l, r)) if l != r => {
                    ComparisonOutcome::Uncertain(vec![Uncertainty::InconsistentObservations])
                }
                _ => ComparisonOutcome::Equivalent(EquivalenceBasis::ContentHash),
            };
        }
        let mut differences = vec![Difference::ContentHash];
        if let Some((l, r)) = sizes.filter(|(l, r)| l != r) {
            differences.push(Difference::Size { left: l, right: r });
        }
        if let Ok(Some(newer)) = modified {
            differences.push(Difference::Modified { newer });
        }
        return ComparisonOutcome::Different(differences);
    }

    let Some((left_size, right_size)) = sizes else {
        return ComparisonOutcome::Uncertain(vec![Uncertainty::SizeUnknown]);
    };
    let mut differences = Vec::new();
    if left_size != right_size {
        differences.push(Difference::Size {
            left: left_size,
            right: right_size,
        });
    }
    match modified {
        Ok(Some(newer)) => differences.push(Difference::Modified { newer }),
        Ok(None) => {}
        // Des tailles différentes suffisent à établir la différence ; sinon
        // la date manquante empêche toute conclusion.
        Err(uncertainty) if differences.is_empty() => {
            return ComparisonOutcome::Uncertain(vec![uncertainty]);
        }
        Err(_) => {}
    }
    if differences.is_empty() {
        ComparisonOutcome::Equivalent(EquivalenceBasis::SizeAndModified)
    } else {
        ComparisonOutcome::Different(differences)
    }
}

/// `Ok(None)` si les dates sont égales à la précision commune près,
/// `Ok(Some(côté))` pour le côté le plus récent, `Err` si on ne peut conclure.
fn compare_modified(
    left: &FileMetadata,
    right: &FileMetadata,
    options: &CompareOptions,
) -> Result<Option<Side>, Uncertainty> {
    let (Some(l), Some(r)) = (left.modified(), right.modified()) else {
        return Err(Uncertainty::ModifiedUnknown);
    };
    let precision = left
        .modified_precision()
        .coarsest(right.modified_precision());
    let Some(granularity) = precision.granularity() else {
        return Err(Uncertainty::PrecisionUnknown);
    };
    let delta = nanos(l) - nanos(r);
    let distance = delta.unsigned_abs();
    if distance < granularity.as_nanos() || distance <= options.modified_tolerance.as_nanos() {
        Ok(None)
    } else if delta > 0 {
        Ok(Some(Side::Left))
    } else {
        Ok(Some(Side::Right))
    }
}

fn nanos(ts: Timestamp) -> i128 {
    i128::from(ts.secs()) * 1_000_000_000 + i128::from(ts.subsec_nanos())
}

/// Constat pour un chemin présent d'un seul côté.
fn one_sided(present: Side, other: Presence<'_>) -> ComparisonOutcome {
    let other_side = match present {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    };
    match other {
        Presence::Absent => match present {
            Side::Left => ComparisonOutcome::LeftOnly,
            Side::Right => ComparisonOutcome::RightOnly,
        },
        Presence::Excluded => {
            ComparisonOutcome::Uncertain(vec![Uncertainty::ExcludedOnSide { side: other_side }])
        }
        Presence::Unknown(reason) => {
            ComparisonOutcome::Uncertain(vec![Uncertainty::PresenceUnknown {
                side: other_side,
                reason,
            }])
        }
        // Impossible : le chemin vient de l'autre inventaire et n'est pas
        // dans celui-ci. Traité prudemment plutôt que par une panique.
        Presence::Present(_) => {
            ComparisonOutcome::Uncertain(vec![Uncertainty::InconsistentObservations])
        }
    }
}

/// Compare deux inventaires chemin par chemin.
///
/// Chaque chemin présent d'au moins un côté reçoit un constat. Un chemin
/// présent d'un seul côté n'est rapporté `LeftOnly`/`RightOnly` que si son
/// absence de l'autre côté est **établie** ([`Inventory::presence`]) ; sinon
/// il est incertain. Si l'un des côtés n'est pas sûrement sensible à la casse,
/// les chemins qui ne diffèrent que par la casse deviennent incertains.
///
/// # Errors
/// [`CompareError::FilterMismatch`] si les inventaires n'ont pas été filtrés
/// avec les mêmes règles.
pub fn compare(
    left: &Inventory,
    right: &Inventory,
    options: &CompareOptions,
) -> Result<ComparisonReport, CompareError> {
    if left.filter_rules() != right.filter_rules() {
        return Err(CompareError::FilterMismatch);
    }
    let paths: BTreeSet<&ResourcePath> = left
        .entries()
        .chain(right.entries())
        .map(FileEntry::path)
        .collect();

    let collisions = if case_sensitive(left) && case_sensitive(right) {
        BTreeMap::new()
    } else {
        case_collisions(&paths)
    };

    let mut rows = Vec::with_capacity(paths.len());
    let mut summary = ComparisonSummary::default();
    for path in paths {
        let left_entry = left.get(path);
        let right_entry = right.get(path);
        let outcome = match (left_entry, right_entry) {
            (Some(l), Some(r)) => compare_entries(l, r, options),
            (Some(_), None) => one_sided(Side::Left, right.presence(path)),
            (None, Some(_)) => one_sided(Side::Right, left.presence(path)),
            (None, None) => continue,
        };
        let outcome = match collisions.get(path) {
            Some(with) => ComparisonOutcome::Uncertain(vec![Uncertainty::CaseCollision {
                with: with.clone(),
            }]),
            None => outcome,
        };
        match &outcome {
            ComparisonOutcome::LeftOnly => summary.left_only += 1,
            ComparisonOutcome::RightOnly => summary.right_only += 1,
            ComparisonOutcome::Equivalent(basis) => {
                summary.equivalent += 1;
                if basis.is_content_verified() {
                    summary.content_verified += 1;
                }
            }
            ComparisonOutcome::Different(_) => summary.different += 1,
            ComparisonOutcome::Uncertain(_) => summary.uncertain += 1,
        }
        rows.push(PathComparison {
            path: path.clone(),
            left: left_entry.cloned(),
            right: right_entry.cloned(),
            outcome,
        });
    }
    Ok(ComparisonReport {
        rows,
        left_complete: left.is_complete(),
        right_complete: right.is_complete(),
        summary,
        filter: left.filter_rules().clone(),
        left_connector: left.connector().clone(),
        right_connector: right.connector().clone(),
    })
}

/// Fichiers dont seul le contenu peut départager les deux côtés : fichier
/// ordinaire des deux côtés, tailles **égales** (ou inconnues d'un côté),
/// sans empreinte commune déjà disponible. Triés par chemin.
///
/// Des tailles connues et différentes établissent déjà la différence, et un chemin
/// présent d'un seul côté n'a rien à départager : relire ces fichiers ne
/// changerait aucun constat de [`compare`]. Calculer les empreintes de ces
/// seuls candidats ([`sync_scanner::hash_selected`]) donne donc les mêmes
/// constats qu'un scan avec empreinte de tous les fichiers, sans relire le
/// reste (même principe que la comparaison par contenu de FreeFileSync).
#[must_use]
pub fn content_candidates(left: &Inventory, right: &Inventory) -> Vec<ResourcePath> {
    left.entries()
        .filter(|l| l.kind() == EntryKind::File)
        .filter_map(|l| {
            let r = right.get(l.path())?;
            let same_size = r.kind() == EntryKind::File
                && match (l.metadata().size(), r.metadata().size()) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                };
            let shared_hash = matches!(
                (l.metadata().hash(), r.metadata().hash()),
                (Some(a), Some(b)) if a.algorithm() == b.algorithm()
            );
            (same_size && !shared_hash).then(|| l.path().clone())
        })
        .collect()
}

fn case_sensitive(inventory: &Inventory) -> bool {
    inventory.capabilities().case_sensitivity == CaseSensitivity::Sensitive
}

/// Chemins distincts dont la forme repliée (minuscules Unicode simples)
/// coïncide. Approximation : la table de casse de NTFS et les
/// normalisations Unicode (NFC/NFD) ne sont pas reproduites.
fn case_collisions(paths: &BTreeSet<&ResourcePath>) -> BTreeMap<ResourcePath, Vec<ResourcePath>> {
    let mut groups: BTreeMap<String, Vec<&ResourcePath>> = BTreeMap::new();
    for path in paths {
        groups
            .entry(path.as_str().to_lowercase())
            .or_default()
            .push(path);
    }
    let mut collisions = BTreeMap::new();
    for members in groups.into_values().filter(|members| members.len() > 1) {
        for path in &members {
            let others = members
                .iter()
                .filter(|other| *other != path)
                .map(|other| (*other).clone())
                .collect();
            collisions.insert((*path).clone(), others);
        }
    }
    collisions
}
