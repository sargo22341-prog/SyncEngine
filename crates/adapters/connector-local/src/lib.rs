//! Connecteur du système de fichiers local.
//!
//! [`LocalConnector`] implémente [`connector_api::StorageConnector`] pour un
//! répertoire racine absolu de l'OS (lettre de lecteur, chemin UNC ou chemin
//! POSIX). Phase 02 : listing, métadonnées et lecture en flux. Phase 04 :
//! mutations élémentaires, **désactivées par défaut** et activées seulement
//! par [`LocalOptions::with_mutations`] (extrémité destination).
//!
//! # Sûreté
//! - les chemins logiques sont convertis segment par segment sous la racine ;
//!   les noms ambigus pour Win32 (`CON`, `a:b`, `fin.`) sont refusés ;
//! - liens symboliques et jonctions sont rapportés, jamais suivis, et un
//!   chemin qui en traverse un est refusé ;
//! - une racine absente est signalée `Disconnected`, jamais comme un
//!   répertoire vide ;
//! - un nom non UTF-8 est signalé par une anomalie de listing, jamais
//!   converti avec perte ni ignoré en silence ;
//! - écriture : fichier temporaire dans le dossier cible
//!   ([`is_staging_name`]), `fsync`, puis publication sans écrasement (lien
//!   physique) ou remplacement par renommage ; la date de modification
//!   demandée est appliquée avant publication ;
//! - suppression : fichier ou répertoire **vide** de la nature attendue,
//!   jamais un lien, jamais récursive.
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

mod confined;
mod connector;
mod errors;
mod paging;
mod paths;
mod write;

pub use connector::{LocalConfigError, LocalConnector, LocalOptions};

/// Vrai si `name` est un nom de fichier temporaire d'écriture de ce
/// connecteur (`.syncengine-….partial`) : un tel fichier, laissé par une
/// exécution interrompue, ne contient jamais une version validée.
#[must_use]
pub fn is_staging_name(name: &str) -> bool {
    name.starts_with(write::STAGING_PREFIX) && name.ends_with(write::STAGING_SUFFIX)
}

/// Identifiant du type de connecteur : `local`.
#[must_use]
pub fn connector_id() -> sync_core::ConnectorId {
    // Littéral constant conforme aux invariants de `ConnectorId`.
    sync_core::ConnectorId::new("local").unwrap_or_else(|_| unreachable!())
}
