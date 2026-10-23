//! Décision par chemin, commune aux modes unidirectionnels.

use sync_compare::{ComparisonOutcome, Difference, PathComparison, Side, Uncertainty};
use sync_core::EntryKind;
use sync_plan::{Action, OverwritePolicy, PlanPolicy, PlanSide, Reason, UncertainCause};

/// Action et motif retenus pour un chemin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Action.
    pub action: Action,
    /// Motif.
    pub reason: Reason,
}

impl Decision {
    /// Construit une décision.
    #[must_use]
    pub fn new(action: Action, reason: Reason) -> Self {
        Self { action, reason }
    }
}

/// Résultat de la règle commune : décision prise, ou chemin présent
/// seulement à la destination (traitement propre au mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RowDecision {
    Decided(Decision),
    DestinationOnly,
}

fn plan_side(side: Side) -> PlanSide {
    match side {
        Side::Left => PlanSide::Source,
        Side::Right => PlanSide::Destination,
    }
}

/// Traduit une incertitude de comparaison (gauche = source).
pub(crate) fn uncertain_cause(uncertainty: &Uncertainty) -> UncertainCause {
    match uncertainty {
        Uncertainty::PresenceUnknown { side, .. } => UncertainCause::PresenceUnknown {
            side: plan_side(*side),
        },
        Uncertainty::ExcludedOnSide { side } => UncertainCause::Excluded {
            side: plan_side(*side),
        },
        Uncertainty::SizeUnknown => UncertainCause::SizeUnknown,
        Uncertainty::ModifiedUnknown => UncertainCause::ModifiedUnknown,
        Uncertainty::PrecisionUnknown => UncertainCause::PrecisionUnknown,
        Uncertainty::NotComparable { .. } => UncertainCause::NotComparable,
        Uncertainty::InconsistentObservations => UncertainCause::InconsistentObservations,
        Uncertainty::CaseCollision { .. } => UncertainCause::CaseCollision,
        _ => UncertainCause::Other,
    }
}

fn skip_uncertain(causes: Vec<UncertainCause>) -> Decision {
    let causes = if causes.is_empty() {
        vec![UncertainCause::Other]
    } else {
        causes
    };
    Decision::new(Action::Skip, Reason::Uncertain { causes })
}

fn is_file_or_dir(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::File | EntryKind::Directory)
}

/// Règle commune (gauche = source, droite = destination) :
/// - présent seulement à la source : copie (fichier, répertoire), sinon saut ;
/// - équivalents : conservation ;
/// - natures différentes : conflit ;
/// - fichiers différents : selon [`OverwritePolicy`] et la date la plus
///   récente ([`decide_overwrite`]) ;
/// - incertain : saut, **jamais** d'écriture ni de suppression ;
/// - présent seulement à la destination : renvoyé au mode.
pub(crate) fn decide(row: &PathComparison, policy: &PlanPolicy) -> RowDecision {
    let decision = match row.outcome() {
        ComparisonOutcome::LeftOnly => match row.left().map(sync_core::FileEntry::kind) {
            Some(kind) if is_file_or_dir(kind) => {
                Decision::new(Action::Copy, Reason::MissingAtDestination)
            }
            Some(kind) => Decision::new(Action::Skip, Reason::UnsupportedEntryKind { kind }),
            None => skip_uncertain(vec![UncertainCause::InconsistentObservations]),
        },
        ComparisonOutcome::RightOnly => return RowDecision::DestinationOnly,
        ComparisonOutcome::Equivalent(basis) => Decision::new(
            Action::Keep,
            Reason::Equivalent {
                verified: basis.is_content_verified(),
            },
        ),
        ComparisonOutcome::Different(differences) => decide_different(row, differences, policy),
        ComparisonOutcome::Uncertain(uncertainties) => {
            skip_uncertain(uncertainties.iter().map(uncertain_cause).collect())
        }
        _ => skip_uncertain(vec![UncertainCause::Other]),
    };
    RowDecision::Decided(decision)
}

