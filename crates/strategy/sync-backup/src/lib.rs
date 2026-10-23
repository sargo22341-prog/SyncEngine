//! Stratégie de sauvegarde (source → destination).
//!
//! Sémantique :
//! - entrée absente de la destination : copiée (répertoires créés) ;
//! - fichier différent : selon [`OverwritePolicy`] ; par défaut
//!   [`OverwritePolicy::NewerSourceOnly`] — remplacé seulement si la source
//!   est plus récente, sinon **conflit** (rien n'est écrit) ;
//! - entrée présente seulement à la destination : **conservée** ;
//! - **jamais de suppression** : une politique autorisant les suppressions
//!   est refusée ([`PolicyViolation::DeletionNotSupported`]).
//!
//! Les règles communes (incertitudes, parents, conflits de nature) sont
//! celles de [`sync_decisions::plan_one_way`].
//!
//! ```
//! # fn demo(report: &sync_compare::ComparisonReport, context: &sync_decisions::PlanContext)
//! # -> Result<(), sync_decisions::PlanningError> {
//! use sync_backup::BackupStrategy;
//! use sync_decisions::{PlanStrategy, PlanningInput};
//!
//! let strategy = BackupStrategy::new();
//! let plan = strategy.build_plan(
//!     &PlanningInput::new(report, context),
//!     &BackupStrategy::default_policy(),
//! )?;
//! assert_eq!(plan.summary().delete, 0);
//! # Ok(()) }
//! ```

use sync_decisions::{
    Decision, OneWayMode, PlanStrategy, PlanningError, PlanningInput, PolicyViolation, plan_one_way,
};
use sync_plan::{
    Action, DeletionPolicy, EntrySnapshot, ExecutionPlan, OverwritePolicy, PlanPolicy, Reason,
    SyncMode,
};

/// Stratégie de sauvegarde (voir la documentation du crate).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackupStrategy;

impl BackupStrategy {
    /// Stratégie de sauvegarde.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Politique par défaut : remplacement si la source est plus récente,
    /// aucune suppression.
    #[must_use]
    pub fn default_policy() -> PlanPolicy {
        PlanPolicy::new(OverwritePolicy::NewerSourceOnly, DeletionPolicy::Disabled)
    }
}

impl OneWayMode for BackupStrategy {
    fn mode(&self) -> SyncMode {
        SyncMode::Backup
    }

    fn check_policy(&self, policy: &PlanPolicy) -> Result<(), PolicyViolation> {
        match policy.deletion {
            DeletionPolicy::Disabled => Ok(()),
            _ => Err(PolicyViolation::DeletionNotSupported {
                mode: SyncMode::Backup,
            }),
        }
    }

    fn destination_only(&self, _destination: &EntrySnapshot, _policy: &PlanPolicy) -> Decision {
        Decision::new(Action::Keep, Reason::DestinationOnly)
    }
}

impl PlanStrategy for BackupStrategy {
    fn mode(&self) -> SyncMode {
        SyncMode::Backup
    }

    fn build_plan(
        &self,
        input: &PlanningInput<'_>,
        policy: &PlanPolicy,
    ) -> Result<ExecutionPlan, PlanningError> {
        plan_one_way(self, input, policy)
    }
}
