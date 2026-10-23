//! Événements informatifs SyncEngine.
//!
//! Les événements servent à l'affichage (GUI, CLI, agent) et à la
//! journalisation. Ils **ne pilotent aucune mutation** et ne sont pas une
//! source de vérité : un événement peut être perdu si un abonné est trop
//! lent. La perte est comptée et signalée, jamais silencieuse.
//!
//! # Choix synchrone
//! La diffusion utilise `std::sync::mpsc::sync_channel` en mode `try_send` :
//! émettre ne bloque jamais l'émetteur (ni un exécuteur async). Ce choix
//! n'impose pas `tokio` à ce crate ; un pont async pourra être ajouté par un
//! client sans modifier le contrat [`EventSink`].

mod event;
mod sink;

pub use event::{Event, EventKind, ProgressSnapshot};
pub use sink::{
    BoundedEventSink, DropReason, EmitOutcome, EventEmitter, EventSink, bounded_channel,
};
