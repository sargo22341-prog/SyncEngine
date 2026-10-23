//! Entrée de planification, contrat de stratégie et erreurs.

use sync_compare::ComparisonReport;
use sync_core::{PlanId, ProfileId, Timestamp};
use sync_errors::{ClassifiedError, ErrorClass};
use sync_plan::{ExecutionPlan, OverwritePolicy, PlanError, PlanPolicy, SyncMode};
use thiserror::Error;

/// Identification du plan à produire (fournie par l'appelant pour que la
/// planification reste pure et reproductible).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanContext {
    /// Identifiant du plan.
    pub plan_id: PlanId,
    /// Profil d'origine, s'il y en a un.
    pub profile: Option<ProfileId>,
    /// Instant de création (UTC).
    pub created_at: Timestamp,
    /// Libellé d'affichage de la source (jamais de secret).
    pub source_location: String,
    /// Libellé d'affichage de la destination (jamais de secret).
    pub destination_location: String,
}

/// Entrée d'une stratégie unidirectionnelle.
///
/// Convention : la **gauche** du rapport de comparaison est la **source**,
/// la **droite** est la **destination**.
#[derive(Debug, Clone, Copy)]
pub struct PlanningInput<'a> {
    report: &'a ComparisonReport,
    context: &'a PlanContext,
}

impl<'a> PlanningInput<'a> {
    /// Associe un rapport (gauche = source) et un contexte.
    #[must_use]
    pub fn new(report: &'a ComparisonReport, context: &'a PlanContext) -> Self {
        Self { report, context }
    }

    /// Rapport de comparaison.
    #[must_use]
    pub fn report(&self) -> &'a ComparisonReport {
        self.report
    }

    /// Contexte.
    #[must_use]
    pub fn context(&self) -> &'a PlanContext {
        self.context
    }
}

/// Politique incompatible avec le mode demandé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PolicyViolation {
    /// Le mode ne supprime jamais (sauvegarde).
    #[error("le mode {mode:?} ne supprime jamais : politique de suppression refusée")]
    DeletionNotSupported {
        /// Mode concerné.
        mode: SyncMode,
    },
    /// Politique d'écrasement contraire à la sémantique du mode.
    #[error("politique d'écrasement {overwrite:?} incompatible avec le mode {mode:?}")]
    OverwriteNotSupported {
        /// Mode concerné.
        mode: SyncMode,
        /// Politique refusée.
        overwrite: OverwritePolicy,
    },
}

/// Échec de planification.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PlanningError {
    /// Politique refusée par la stratégie.
    #[error("politique invalide : {0}")]
    InvalidPolicy(#[from] PolicyViolation),
    /// Le plan produit viole un invariant : défaut de la stratégie, jamais
    /// masqué (aucun plan n'est rendu).
    #[error("plan incohérent produit par la stratégie : {0}")]
    InconsistentPlan(#[source] PlanError),
}

impl ClassifiedError for PlanningError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::InvalidPolicy(_) => ErrorClass::InvalidInput,
            Self::InconsistentPlan(_) => ErrorClass::Internal,
        }
    }
}

/// Contrat d'une stratégie de planification (sauvegarde, miroir,
/// bidirectionnelle plus tard).
///
/// Une stratégie produit un plan **inspectable**, jamais de mutation. Les
/// constats incertains, les inventaires incomplets et les garde-fous se
/// traduisent par des opérations sautées et motivées, pas par une erreur.
pub trait PlanStrategy: Send + Sync {
    /// Mode produit.
    fn mode(&self) -> SyncMode;

    /// Construit le plan.
    ///
    /// # Errors
    /// [`PlanningError::InvalidPolicy`] si la politique est contraire au
    /// mode ; [`PlanningError::InconsistentPlan`] en cas de défaut interne.
    fn build_plan(
        &self,
        input: &PlanningInput<'_>,
        policy: &PlanPolicy,
    ) -> Result<ExecutionPlan, PlanningError>;
}
