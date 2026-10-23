//! Stratégie miroir : rendre la destination conforme à la source.
//!
//! Sémantique :
//! - entrée absente de la destination : copiée ;
//! - fichier différent : par défaut [`OverwritePolicy::SourceWins`] — la
//!   source l'emporte, y compris sur une destination plus récente (motif
//!   `destination_newer` signalé dans le plan) ;
//!   [`OverwritePolicy::NewerSourceOnly`] donne un miroir prudent (conflit
//!   si la destination est plus récente) ; [`OverwritePolicy::KeepDestination`]
//!   est refusée (contraire au miroir) ;
//! - entrée présente seulement à la destination (absence **établie** à la
//!   source) : supprimée **seulement** si la politique l'autorise
//!   explicitement ([`DeletionPolicy::Enabled`]) ; sinon conservée. Les
//!   liens et entrées spéciales ne sont jamais supprimés (saut motivé).
//!
//! Les suppressions restent soumises aux garde-fous tout ou rien de
//! [`sync_decisions::plan_one_way`] : inventaire source ou destination
//! incomplet, source vide, plafonds ⇒ aucune suppression. Une source absente
//! (racine illisible, montage manquant) ne produit même pas d'inventaire :
//! aucun plan, donc aucune suppression.

use sync_core::EntryKind;
use sync_decisions::{
    Decision, OneWayMode, PlanStrategy, PlanningError, PlanningInput, PolicyViolation, plan_one_way,
};
use sync_plan::{
    Action, DeletionPolicy, EntrySnapshot, ExecutionPlan, OverwritePolicy, PlanPolicy, Reason,
    SyncMode,
};

/// Stratégie miroir (voir la documentation du crate).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MirrorStrategy;

impl MirrorStrategy {
    /// Stratégie miroir.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Politique par défaut : la source l'emporte ; **suppressions non
    /// autorisées** (elles doivent être activées explicitement).
    #[must_use]
    pub fn default_policy() -> PlanPolicy {
        PlanPolicy::new(OverwritePolicy::SourceWins, DeletionPolicy::Disabled)
    }
}

impl OneWayMode for MirrorStrategy {
    fn mode(&self) -> SyncMode {
        SyncMode::Mirror
    }

    fn check_policy(&self, policy: &PlanPolicy) -> Result<(), PolicyViolation> {
        match policy.overwrite {
            OverwritePolicy::SourceWins | OverwritePolicy::NewerSourceOnly => Ok(()),
            overwrite => Err(PolicyViolation::OverwriteNotSupported {
                mode: SyncMode::Mirror,
                overwrite,
            }),
        }
    }

    fn destination_only(&self, destination: &EntrySnapshot, policy: &PlanPolicy) -> Decision {
        match destination.kind() {
            EntryKind::File | EntryKind::Directory => match policy.deletion {
                DeletionPolicy::Enabled { .. } => {
                    Decision::new(Action::Delete, Reason::AbsentFromSource)
                }
                _ => Decision::new(Action::Keep, Reason::DeletionDisabled),
            },
            kind => Decision::new(Action::Skip, Reason::UnsupportedEntryKind { kind }),
        }
    }
}

impl PlanStrategy for MirrorStrategy {
    fn mode(&self) -> SyncMode {
        SyncMode::Mirror
    }

    fn build_plan(
        &self,
        input: &PlanningInput<'_>,
        policy: &PlanPolicy,
    ) -> Result<ExecutionPlan, PlanningError> {
        plan_one_way(self, input, policy)
    }
}
