//! Modèle du plan d'exécution SyncEngine.
//!
//! Un [`ExecutionPlan`] est le **contrat** entre la planification (phase 03 :
//! `sync-decisions`, `sync-backup`, `sync-mirror`) et l'exécution
//! (phase 04) : la prévisualisation affiche exactement le plan qui sera
//! exécuté. Ce crate ne fait aucune E/S et ne décide de rien : il décrit,
//! sérialise et **valide**.
//!
//! # Contenu
//! - [`Operation`] : un chemin, une [`Action`] (`copy`, `replace`, `keep`,
//!   `delete`, `conflict`, `skip`), un [`Reason`], les observations de
//!   chaque côté ([`EntrySnapshot`]), et des champs dérivés : [`Risk`],
//!   [`Precondition`]s à revérifier avant mutation, dépendances, octets ;
//! - [`PlanPolicy`] : [`OverwritePolicy`] et [`DeletionPolicy`] avec ses
//!   garde-fous ([`DeletionGuard`]) ;
//! - forme JSON versionnée ([`PLAN_SCHEMA_VERSION`],
//!   [`ExecutionPlan::to_json`], [`ExecutionPlan::from_json`]).
//!
//! # Invariants de sûreté (vérifiés à la construction et au chargement)
//! - aucune suppression en mode sauvegarde ni sans politique explicite ;
//! - aucune suppression si l'un des inventaires est incomplet, si la source
//!   est vide (sauf autorisation) ou au-delà des plafonds ;
//! - une suppression de répertoire n'est jamais récursive : tout son contenu
//!   doit être supprimé par des opérations distinctes ;
//! - une copie n'a lieu que sous un répertoire disponible à la destination ;
//! - un remplacement respecte la politique d'écrasement.
//!
//! # Écart au contrat conceptuel
//! `CONTEXTE.md` place le plan dans `sync-core`. Il embarque ici les règles
//! de filtre (`sync-filter`), que `sync-core` ne peut pas connaître sans
//! inverser le graphe ; voir ADR 0003.

mod baseline;
mod format;
mod operation;
mod plan;
mod policy;
mod validate;

pub use baseline::{Baseline, BaselineError, StateBinding, same_content};
pub use format::PlanFormatError;
pub use operation::{
    Action, EntrySnapshot, Operation, OperationSpec, PlanSide, Precondition, Reason, Risk,
    UncertainCause,
};
pub use plan::{
    EndpointSnapshot, ExecutionPlan, PLAN_SCHEMA_VERSION, PlanHeader, PlanSummary, SyncMode,
};
pub use policy::{
    DEFAULT_MAX_DELETE_PERCENT, DeletionCensus, DeletionGuard, DeletionPolicy, DeletionRefusal,
    OverwritePolicy, PlanPolicy, PolicyError,
};
pub use validate::{DeletionBlock, PlanError};
