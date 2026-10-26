//! Rapport d'exécution.

use std::fmt;

use serde::{Deserialize, Serialize};
use sync_core::{ExecutionId, ExecutionStatus, OperationId, PlanId, ProfileId, ResourcePath};
use sync_errors::{ErrorClass, ErrorReport};
use sync_journal::{JournalTotals, OutcomeCode};
use sync_plan::{Action, PlanSide};
use sync_verify::Mismatch;

/// Raison d'un refus : une précondition n'est plus vraie au moment de la
/// mutation. Rien n'a été modifié pour cette opération.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Refusal {
    /// La source ne correspond plus à l'observation du plan.
    SourceChanged {
        /// Écart constaté.
        mismatch: Mismatch,
    },
    /// La source a changé pendant le transfert (taille, empreinte ou date).
    SourceChangedDuringTransfer {
        /// Écart constaté.
        mismatch: Mismatch,
    },
    /// La source a disparu depuis le plan (copie ou remplacement).
    SourceVanished,
    /// Une entrée est réapparue à la source : la suppression n'a plus de
    /// fondement.
    SourceReappeared,
    /// L'absence à la source ne peut pas être revérifiée (erreur de lecture) :
    /// une suppression ne se fonde jamais sur une erreur.
    SourceUnverifiable {
        /// Classe de l'erreur rencontrée.
        class: ErrorClass,
    },
    /// Une entrée est apparue à la destination : rien n'est écrasé.
    DestinationAppeared,
    /// La destination ne correspond plus à l'observation du plan.
    DestinationChanged {
        /// Écart constaté.
        mismatch: Mismatch,
    },
    /// L'entrée de destination à remplacer ou supprimer a disparu.
    DestinationVanished,
    /// Le répertoire à supprimer n'est pas vide (contenu non planifié, par
    /// exemple exclu par le filtre) : jamais de suppression récursive.
    DirectoryNotEmpty,
    /// Une opération dont celle-ci dépend n'a pas abouti.
    DependencyNotDone {
        /// Opération en cause.
        operation: OperationId,
    },
    /// Suppressions bloquées : la source est devenue indisponible ou
    /// illisible pendant l'exécution.
    DeletionsBlocked,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceChanged { mismatch } => write!(f, "source modifiée ({mismatch})"),
            Self::SourceChangedDuringTransfer { mismatch } => {
                write!(f, "source modifiée pendant le transfert ({mismatch})")
            }
            Self::SourceVanished => f.write_str("source disparue"),
            Self::SourceReappeared => f.write_str("entrée réapparue à la source"),
            Self::SourceUnverifiable { class } => {
                write!(f, "absence à la source non vérifiable ({})", class.code())
            }
            Self::DestinationAppeared => f.write_str("entrée apparue à la destination"),
            Self::DestinationChanged { mismatch } => {
                write!(f, "destination modifiée ({mismatch})")
            }
            Self::DestinationVanished => f.write_str("entrée de destination disparue"),
            Self::DirectoryNotEmpty => f.write_str("répertoire non vide"),
            Self::DependencyNotDone { operation } => {
                write!(f, "dépendance {operation} non réalisée")
            }
            Self::DeletionsBlocked => f.write_str("suppressions bloquées (source indisponible)"),
        }
    }
}

/// Cause d'arrêt anticipé d'une exécution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopCause {
    /// A transfer worker failed unexpectedly; all workers have been drained.
    WorkerFailure,
    /// Annulation demandée.
    Cancelled,
    /// Espace disque ou quota insuffisant à la destination.
    InsufficientSpace,
    /// Source déconnectée ou illisible.
    SourceUnavailable,
    /// Destination déconnectée.
    DestinationUnavailable,
    /// Le journal ne peut plus être écrit : aucune mutation non tracée.
    JournalFailure,
}

