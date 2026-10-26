//! Planification unidirectionnelle (source → destination) et garde-fous.

use sync_core::{EntryKind, ResourcePath};
use sync_plan::{
    Action, DeletionCensus, EndpointSnapshot, EntrySnapshot, ExecutionPlan, OperationSpec,
    PlanHeader, PlanPolicy, Reason, SyncMode, UncertainCause,
};

use crate::input::{PlanningError, PlanningInput, PolicyViolation};
use crate::rules::{Decision, RowDecision, decide};

/// Règles propres à un mode unidirectionnel. La règle commune (copie,
/// remplacement, conflit, incertitude) est appliquée par [`plan_one_way`] ;
/// le mode ne décide que du sort des entrées présentes **seulement** à la
/// destination et des politiques qu'il accepte.
pub trait OneWayMode: Send + Sync {
    /// Mode produit.
    fn mode(&self) -> SyncMode;

    /// Refuse une politique contraire au mode.
    ///
    /// # Errors
    /// [`PolicyViolation`] décrivant l'incompatibilité.
    fn check_policy(&self, policy: &PlanPolicy) -> Result<(), PolicyViolation>;

    /// Décision pour une entrée présente seulement à la destination (absence
    /// à la source **établie** par la comparaison). Une suppression retournée
    /// ici reste soumise aux garde-fous de [`plan_one_way`].
    fn destination_only(&self, destination: &EntrySnapshot, policy: &PlanPolicy) -> Decision;
}

/// Construit un plan unidirectionnel.
///
/// Étapes :
/// 1. politique vérifiée par le mode ;
/// 2. une décision par constat de comparaison (gauche = source) ;
/// 3. copie ou remplacement dont le répertoire parent ne sera pas disponible
///    à la destination ⇒ saut (`ParentUnavailable`) ;
/// 4. suppression d'un répertoire dont tout le contenu n'est pas supprimé ⇒
///    saut (`DirectoryNotEmptied`) — jamais de suppression récursive ;
/// 5. garde-fous de suppression **tout ou rien** : inventaire source ou
///    destination incomplet, source vide, plafonds ⇒ toutes les suppressions
///    deviennent des sauts motivés (`DeletionRefused`) ;
/// 6. construction et validation du plan ([`ExecutionPlan::new`]).
///
/// # Errors
/// [`PlanningError::InvalidPolicy`] ; [`PlanningError::InconsistentPlan`]
/// si le mode produit une décision inadmissible.
pub fn plan_one_way(
    mode: &dyn OneWayMode,
    input: &PlanningInput<'_>,
    policy: &PlanPolicy,
) -> Result<ExecutionPlan, PlanningError> {
    mode.check_policy(policy)?;
    let report = input.report();
    let mut specs: Vec<OperationSpec> = report
        .rows()
        .iter()
        .map(|row| {
            let source = row.left().map(|entry| {
                EntrySnapshot::from(entry).with_origin_paths(row.source_origins().to_vec())
            });
            let destination = row.right().map(EntrySnapshot::from);
            let decision = match (decide(row, policy), &destination) {
                (RowDecision::Decided(decision), _) => decision,
                (RowDecision::DestinationOnly, Some(observed)) => {
                    mode.destination_only(observed, policy)
                }
                (RowDecision::DestinationOnly, None) => Decision::new(
                    Action::Skip,
                    Reason::Uncertain {
                        causes: vec![UncertainCause::InconsistentObservations],
                    },
                ),
            };
            OperationSpec {
                path: row.path().clone(),
                action: decision.action,
                reason: decision.reason,
                source,
                destination,
            }
        })
        .collect();
    // Les constats sont triés par chemin : un parent précède ses descendants.
    skip_copies_without_parent(&mut specs);
    skip_unemptied_directory_deletions(&mut specs);
    apply_deletion_guard(
        &mut specs,
        policy,
        report.left_complete(),
        report.right_complete(),
    );

    let context = input.context();
    let header = PlanHeader {
        id: context.plan_id.clone(),
        profile: context.profile.clone(),
        mode: mode.mode(),
        created_at: context.created_at,
        source: EndpointSnapshot::new(
            report.left_connector().clone(),
            context.source_location.clone(),
            report.left_complete(),
        ),
        destination: EndpointSnapshot::new(
            report.right_connector().clone(),
            context.destination_location.clone(),
            report.right_complete(),
        ),
        filter: report.filter_rules().clone(),
        policy: *policy,
    };
    ExecutionPlan::new(header, specs).map_err(PlanningError::InconsistentPlan)
}

