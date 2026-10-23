//! Diffusion non bloquante des événements.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use sync_core::{ExecutionId, ProfileId, Timestamp};

use crate::event::{Event, EventKind};

/// Raison de la perte d'un événement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// File pleine : l'abonné consomme trop lentement.
    Full,
    /// Plus aucun abonné.
    Disconnected,
}

/// Résultat d'une émission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum EmitOutcome {
    /// Événement remis à la file.
    Delivered,
    /// Événement perdu ; la perte est comptée par le récepteur concerné.
    Dropped(DropReason),
}

/// Destination d'événements.
///
/// # Contrat
/// `emit` ne doit jamais bloquer ni paniquer. Un événement perdu doit être
/// signalé par [`EmitOutcome::Dropped`].
pub trait EventSink: Send + Sync {
    /// Remet un événement à la destination.
    fn emit(&self, event: Event) -> EmitOutcome;
}

/// Destination bornée adossée à `std::sync::mpsc::sync_channel`.
#[derive(Debug)]
pub struct BoundedEventSink {
    sender: SyncSender<Event>,
    dropped: AtomicU64,
}

/// Crée une destination bornée à `capacity` événements et son récepteur.
///
/// Une capacité nulle est relevée à 1 (une file de rendez-vous rejetterait
/// tout événement émis sans récepteur en attente).
#[must_use]
pub fn bounded_channel(capacity: usize) -> (BoundedEventSink, Receiver<Event>) {
    let (sender, receiver) = sync_channel(capacity.max(1));
    (
        BoundedEventSink {
            sender,
            dropped: AtomicU64::new(0),
        },
        receiver,
    )
}

impl BoundedEventSink {
    /// Nombre total d'événements perdus depuis la création.
    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl EventSink for BoundedEventSink {
    fn emit(&self, event: Event) -> EmitOutcome {
        match self.sender.try_send(event) {
            Ok(()) => EmitOutcome::Delivered,
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                EmitOutcome::Dropped(DropReason::Full)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                EmitOutcome::Dropped(DropReason::Disconnected)
            }
        }
    }
}

/// Émetteur lié à une exécution : numérote et date les événements.
///
/// Après une perte, l'émetteur insère un [`EventKind::EventsDropped`] dès que
/// la file accepte de nouveau un événement, afin que l'abonné sache que sa
/// vue est incomplète.
pub struct EventEmitter {
    execution_id: ExecutionId,
    profile_id: Option<ProfileId>,
    sink: Arc<dyn EventSink>,
    // Le verrou garantit l'ordre « numéro attribué = ordre de remise ».
    state: Mutex<EmitterState>,
}

#[derive(Default)]
struct EmitterState {
    next_sequence: u64,
    pending_dropped: u64,
}

impl EventEmitter {
    /// Crée un émetteur pour une exécution.
    #[must_use]
    pub fn new(
        execution_id: ExecutionId,
        profile_id: Option<ProfileId>,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            execution_id,
            profile_id,
            sink,
            state: Mutex::new(EmitterState::default()),
        }
    }

    /// Émet un événement daté de l'instant courant.
    ///
    /// Si l'horloge système est inutilisable, l'événement est daté de
    /// l'époque Unix plutôt que perdu : la date d'un événement est informative.
    pub fn emit(&self, kind: EventKind) -> EmitOutcome {
        let now = Timestamp::now().unwrap_or(Timestamp::UNIX_EPOCH);
        self.emit_at(kind, now)
    }

    /// Émet un événement daté de `emitted_at`.
    pub fn emit_at(&self, kind: EventKind, emitted_at: Timestamp) -> EmitOutcome {
        // Un verrou empoisonné ne contient qu'un compteur : il reste utilisable.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if state.pending_dropped > 0 {
            let notice = EventKind::EventsDropped {
                count: state.pending_dropped,
            };
            match self.deliver(&mut state, notice, emitted_at) {
                EmitOutcome::Delivered => state.pending_dropped = 0,
                dropped @ EmitOutcome::Dropped(_) => {
                    state.pending_dropped += 1;
                    return dropped;
                }
            }
        }
        let outcome = self.deliver(&mut state, kind, emitted_at);
        if matches!(outcome, EmitOutcome::Dropped(_)) {
            state.pending_dropped += 1;
        }
        outcome
    }

    fn deliver(&self, state: &mut EmitterState, kind: EventKind, at: Timestamp) -> EmitOutcome {
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        self.sink.emit(Event::new(
            self.execution_id.clone(),
            self.profile_id.clone(),
            sequence,
            at,
            kind,
        ))
    }
}

