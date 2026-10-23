//! Contrat commun des connecteurs de stockage.
//!
//! Un connecteur donne accès à **son** protocole ; il ne choisit jamais une
//! règle de sauvegarde, de miroir ou de synchronisation.
//!
//! # Périmètre
//! - phase 01 : identité et capacités ([`StorageConnector::capabilities`]) ;
//! - phase 02 : opérations sans mutation, synchrones et bloquantes
//!   ([`StorageConnector::list`] paginé, [`StorageConnector::stat`],
//!   [`StorageConnector::open_read`] en flux) et annulation coopérative
//!   (voir `docs/adr/0002-scan-et-comparaison.md`).
//!
//! Écritures, déplacements et suppressions ne font pas encore partie du
//! contrat. Aucune implémentation de connecteur n'est fournie ici.

mod capabilities;
mod connector;
mod error;
mod listing;

pub use capabilities::{
    CaseSensitivity, ClockSource, ListingConsistency, Pagination, StorageCapabilities,
    StorageFeature, SymlinkSupport,
};
pub use connector::{ContentReader, StorageConnector, ensure_supported};
pub use error::{StorageError, StorageErrorKind, StorageOperation};
pub use listing::{ListPage, ListingIssue, ListingIssueKind, NameIssue, PageToken};
