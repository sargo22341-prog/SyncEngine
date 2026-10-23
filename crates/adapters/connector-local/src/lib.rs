//! Connecteur du système de fichiers local, en lecture seule.
//!
//! [`LocalConnector`] implémente [`connector_api::StorageConnector`] pour un
//! répertoire racine absolu de l'OS (lettre de lecteur, chemin UNC ou chemin
//! POSIX). Phase 02 : listing, métadonnées et lecture en flux uniquement ;
//! **aucune** écriture, création ni suppression.
//!
//! # Sûreté
//! - les chemins logiques sont convertis segment par segment sous la racine ;
//!   les noms ambigus pour Win32 (`CON`, `a:b`, `fin.`) sont refusés ;
//! - liens symboliques et jonctions sont rapportés, jamais suivis, et un
//!   chemin qui en traverse un est refusé ;
//! - une racine absente est signalée `Disconnected`, jamais comme un
//!   répertoire vide ;
//! - un nom non UTF-8 est signalé par une anomalie de listing, jamais
//!   converti avec perte ni ignoré en silence.
//!
//! # Exemple
//! ```no_run
//! use connector_api::StorageConnector;
//! use connector_local::{LocalConnector, LocalOptions};
//! use sync_core::{CancellationToken, ResourcePath};
//!
//! let connector = LocalConnector::new("/srv/donnees", LocalOptions::new())?;
//! let page = connector.list(&ResourcePath::root(), None, &CancellationToken::new())?;
//! for entry in page.entries() {
//!     println!("{} {:?}", entry.path(), entry.kind());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod connector;
mod errors;
mod paths;

pub use connector::{LocalConfigError, LocalConnector, LocalOptions};

/// Identifiant du type de connecteur : `local`.
#[must_use]
pub fn connector_id() -> sync_core::ConnectorId {
    // Littéral constant conforme aux invariants de `ConnectorId`.
    sync_core::ConnectorId::new("local").unwrap_or_else(|_| unreachable!())
}
