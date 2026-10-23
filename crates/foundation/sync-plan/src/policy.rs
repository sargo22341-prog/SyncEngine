//! Politiques de décision et garde-fous de suppression, enregistrés dans le plan.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Traitement d'un fichier présent des deux côtés et différent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OverwritePolicy {
    /// La source l'emporte toujours : la destination est remplacée même si
    /// elle paraît plus récente (sémantique du miroir). Le motif de
    /// l'opération signale alors le risque.
    SourceWins,
    /// Remplacement seulement si la source est **strictement plus récente**
    /// à la précision commune. Destination plus récente, ou contenu différent
    /// sans date exploitable ⇒ conflit : rien n'est écrit.
    NewerSourceOnly,
    /// Jamais de remplacement : la destination existante est conservée.
    KeepDestination,
}

/// Erreur de construction d'une politique.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PolicyError {
    /// Pourcentage hors de `1..=100`.
    #[error("pourcentage de suppression {0} hors de 1..=100")]
    InvalidPercent(u8),
}

/// Garde-fous appliqués aux suppressions d'un plan.
///
/// Les vérifications sont **tout ou rien** : si l'une échoue, aucune
/// suppression du plan n'est retenue (elles deviennent des sauts motivés).
/// Valeur par défaut : au plus 50 % des entrées de la destination, aucune
/// suppression si la source est vide, pas de plafond absolu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawGuard", into = "RawGuard")]
pub struct DeletionGuard {
    max_count: Option<u64>,
    max_percent: Option<u8>,
    allow_empty_source: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuard {
    max_count: Option<u64>,
    max_percent: Option<u8>,
    allow_empty_source: bool,
}

impl TryFrom<RawGuard> for DeletionGuard {
    type Error = PolicyError;

    fn try_from(raw: RawGuard) -> Result<Self, Self::Error> {
        Self::new()
            .with_max_count(raw.max_count)
            .with_max_percent(raw.max_percent)
            .map(|guard| guard.with_empty_source_allowed(raw.allow_empty_source))
    }
}

impl From<DeletionGuard> for RawGuard {
    fn from(guard: DeletionGuard) -> Self {
        Self {
            max_count: guard.max_count,
            max_percent: guard.max_percent,
            allow_empty_source: guard.allow_empty_source,
        }
    }
}

impl Default for DeletionGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Pourcentage maximal par défaut des entrées de destination supprimables.
pub const DEFAULT_MAX_DELETE_PERCENT: u8 = 50;

impl DeletionGuard {
    /// Garde-fous par défaut (voir le type).
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_count: None,
            max_percent: Some(DEFAULT_MAX_DELETE_PERCENT),
            allow_empty_source: false,
        }
    }

    /// Plafond absolu de suppressions (`None` : pas de plafond).
    #[must_use]
    pub fn with_max_count(mut self, max_count: Option<u64>) -> Self {
        self.max_count = max_count;
        self
    }

    /// Plafond relatif, en pourcentage des entrées observées à la
    /// destination (`None` : pas de plafond).
    ///
    /// # Errors
    /// [`PolicyError::InvalidPercent`] hors de `1..=100`.
    pub fn with_max_percent(mut self, max_percent: Option<u8>) -> Result<Self, PolicyError> {
        if let Some(percent) = max_percent
            && !(1..=100).contains(&percent)
        {
            return Err(PolicyError::InvalidPercent(percent));
        }
        self.max_percent = max_percent;
        Ok(self)
    }

    /// Autorise des suppressions alors que la source ne contient aucune
    /// entrée. Désactivé par défaut : une source vide est le symptôme
    /// classique d'un montage absent ou d'un mauvais dossier.
    #[must_use]
    pub fn with_empty_source_allowed(mut self, allow: bool) -> Self {
        self.allow_empty_source = allow;
        self
    }

    /// Plafond absolu.
    #[must_use]
    pub fn max_count(&self) -> Option<u64> {
        self.max_count
    }

    /// Plafond relatif (pourcentage).
    #[must_use]
    pub fn max_percent(&self) -> Option<u8> {
        self.max_percent
    }

    /// Vrai si une source vide n'empêche pas les suppressions.
    #[must_use]
    pub fn allows_empty_source(&self) -> bool {
        self.allow_empty_source
    }

    /// Vérifie un ensemble de suppressions envisagées.
    ///
    /// Ordre des vérifications : complétude de la source, puis de la
    /// destination, source vide, plafond absolu, plafond relatif. Aucune
    /// suppression envisagée ⇒ toujours accepté.
    ///
    /// # Errors
    /// La première [`DeletionRefusal`] rencontrée.
    pub fn check(&self, census: &DeletionCensus) -> Result<(), DeletionRefusal> {
        if census.deletions == 0 {
            return Ok(());
        }
        if !census.source_complete {
            return Err(DeletionRefusal::SourceIncomplete);
        }
        if !census.destination_complete {
            return Err(DeletionRefusal::DestinationIncomplete);
        }
        if census.source_entries == 0 && !self.allow_empty_source {
            return Err(DeletionRefusal::SourceEmpty);
        }
        if let Some(limit) = self.max_count
            && census.deletions > limit
        {
            return Err(DeletionRefusal::TooMany {
                count: census.deletions,
                limit,
            });
        }
        if let Some(max_percent) = self.max_percent {
            // u128 : pas de débordement possible sur u64 × 100.
            let deleted = u128::from(census.deletions) * 100;
            let allowed = u128::from(census.destination_entries) * u128::from(max_percent);
            if deleted > allowed {
                return Err(DeletionRefusal::TooLarge {
                    count: census.deletions,
                    total: census.destination_entries,
                    max_percent,
                });
            }
        }
        Ok(())
    }
}

