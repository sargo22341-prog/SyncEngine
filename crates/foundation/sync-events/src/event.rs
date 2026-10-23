//! Modèle d'événement.

use serde::{Deserialize, Serialize};
use sync_core::{ExecutionId, ExecutionStatus, ProfileId, Timestamp};
use sync_errors::ErrorReport;

/// Instantané agrégé de progression.
///
/// Un instantané résume l'avancement ; il n'est jamais émis par octet
/// transféré. Les totaux sont `None` tant qu'ils sont inconnus (scan en
/// cours), ce qui est distinct d'un total nul.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    /// Entrées traitées.
    pub entries_done: u64,
    /// Entrées prévues, si connues.
    pub entries_total: Option<u64>,
    /// Octets traités.
    pub bytes_done: u64,
    /// Octets prévus, si connus.
    pub bytes_total: Option<u64>,
}

/// Nature d'un événement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventKind {
    /// Changement de statut d'exécution.
    StatusChanged {
        /// Nouveau statut.
        status: ExecutionStatus,
    },
    /// Progression agrégée.
    Progress(ProgressSnapshot),
    /// Diagnostic (avertissement ou erreur) déjà caviardé.
    Diagnostic(ErrorReport),
    /// Des événements précédents ont été perdus faute de place.
    EventsDropped {
        /// Nombre d'événements perdus depuis le dernier signalement.
        count: u64,
    },
}

/// Événement daté et ordonné au sein d'une exécution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    execution_id: ExecutionId,
    profile_id: Option<ProfileId>,
    sequence: u64,
    emitted_at: Timestamp,
    kind: EventKind,
}

impl Event {
    /// Construit un événement.
    #[must_use]
    pub fn new(
        execution_id: ExecutionId,
        profile_id: Option<ProfileId>,
        sequence: u64,
        emitted_at: Timestamp,
        kind: EventKind,
    ) -> Self {
        Self {
            execution_id,
            profile_id,
            sequence,
            emitted_at,
            kind,
        }
    }

    /// Exécution émettrice.
    #[must_use]
    pub fn execution_id(&self) -> &ExecutionId {
        &self.execution_id
    }

    /// Profil concerné, si l'exécution est rattachée à un profil.
    #[must_use]
    pub fn profile_id(&self) -> Option<&ProfileId> {
        self.profile_id.as_ref()
    }

    /// Numéro d'ordre strictement croissant au sein d'une exécution ; un trou
    /// dans la séquence révèle une perte.
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Instant d'émission (horloge locale, informatif).
    #[must_use]
    pub fn emitted_at(&self) -> Timestamp {
        self.emitted_at
    }

    /// Nature de l'événement.
    #[must_use]
    pub fn kind(&self) -> &EventKind {
        &self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_with_tag() {
        let event = Event::new(
            ExecutionId::new("e1").unwrap(),
            Some(ProfileId::new("p1").unwrap()),
            3,
            Timestamp::new(10, 0).unwrap(),
            EventKind::StatusChanged {
                status: ExecutionStatus::Running,
            },
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"status_changed""#), "{json}");
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);

        let progress = EventKind::Progress(ProgressSnapshot {
            entries_done: 1,
            entries_total: None,
            bytes_done: 10,
            bytes_total: Some(100),
        });
        let json = serde_json::to_string(&progress).unwrap();
        assert_eq!(serde_json::from_str::<EventKind>(&json).unwrap(), progress);
    }
}
