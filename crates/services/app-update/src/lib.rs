//! Mise à jour applicative de SyncEngine depuis les releases GitHub.
//!
//! SyncEngine est portable : la mise à jour remplace les exécutables dans le
//! dossier du programme, sans installateur ni droits administrateur. Ce crate
//! ne contient aucune interface ; il ne décide jamais seul d'installer :
//! l'appelant déclenche chaque étape après un choix explicite de l'utilisateur.
//!
//! Étapes, chacune refusant au moindre doute sans rien modifier :
//!
//! 1. [`check`] : dernière release publiée (`releases/latest`, ni brouillon ni
//!    pré-version), comparaison semver avec la version compilée, présence de
//!    l'archive de la cible courante et de son empreinte SHA-256 publiée dans
//!    le texte de la release (bloc `### Empreintes SHA-256`).
//! 2. [`download`] : téléchargement en flux dans le dossier du programme
//!    (`syncengine-update.download`), progression et annulation par
//!    [`Progress`], puis vérification de l'empreinte. Un échec retire le
//!    fichier temporaire.
//! 3. [`stage`] : extraction stricte dans `syncengine-update.staging/` : seuls
//!    les cinq noms attendus sous la racine attendue sont acceptés ; tout
//!    autre élément, chemin absolu, `..`, lien ou fichier manquant rejette
//!    l'archive entière.
//! 4. [`install`] : chaque fichier actuel est renommé en `<nom>.old` puis
//!    remplacé par renommage du fichier extrait. Un échec restaure les `.old`
//!    ; un retour arrière incomplet est signalé
//!    ([`ErrorKind::RollbackIncomplete`]), jamais masqué. L'ensemble n'est pas
//!    atomique : le système ne le garantit pas pour plusieurs fichiers.
//! 5. [`cleanup`] au démarrage suivant : retire uniquement les noms exacts
//!    ci-dessus, sans jamais suivre un lien.
//!
//! L'arrêt et la relance de l'agent, ainsi que la relance de l'interface,
//! appartiennent à l'appelant (voir `apps/desktop/src/app_update.rs`).
//!
//! Limite : l'empreinte est publiée dans la même release que l'archive ; elle
//! protège d'un téléchargement corrompu ou tronqué, pas d'un compte GitHub
//! compromis (les archives ne sont pas signées).
//!
//! ```no_run
//! use app_update::{Check, Progress, Target, UpdateSource};
//! # fn main() -> Result<(), app_update::UpdateError> {
//! let target = Target::current().ok_or_else(app_update::unsupported_platform)?;
//! let source = UpdateSource::github()?;
//! if let Check::Available(update) = app_update::check(&source, env!("CARGO_PKG_VERSION"), target)? {
//!     let exe = std::env::current_exe().map_err(app_update::io_error)?;
//!     let dir = app_update::program_dir(&exe, target)?;
//!     let progress = Progress::default();
//!     let downloaded = app_update::download(&source, &update, &dir, &progress)?;
//!     let staged = app_update::stage(downloaded, &progress)?;
//!     // Arrêter ici l'agent, puis :
//!     let installed = app_update::install(staged)?;
//!     // Relancer `installed.desktop`, puis quitter l'ancienne interface.
//!     assert!(installed.desktop.is_absolute());
//! }
//! # Ok(())
//! # }
//! ```

mod archive;
mod error;
mod files;
mod http;
mod install;
mod release;
mod version;

pub use archive::{Staged, stage};
pub use error::{ErrorKind, UpdateError, io_error, unsupported_platform};
pub use files::{Cleanup, cleanup, program_dir};
pub use http::{Check, Downloaded, Progress, Update, UpdateSource, check, download};
pub use install::{Installed, install};
pub use release::{checksum_for, parse_checksums, release_notes};
pub use version::{ArchiveFormat, Target, is_newer};