#[cfg(test)]
mod tests {
    use sync_core::ExecutionStatus;

    use super::*;
    use crate::event::ProgressSnapshot;

    fn emitter(sink: Arc<BoundedEventSink>) -> EventEmitter {
        EventEmitter::new(
            ExecutionId::new("exec-1").unwrap(),
            Some(ProfileId::new("profil").unwrap()),
            sink,
        )
    }

    fn progress(n: u64) -> EventKind {
        EventKind::Progress(ProgressSnapshot {
            entries_done: n,
            ..ProgressSnapshot::default()
        })
    }

    #[test]
    fn delivers_in_order_with_sequence() {
        let (sink, rx) = bounded_channel(8);
        let emitter = emitter(Arc::new(sink));
        assert_eq!(
            emitter.emit(EventKind::StatusChanged {
                status: ExecutionStatus::Running
            }),
            EmitOutcome::Delivered
        );
        assert_eq!(emitter.emit(progress(1)), EmitOutcome::Delivered);

        let events: Vec<Event> = rx.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence(), 0);
        assert_eq!(events[1].sequence(), 1);
        assert_eq!(events[0].execution_id().as_str(), "exec-1");
        assert_eq!(
            events[0].profile_id().map(ProfileId::as_str),
            Some("profil")
        );
    }

    #[test]
    fn full_queue_never_blocks_and_reports_loss() {
        let (sink, rx) = bounded_channel(2);
        let sink = Arc::new(sink);
        let emitter = emitter(Arc::clone(&sink));
        let at = Timestamp::UNIX_EPOCH;

        assert_eq!(emitter.emit_at(progress(1), at), EmitOutcome::Delivered);
        assert_eq!(emitter.emit_at(progress(2), at), EmitOutcome::Delivered);
        assert_eq!(
            emitter.emit_at(progress(3), at),
            EmitOutcome::Dropped(DropReason::Full)
        );
        assert_eq!(sink.dropped_count(), 1);

        // L'abonné rattrape son retard.
        assert_eq!(rx.try_iter().count(), 2);

        // L'émission suivante est précédée d'un avis de perte.
        assert_eq!(emitter.emit_at(progress(4), at), EmitOutcome::Delivered);
        let events: Vec<Event> = rx.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind(), &EventKind::EventsDropped { count: 1 });
        assert_eq!(events[1].kind(), &progress(4));
        // Le trou de séquence (2) révèle également la perte.
        assert_eq!(events[0].sequence(), 3);
        assert_eq!(events[1].sequence(), 4);
    }

    #[test]
    fn disconnected_receiver_is_reported() {
        let (sink, rx) = bounded_channel(1);
        drop(rx);
        let sink = Arc::new(sink);
        let emitter = emitter(Arc::clone(&sink));
        assert_eq!(
            emitter.emit(progress(1)),
            EmitOutcome::Dropped(DropReason::Disconnected)
        );
        assert_eq!(sink.dropped_count(), 1);
    }

    #[test]
    fn zero_capacity_is_raised_to_one() {
        let (sink, rx) = bounded_channel(0);
        let emitter = emitter(Arc::new(sink));
        assert_eq!(emitter.emit(progress(1)), EmitOutcome::Delivered);
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn emitter_is_shareable_across_threads() {
        let (sink, rx) = bounded_channel(64);
        let emitter = Arc::new(emitter(Arc::new(sink)));
        let handles: Vec<_> = (0..4)
            .map(|t| {
                let emitter = Arc::clone(&emitter);
                std::thread::spawn(move || {
                    for i in 0..8 {
                        let _outcome = emitter.emit(progress(t * 100 + i));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let mut sequences: Vec<u64> = rx.try_iter().map(|e| e.sequence()).collect();
        sequences.sort_unstable();
        assert_eq!(sequences, (0..32).collect::<Vec<_>>());
    }
}
