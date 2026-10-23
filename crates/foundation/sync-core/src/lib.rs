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
//! Les types de comparaison, de conflit et de plan (`ComparisonOutcome`,
//! `Conflict`, `ExecutionPlan`…) sont introduits par les phases 02 et 03.

mod entry;
mod execution;
mod hash;
mod ids;
mod path;
mod time;

pub use entry::{EntryKind, FileEntry, FileMetadata};
pub use execution::ExecutionStatus;
pub use hash::HashAlgorithm;
pub use ids::{ConnectorId, EndpointId, ExecutionId, IdError, MAX_ID_LEN, OperationId, ProfileId};
pub use path::{PathError, ResourcePath};
pub use time::{Timestamp, TimestampError, TimestampPrecision};
