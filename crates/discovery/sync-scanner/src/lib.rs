//! Scanner d'inventaire.
//!
//! [`scan`] parcourt une extrémité au travers du contrat
//! [`connector_api::StorageConnector`] (jamais d'une implémentation
//! concrète), applique un [`sync_filter::Filter`] et produit un
//! [`Inventory`] : entrées observées, répertoires listés complètement ou
//! non, anomalies non fatales et, sur demande, empreintes BLAKE3.
//!
//! # Garanties
//! - **aucune mutation** : le scanner n'appelle que `list`, `stat` et
//!   `open_read` ;
//! - une erreur de listing n'est jamais confondue avec un répertoire vide :
//!   racine illisible ⇒ [`ScanError::RootUnavailable`], sous-répertoire
//!   illisible ⇒ [`DirectoryState::Incomplete`] et
//!   [`Inventory::presence`] répond [`Presence::Unknown`] sous ce répertoire ;
//! - les liens symboliques sont inventoriés, jamais suivis ;
//! - l'annulation conserve l'inventaire partiel ([`ScanError::Cancelled`]).
//!
//! ```
//! # fn demo(connector: &dyn connector_api::StorageConnector) -> Result<(), sync_scanner::ScanError> {
//! use sync_core::CancellationToken;
//! use sync_filter::Filter;
//! use sync_scanner::{scan, ScanOptions};
//!
//! let inventory = scan(connector, &Filter::include_all(), &ScanOptions::new(), &CancellationToken::new())?;
//! if !inventory.is_complete() {
//!     // Certaines absences sont inconnues : aucune suppression ne doit en découler.
//! }
//! # Ok(()) }
//! ```

mod inventory;
mod scan;

pub use inventory::{
    DirectoryState, IncompleteReason, Inventory, Presence, ScanIssue, ScanIssueKind, ScanStats,
    UnknownReason,
};
pub use scan::{ScanError, ScanOptions, scan, scan_observed};
