//! Types partagés stables de SyncEngine.
//!
//! `sync-core` est la racine du graphe de dépendances : il ne dépend d'aucun
//! autre crate SyncEngine, ne fait aucune E/S et ne connaît ni l'interface
//! graphique ni les implémentations concrètes de connecteurs.
//!
//! Contenu de la phase 01 :
//! - identifiants typés ([`ProfileId`], [`ExecutionId`], [`OperationId`],
//!   [`ConnectorId`], [`EndpointId`]) ;
//! - chemin logique relatif à un connecteur ([`ResourcePath`]) ;
//! - horodatage UTC et précision déclarée ([`Timestamp`], [`TimestampPrecision`]) ;
//! - description d'une entrée inventoriée ([`FileEntry`], [`FileMetadata`], [`EntryKind`]) ;
//! - algorithmes de hachage nommés ([`HashAlgorithm`]) ;
//! - statut du cycle de vie d'une exécution ([`ExecutionStatus`]).
//!
//! Ajouts de la phase 02 :
//! - empreinte de contenu étiquetée ([`ContentHash`]), portée par [`FileMetadata`] ;
//! - jeton d'annulation coopérative ([`CancellationToken`]).
//!
//! Ajout de la phase 03 : identifiant de plan ([`PlanId`]).
//!
//! Le modèle de comparaison est défini par `sync-compare` (crate pur, sans
//! E/S) ; le modèle de plan (`ExecutionPlan`, opérations, préconditions,
//! politiques) par `sync-plan`, car il embarque les règles de filtre
//! (voir ADR 0003).

mod cancel;
mod entry;
mod execution;
mod hash;
mod ids;
mod path;
mod time;

pub use cancel::CancellationToken;
pub use entry::{EntryKind, FileEntry, FileMetadata};
pub use execution::ExecutionStatus;
pub use hash::{ContentHash, ContentHashError, HashAlgorithm};
pub use ids::{
    ConnectorId, EndpointId, ExecutionId, IdError, MAX_ID_LEN, OperationId, PlanId, ProfileId,
};
pub use path::{PathError, ResourcePath};
pub use time::{Timestamp, TimestampError, TimestampPrecision};
