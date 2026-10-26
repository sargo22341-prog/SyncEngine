//! Erreurs de l'exécuteur.

use connector_api::{StorageError, StorageFeature};
use sync_core::{ConnectorId, EntryKind};
use sync_errors::{ClassifiedError, ErrorClass, ErrorContext};
use sync_journal::JournalError;
use sync_plan::PlanSide;
use sync_verify::Mismatch;
use thiserror::Error;

use crate::report::Refusal;

/// Refus d'exécuter un plan, constaté **avant toute mutation**.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutionError {
    /// Le plan est trop ancien : il doit être reconstruit.
    #[error("plan expiré : créé il y a {age_secs} s (au plus {max_secs} s) ; reconstruire le plan")]
    PlanExpired {
        /// Âge du plan, en secondes.
        age_secs: u64,
        /// Âge maximal admis, en secondes.
        max_secs: u64,
    },
    /// La date de création du plan est dans le futur (horloge incohérente).
    #[error("plan daté du futur : horloge incohérente, plan refusé")]
    PlanFromFuture,
    /// Le connecteur fourni n'est pas celui que le plan désigne.
    #[error("connecteur {found} fourni pour {side:?}, le plan attend {expected}")]
    ConnectorMismatch {
        /// Côté concerné.
        side: PlanSide,
        /// Connecteur du plan.
        expected: ConnectorId,
        /// Connecteur fourni.
        found: ConnectorId,
    },
    /// Le plan contient des suppressions non confirmées explicitement.
    #[error("le plan prévoit {count} suppression(s) : confirmation explicite requise")]
    DeletionsNotConfirmed {
        /// Nombre de suppressions prévues.
        count: u64,
    },
    /// Une capacité nécessaire au plan manque à un connecteur.
    #[error("capacité {feature:?} absente côté {side:?}")]
    CapabilityMissing {
        /// Côté concerné.
        side: PlanSide,
        /// Capacité manquante.
        feature: StorageFeature,
    },
    /// Une extrémité est indisponible (racine illisible, montage absent).
    #[error("extrémité {side:?} indisponible : {error}")]
    EndpointUnavailable {
        /// Côté concerné.
        side: PlanSide,
        /// Erreur du stockage.
        #[source]
        error: StorageError,
    },
    /// La racine d'une extrémité n'est pas un répertoire.
    #[error("la racine {side:?} n'est pas un répertoire")]
    EndpointNotDirectory {
        /// Côté concerné.
        side: PlanSide,
    },
    /// Le journal n'a pas pu consigner le début de l'exécution.
    #[error("journal indisponible : {0}")]
    Journal(#[source] JournalError),
    /// Annulation demandée avant le début de l'exécution.
    #[error("annulé avant le début de l'exécution")]
    Cancelled,
}

impl ClassifiedError for ExecutionError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::PlanExpired { .. } | Self::PlanFromFuture | Self::ConnectorMismatch { .. } => {
                ErrorClass::InvalidInput
            }
            Self::DeletionsNotConfirmed { .. } => ErrorClass::Conflict,
            Self::CapabilityMissing { .. } => ErrorClass::NotSupported,
            Self::EndpointUnavailable { error, .. } => error.kind().class(),
            Self::EndpointNotDirectory { .. } => ErrorClass::InvalidInput,
            Self::Journal(error) => error.class(),
            Self::Cancelled => ErrorClass::Cancelled,
        }
    }

    fn context(&self) -> Option<&ErrorContext> {
        match self {
            Self::EndpointUnavailable { error, .. } => error.context(),
            _ => None,
        }
    }
}

/// Défaut constaté par l'exécuteur lui-même (sans erreur de stockage).
#[derive(Debug, Error)]
pub(crate) enum Fault {
    #[error("un worker de copie s'est arrêté de manière inattendue")]
    WorkerFailed,
    /// Le dépôt relu ne correspond pas au contenu lu à la source : rien
    /// n'est validé.
    #[error("contenu déposé non conforme, rien n'est validé : {0}")]
    StagedContent(Mismatch),
    /// Le fichier publié ne correspond pas à ce qui a été transféré.
    #[error("fichier publié non conforme : {0}")]
    PublishedEntry(Mismatch),
    /// Nature d'entrée que l'exécuteur ne sait pas traiter.
    #[error("nature d'entrée non prise en charge : {0:?}")]
    UnsupportedKind(EntryKind),
    /// Observation attendue absente du plan (plan incohérent).
    #[error("observation manquante dans le plan")]
    MissingObservation,
}

impl ClassifiedError for Fault {
    fn class(&self) -> ErrorClass {
        match self {
            Self::StagedContent(_) | Self::PublishedEntry(_) => ErrorClass::Integrity,
            Self::UnsupportedKind(_) => ErrorClass::NotSupported,
            Self::MissingObservation | Self::WorkerFailed => ErrorClass::Internal,
        }
    }
}

/// Refus présenté comme un diagnostic (avertissement).
#[derive(Debug, Error)]
#[error("mutation refusée : {0}")]
pub(crate) struct RefusalNotice(pub(crate) Refusal);

impl ClassifiedError for RefusalNotice {
    fn class(&self) -> ErrorClass {
        ErrorClass::Conflict
    }
}