/// Issue d'une opération.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum OutcomeStatus {
    /// Mutation réalisée et vérifiée.
    Done {
        /// Octets transférés et validés.
        bytes: u64,
        /// Version conservée avant remplacement ou suppression.
        version: Option<ResourcePath>,
    },
    /// Aucune mutation prévue (conservation, conflit, saut).
    NoAction,
    /// Refus : une précondition n'est plus vraie, rien n'est modifié.
    Refused {
        /// Raison.
        refusal: Refusal,
    },
    /// Échec ; la cible n'a pas été validée (sauf mention contraire du
    /// diagnostic, par exemple fichier publié non conforme).
    Failed {
        /// Diagnostic caviardé.
        error: ErrorReport,
    },
    /// Interrompue par l'annulation ; la cible n'a pas été validée.
    Cancelled,
    /// Non commencée : l'exécution s'est arrêtée avant.
    NotStarted,
}

impl OutcomeStatus {
    /// Code consigné au journal.
    #[must_use]
    pub fn code(&self) -> OutcomeCode {
        match self {
            Self::Done { .. } => OutcomeCode::Done,
            Self::NoAction => OutcomeCode::NoAction,
            Self::Refused { .. } => OutcomeCode::Refused,
            Self::Failed { .. } => OutcomeCode::Failed,
            Self::Cancelled => OutcomeCode::Cancelled,
            Self::NotStarted => OutcomeCode::NotStarted,
        }
    }
}

/// Issue d'une opération du plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationOutcome {
    #[serde(default = "legacy_target")]
    pub(crate) target: PlanSide,
    pub(crate) operation: OperationId,
    pub(crate) path: ResourcePath,
    pub(crate) action: Action,
    pub(crate) status: OutcomeStatus,
}

impl OperationOutcome {
    /// Physical receiving side of this operation.
    #[must_use]
    pub fn target(&self) -> PlanSide {
        self.target
    }

    /// Opération du plan.
    #[must_use]
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    /// Chemin.
    #[must_use]
    pub fn path(&self) -> &ResourcePath {
        &self.path
    }

    /// Action prévue.
    #[must_use]
    pub fn action(&self) -> Action {
        self.action
    }

    /// Issue.
    #[must_use]
    pub fn status(&self) -> &OutcomeStatus {
        &self.status
    }
}

/// Rapport d'une exécution, y compris partielle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReport {
    pub(crate) execution: ExecutionId,
    pub(crate) plan: PlanId,
    pub(crate) profile: Option<ProfileId>,
    pub(crate) status: ExecutionStatus,
    pub(crate) stop: Option<StopCause>,
    pub(crate) totals: JournalTotals,
    pub(crate) outcomes: Vec<OperationOutcome>,
    pub(crate) leftovers: Vec<ResourcePath>,
}

impl ExecutionReport {
    /// Exécution.
    #[must_use]
    pub fn execution(&self) -> &ExecutionId {
        &self.execution
    }

    /// Plan exécuté.
    #[must_use]
    pub fn plan(&self) -> &PlanId {
        &self.plan
    }

    /// Profil, s'il y en a un.
    #[must_use]
    pub fn profile(&self) -> Option<&ProfileId> {
        self.profile.as_ref()
    }

    /// Statut final : `succeeded`, `succeeded_with_warnings` (refus,
    /// conflits ou chemins ignorés par le plan), `failed`, `cancelled`.
    #[must_use]
    pub fn status(&self) -> ExecutionStatus {
        self.status
    }

    /// Cause d'arrêt anticipé, le cas échéant.
    #[must_use]
    pub fn stop(&self) -> Option<StopCause> {
        self.stop
    }

    /// Décomptes.
    #[must_use]
    pub fn totals(&self) -> &JournalTotals {
        &self.totals
    }

    /// Issue de chaque opération, dans l'ordre du plan.
    #[must_use]
    pub fn outcomes(&self) -> &[OperationOutcome] {
        &self.outcomes
    }

    /// Fichiers temporaires que l'exécuteur n'a pas pu retirer de la
    /// destination (normalement vide ; jamais une version validée).
    #[must_use]
    pub fn leftovers(&self) -> &[ResourcePath] {
        &self.leftovers
    }

    /// Issue de l'opération portant sur `path`.
    #[must_use]
    pub fn outcome(&self, path: &ResourcePath) -> Option<&OperationOutcome> {
        self.outcomes.iter().find(|outcome| &outcome.path == path)
    }
}

fn legacy_target() -> PlanSide {
    PlanSide::Destination
}