fn decide_different(
    row: &PathComparison,
    differences: &[Difference],
    policy: &PlanPolicy,
) -> Decision {
    let kinds = row
        .left()
        .map(sync_core::FileEntry::kind)
        .zip(row.right().map(sync_core::FileEntry::kind));
    let Some((source, destination)) = kinds else {
        return skip_uncertain(vec![UncertainCause::InconsistentObservations]);
    };
    if source != destination {
        return Decision::new(
            Action::Conflict,
            Reason::KindMismatch {
                source,
                destination,
            },
        );
    }
    if source != EntryKind::File {
        return Decision::new(Action::Skip, Reason::UnsupportedEntryKind { kind: source });
    }
    let newer = differences.iter().find_map(|difference| match difference {
        Difference::Modified { newer } => Some(plan_side(*newer)),
        _ => None,
    });
    decide_overwrite(policy.overwrite, newer)
}

/// Décision pour deux fichiers différents, selon la politique et le côté
/// dont la date est la plus récente (`None` : dates égales à la précision
/// commune, ou inconnues alors que la taille ou le contenu diffère).
///
/// | politique \ plus récent | source      | destination              | aucun                   |
/// |-------------------------|-------------|--------------------------|-------------------------|
/// | `SourceWins`            | remplacer   | remplacer (signalé)      | remplacer (signalé)     |
/// | `NewerSourceOnly`       | remplacer   | conflit                  | conflit (ambigu)        |
/// | `KeepDestination`       | conserver   | conserver                | conserver               |
#[must_use]
pub fn decide_overwrite(policy: OverwritePolicy, newer: Option<PlanSide>) -> Decision {
    match (policy, newer) {
        (OverwritePolicy::KeepDestination, _) => {
            Decision::new(Action::Keep, Reason::OverwriteDisabled)
        }
        (
            OverwritePolicy::SourceWins | OverwritePolicy::NewerSourceOnly,
            Some(PlanSide::Source),
        ) => Decision::new(Action::Replace, Reason::SourceNewer),
        (OverwritePolicy::SourceWins, Some(PlanSide::Destination)) => {
            Decision::new(Action::Replace, Reason::DestinationNewer)
        }
        (OverwritePolicy::SourceWins, None) => {
            Decision::new(Action::Replace, Reason::ContentDiffers)
        }
        (_, Some(PlanSide::Destination)) => {
            Decision::new(Action::Conflict, Reason::DestinationNewer)
        }
        // `NewerSourceOnly` sans date exploitable, ou politique inconnue de
        // cette version : conflit, rien n'est écrit.
        (_, _) => Decision::new(Action::Conflict, Reason::AmbiguousChange),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrite_table() {
        use OverwritePolicy::{KeepDestination, NewerSourceOnly, SourceWins};
        use PlanSide::{Destination, Source};
        let cases = [
            (
                SourceWins,
                Some(Source),
                Action::Replace,
                Reason::SourceNewer,
            ),
            (
                SourceWins,
                Some(Destination),
                Action::Replace,
                Reason::DestinationNewer,
            ),
            (SourceWins, None, Action::Replace, Reason::ContentDiffers),
            (
                NewerSourceOnly,
                Some(Source),
                Action::Replace,
                Reason::SourceNewer,
            ),
            (
                NewerSourceOnly,
                Some(Destination),
                Action::Conflict,
                Reason::DestinationNewer,
            ),
            (
                NewerSourceOnly,
                None,
                Action::Conflict,
                Reason::AmbiguousChange,
            ),
            (
                KeepDestination,
                Some(Source),
                Action::Keep,
                Reason::OverwriteDisabled,
            ),
            (
                KeepDestination,
                None,
                Action::Keep,
                Reason::OverwriteDisabled,
            ),
        ];
        for (policy, newer, action, reason) in cases {
            assert_eq!(
                decide_overwrite(policy, newer),
                Decision::new(action, reason),
                "{policy:?} {newer:?}"
            );
        }
    }
}
