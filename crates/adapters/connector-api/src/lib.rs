//! Contrat commun des connecteurs de stockage.
//!
//! Un connecteur donne accès à **son** protocole ; il ne choisit jamais une
//! règle de sauvegarde, de miroir ou de synchronisation.
//!
//! # Périmètre de la phase 01
//! Le trait [`StorageConnector`] n'expose que l'identité et les capacités.
//! Les opérations d'E/S (`list` paginé, `stat`, `read`/`write` en flux,
//! `mkdir`, `move`, `delete`, annulation) et leur modèle sync/async sont
//! fixés en phase 02 (voir `docs/CONTRATS.md`). Aucune implémentation de
//! connecteur n'est fournie ici.

mod capabilities;
mod connector;
mod error;

pub use capabilities::{
    CaseSensitivity, ClockSource, ListingConsistency, Pagination, StorageCapabilities,
    StorageFeature, SymlinkSupport,
};
pub use connector::{StorageConnector, ensure_supported};
pub use error::{StorageError, StorageErrorKind, StorageOperation};
