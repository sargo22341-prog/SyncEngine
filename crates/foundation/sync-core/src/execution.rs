//! Statut du cycle de vie d'une exécution.

use serde::{Deserialize, Serialize};

/// Statut d'une exécution de profil.
///
/// Ce type décrit un état observé ; il n'autorise ni ne déclenche aucune
/// mutation. Le modèle complet de rapport (`ExecutionReport`) arrive en
/// phase 04.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExecutionStatus {
    /// Créée, pas encore démarrée.
    Pending,
    /// En cours.
    Running,
    /// Terminée sans erreur ni avertissement.
    Succeeded,
    /// Terminée ; des avertissements non fatals ont été émis.
    SucceededWithWarnings,
    /// Terminée en échec ; l'état partiel doit être consulté.
    Failed,
    /// Annulée à la demande ; l'état partiel doit être consulté.
    Cancelled,
    /// Interrompue sans fin propre (arrêt du processus, coupure…).
    Interrupted,
}

impl ExecutionStatus {
    /// Vrai si l'exécution ne changera plus de statut.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }

    /// Vrai si l'exécution a pu laisser un état partiel à examiner.
    #[must_use]
    pub fn may_be_partial(self) -> bool {
        matches!(self, Self::Failed | Self::Cancelled | Self::Interrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_partial_states() {
        assert!(!ExecutionStatus::Pending.is_terminal());
        assert!(!ExecutionStatus::Running.is_terminal());
        assert!(ExecutionStatus::Succeeded.is_terminal());
        assert!(!ExecutionStatus::SucceededWithWarnings.may_be_partial());
        assert!(ExecutionStatus::Interrupted.is_terminal());
        assert!(ExecutionStatus::Interrupted.may_be_partial());
        assert!(ExecutionStatus::Cancelled.may_be_partial());
    }

    #[test]
    fn serde_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&ExecutionStatus::SucceededWithWarnings).unwrap(),
            "\"succeeded_with_warnings\""
        );
    }
}