fn index_of(specs: &[OperationSpec], path: &ResourcePath) -> Option<usize> {
    specs.binary_search_by(|spec| spec.path.cmp(path)).ok()
}

/// Même règle que la validation du plan : le répertoire existera-t-il à la
/// destination ?
fn directory_available(specs: &[OperationSpec], path: &ResourcePath) -> bool {
    if path.is_root() {
        return true;
    }
    let Some(spec) = index_of(specs, path).and_then(|index| specs.get(index)) else {
        return false;
    };
    match spec.action {
        Action::Copy => spec
            .source
            .as_ref()
            .is_some_and(|s| s.kind() == EntryKind::Directory),
        Action::Delete => false,
        _ => spec
            .destination
            .as_ref()
            .is_some_and(|d| d.kind() == EntryKind::Directory),
    }
}

fn skip_copies_without_parent(specs: &mut [OperationSpec]) {
    for index in 0..specs.len() {
        let spec = &specs[index];
        if !matches!(spec.action, Action::Copy | Action::Replace) {
            continue;
        }
        let parent = spec.path.parent().unwrap_or_default();
        if !directory_available(specs, &parent) {
            specs[index].action = Action::Skip;
            specs[index].reason = Reason::ParentUnavailable;
        }
    }
}

fn skip_unemptied_directory_deletions(specs: &mut [OperationSpec]) {
    // Ordre inverse : `p/…` suit toujours `p`, le sort des enfants est donc
    // fixé avant celui du parent.
    for index in (0..specs.len()).rev() {
        let spec = &specs[index];
        let is_directory_deletion = spec.action == Action::Delete
            && spec
                .destination
                .as_ref()
                .is_some_and(|d| d.kind() == EntryKind::Directory);
        if !is_directory_deletion {
            continue;
        }
        // Les descendants ne suivent pas forcément le répertoire : `p.old`
        // ou `p-1` s'intercalent (`.` et `-` précèdent `/`). Les chemins
        // `p/…` forment en revanche une plage contiguë.
        let prefix = format!("{}/", spec.path.as_str());
        let depth = spec.path.depth() + 1;
        let start = specs.partition_point(|other| other.path.as_str() < prefix.as_str());
        let kept = specs[start..]
            .iter()
            .take_while(|child| child.path.as_str().starts_with(prefix.as_str()))
            .filter(|child| child.path.depth() == depth)
            .any(|child| child.action != Action::Delete);
        if kept {
            specs[index].action = Action::Skip;
            specs[index].reason = Reason::DirectoryNotEmptied;
        }
    }
}

fn apply_deletion_guard(
    specs: &mut [OperationSpec],
    policy: &PlanPolicy,
    source_complete: bool,
    destination_complete: bool,
) {
    let deletions = specs
        .iter()
        .filter(|spec| spec.action == Action::Delete)
        .count() as u64;
    if deletions == 0 {
        return;
    }
    // Sans garde-fou (suppressions non autorisées), le plan refusera les
    // suppressions : défaut du mode, rapporté comme plan incohérent.
    let Some(guard) = policy.deletion.guard() else {
        return;
    };
    let census = DeletionCensus {
        deletions,
        destination_entries: specs.iter().filter(|s| s.destination.is_some()).count() as u64,
        source_entries: specs.iter().filter(|s| s.source.is_some()).count() as u64,
        source_complete,
        destination_complete,
    };
    if let Err(refusal) = guard.check(&census) {
        for spec in specs
            .iter_mut()
            .filter(|spec| spec.action == Action::Delete)
        {
            spec.action = Action::Skip;
            spec.reason = Reason::DeletionRefused { refusal };
        }
    }
}
