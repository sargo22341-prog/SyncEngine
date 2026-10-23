//! Classification structurée des erreurs SyncEngine.
//!
//! Chaque crate garde ses propres types d'erreur (`thiserror` recommandé) et
//! implémente [`ClassifiedError`] pour les rattacher à une [`ErrorClass`]
//! stable. Un [`ErrorReport`] est l'instantané sérialisable, caviardé, destiné
//! à l'interface, aux journaux et aux événements ; il n'est jamais une source
//! de vérité pour décider d'une mutation.

mod class;
mod context;
mod redact;
mod report;

pub use class::{ErrorClass, Severity};
pub use context::ErrorContext;
pub use redact::{REDACTED, redact_secrets};
pub use report::{ClassifiedError, ErrorReport, MAX_REPORTED_CAUSES};