/// Décompte soumis aux garde-fous de suppression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionCensus {
    /// Suppressions envisagées.
    pub deletions: u64,
    /// Entrées observées à la destination.
    pub destination_entries: u64,
    /// Entrées observées à la source.
    pub source_entries: u64,
    /// Inventaire de la source complet.
    pub source_complete: bool,
    /// Inventaire de la destination complet.
    pub destination_complete: bool,
}

/// Motif du refus de l'ensemble des suppressions d'un plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum DeletionRefusal {
    /// L'inventaire de la source est incomplet (dossier illisible, scan
    /// partiel) : une absence n'y prouve rien.
    SourceIncomplete,
    /// L'inventaire de la destination est incomplet.
    DestinationIncomplete,
    /// La source ne contient aucune entrée (montage absent ? mauvais dossier ?).
    SourceEmpty,
    /// Plus de suppressions que le plafond absolu.
    TooMany {
        /// Suppressions envisagées.
        count: u64,
        /// Plafond.
        limit: u64,
    },
    /// Plus de suppressions que le plafond relatif.
    TooLarge {
        /// Suppressions envisagées.
        count: u64,
        /// Entrées observées à la destination.
        total: u64,
        /// Pourcentage maximal.
        max_percent: u8,
    },
}

/// Politique de suppression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum DeletionPolicy {
    /// Aucune suppression (valeur par défaut) : les entrées absentes de la
    /// source sont conservées.
    #[default]
    Disabled,
    /// Suppressions autorisées **explicitement**, sous garde-fous.
    Enabled {
        /// Garde-fous appliqués.
        guard: DeletionGuard,
    },
}

impl DeletionPolicy {
    /// Suppressions autorisées sous les garde-fous indiqués.
    #[must_use]
    pub fn enabled(guard: DeletionGuard) -> Self {
        Self::Enabled { guard }
    }

    /// Garde-fous, si les suppressions sont autorisées.
    #[must_use]
    pub fn guard(&self) -> Option<&DeletionGuard> {
        match self {
            Self::Disabled => None,
            Self::Enabled { guard } => Some(guard),
        }
    }
}

/// Politique complète utilisée pour produire un plan (enregistrée dans le
/// plan pour l'audit et l'affichage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPolicy {
    /// Traitement des fichiers différents.
    pub overwrite: OverwritePolicy,
    /// Traitement des entrées absentes de la source.
    pub deletion: DeletionPolicy,
}

impl PlanPolicy {
    /// Construit une politique.
    #[must_use]
    pub fn new(overwrite: OverwritePolicy, deletion: DeletionPolicy) -> Self {
        Self {
            overwrite,
            deletion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn census(deletions: u64, destination: u64, source: u64) -> DeletionCensus {
        DeletionCensus {
            deletions,
            destination_entries: destination,
            source_entries: source,
            source_complete: true,
            destination_complete: true,
        }
    }

    #[test]
    fn default_guard_limits_to_half() {
        let guard = DeletionGuard::new();
        assert_eq!(guard.check(&census(5, 10, 3)), Ok(()));
        assert_eq!(
            guard.check(&census(6, 10, 3)),
            Err(DeletionRefusal::TooLarge {
                count: 6,
                total: 10,
                max_percent: 50
            })
        );
        assert_eq!(guard.check(&census(0, 10, 0)), Ok(()));
    }

    #[test]
    fn incomplete_inventories_refuse_first() {
        let guard = DeletionGuard::new().with_max_percent(None).unwrap();
        let mut c = census(1, 10, 0);
        c.source_complete = false;
        c.destination_complete = false;
        assert_eq!(guard.check(&c), Err(DeletionRefusal::SourceIncomplete));
        c.source_complete = true;
        assert_eq!(guard.check(&c), Err(DeletionRefusal::DestinationIncomplete));
        c.destination_complete = true;
        assert_eq!(guard.check(&c), Err(DeletionRefusal::SourceEmpty));
        let guard = guard.with_empty_source_allowed(true);
        assert_eq!(guard.check(&c), Ok(()));
    }

    #[test]
    fn absolute_limit() {
        let guard = DeletionGuard::new()
            .with_max_count(Some(2))
            .with_max_percent(None)
            .unwrap();
        assert_eq!(guard.check(&census(2, 3, 1)), Ok(()));
        assert_eq!(
            guard.check(&census(3, 3, 1)),
            Err(DeletionRefusal::TooMany { count: 3, limit: 2 })
        );
    }

    #[test]
    fn percent_is_validated() {
        assert_eq!(
            DeletionGuard::new().with_max_percent(Some(0)),
            Err(PolicyError::InvalidPercent(0))
        );
        assert_eq!(
            DeletionGuard::new().with_max_percent(Some(101)),
            Err(PolicyError::InvalidPercent(101))
        );
        assert!(DeletionGuard::new().with_max_percent(Some(100)).is_ok());
        let json = r#"{"max_count":null,"max_percent":0,"allow_empty_source":false}"#;
        assert!(serde_json::from_str::<DeletionGuard>(json).is_err());
    }

    #[test]
    fn policy_serialization_is_stable() {
        let policy = PlanPolicy::new(
            OverwritePolicy::NewerSourceOnly,
            DeletionPolicy::enabled(DeletionGuard::new()),
        );
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(
            json,
            r#"{"overwrite":"newer_source_only","deletion":{"mode":"enabled","guard":{"max_count":null,"max_percent":50,"allow_empty_source":false}}}"#
        );
        assert_eq!(serde_json::from_str::<PlanPolicy>(&json).unwrap(), policy);
        assert_eq!(DeletionPolicy::default(), DeletionPolicy::Disabled);
        assert!(DeletionPolicy::Disabled.guard().is_none());
    }
}
