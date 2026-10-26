//! Dérivation et validation des invariants d'un plan.

use std::collections::BTreeSet;

use sync_core::{EntryKind, IdError, OperationId, ResourcePath};
use sync_errors::{ClassifiedError, ErrorClass};
use thiserror::Error;

use crate::operation::{Action, EntrySnapshot, Operation, OperationSpec, PlanSide, Reason};
use crate::plan::{CASE_MERGE_PLAN_SCHEMA_VERSION, ExecutionPlan, PLAN_SCHEMA_VERSION, SyncMode};
use crate::policy::{DeletionCensus, DeletionPolicy, DeletionRefusal, OverwritePolicy};

/// Raison pour laquelle une suppression est inadmissible dans un plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeletionBlock {
    /// Le mode (sauvegarde) ne supprime jamais.
    ModeNeverDeletes,
    /// La politique n'autorise pas les suppressions.
    PolicyDisabled,
    /// Un garde-fou refuse les suppressions.
    Refused(DeletionRefusal),
}

/// Invariant de plan non respecté.
///
/// Les messages désignent les opérations par leur identifiant ; seuls les
/// chemins en double ou mal ordonnés sont cités (chemins logiques, non
/// secrets).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum PlanError {
    /// Persisted filter rules cannot be compiled safely.
    #[error("invalid filter rules in execution plan")]
    InvalidFilter,
    /// A mutation targets a filtered or reserved recovery path.
    #[error("operation {id} targets an excluded or reserved path")]
    FilteredMutation {
        /// Refused operation.
        id: OperationId,
    },
    /// Bidirectional mutation lacks reliable baseline/content evidence.
    #[error("bidirectional mutation lacks reliable baseline evidence")]
    BaselineEvidence,

    /// Version de schéma non prise en charge.
    #[error(
        "version de schéma de plan {found} non prise en charge (attendu {PLAN_SCHEMA_VERSION})"
    )]
    UnsupportedVersion {
        /// Version lue.
        found: u32,
    },
    /// Deux opérations portent sur le même chemin.
    #[error("plusieurs opérations pour le chemin « {path} »")]
    DuplicatePath {
        /// Chemin concerné.
        path: ResourcePath,
    },
    /// Opérations non triées par chemin.
    #[error("opérations non triées par chemin (à « {path} »)")]
    UnsortedOperations {
        /// Premier chemin hors d'ordre.
        path: ResourcePath,
    },
    /// Identifiant d'opération impossible à construire.
    #[error("identifiant d'opération invalide : {0}")]
    OperationId(#[source] IdError),
    /// Identifiant d'opération en double.
    #[error("identifiant d'opération {id} en double")]
    DuplicateOperationId {
        /// Identifiant concerné.
        id: OperationId,
    },
    /// Côtés observés incompatibles avec l'action ou le motif.
    #[error("opération {id} : observations incompatibles avec l'action ou le motif")]
    InvalidShape {
        /// Opération concernée.
        id: OperationId,
    },
    /// Motif non admis pour l'action.
    #[error("opération {id} : motif non admis pour l'action {action}")]
    ReasonNotAllowed {
        /// Opération concernée.
        id: OperationId,
        /// Action portée.
        action: Action,
    },
    /// Nature d'entrée non admise pour l'action (copie d'un lien…).
    #[error("opération {id} : nature d'entrée non admise pour l'action {action}")]
    UnsupportedKind {
        /// Opération concernée.
        id: OperationId,
        /// Action portée.
        action: Action,
    },
    /// Un champ dérivé ne correspond pas à sa valeur attendue.
    #[error("opération {id} : champ « {field} » incohérent")]
    DerivedMismatch {
        /// Opération concernée.
        id: OperationId,
        /// Champ concerné.
        field: &'static str,
    },
    /// Copie ou remplacement sous un répertoire indisponible à la destination.
    #[error("opération {id} : répertoire parent indisponible à la destination")]
    ParentUnavailable {
        /// Opération concernée.
        id: OperationId,
    },
    /// Suppression d'un répertoire dont le contenu n'est pas entièrement supprimé.
    #[error("opération {id} : répertoire supprimé alors que son contenu ne l'est pas")]
    DirectoryNotEmptied {
        /// Opération concernée.
        id: OperationId,
    },
    /// Suppression inadmissible.
    #[error("opération {id} : suppression inadmissible ({cause:?})")]
    DeletionNotAllowed {
        /// Première suppression concernée.
        id: OperationId,
        /// Cause.
        cause: DeletionBlock,
    },
    /// Remplacement interdit par la politique.
    #[error("opération {id} : remplacement interdit par la politique")]
    OverwriteNotAllowed {
        /// Opération concernée.
        id: OperationId,
    },
    /// Décompte d'entrées d'une extrémité incohérent avec les opérations.
    #[error("décompte d'entrées incohérent ({side:?})")]
    EntryCountMismatch {
        /// Extrémité concernée.
        side: PlanSide,
    },
}

impl ClassifiedError for PlanError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Integrity
    }
}

/// Identifiant de la `index`-ième opération (base 0) : `op-000001`…
fn operation_id(index: usize) -> Result<OperationId, PlanError> {
    OperationId::new(format!("op-{:06}", index + 1)).map_err(PlanError::OperationId)
}

fn derived_kind(
    action: Action,
    source: Option<&EntrySnapshot>,
    destination: Option<&EntrySnapshot>,
) -> Option<EntryKind> {
    let pick = |s: Option<&EntrySnapshot>| s.map(EntrySnapshot::kind);
    match action {
        Action::Delete => pick(destination),
        _ => pick(source).or_else(|| pick(destination)),
    }
}

fn derived_bytes(action: Action, source: Option<&EntrySnapshot>) -> Option<u64> {
    match (action, source) {
        (Action::Copy | Action::Replace, Some(source)) if source.kind() == EntryKind::File => {
            source.metadata().size()
        }
        _ => Some(0),
    }
}

/// Index de l'opération portant sur `path` (opérations triées).
fn index_of(operations: &[Operation], path: &ResourcePath) -> Option<usize> {
    operations.binary_search_by(|op| op.path.cmp(path)).ok()
}

/// Index des opérations portant sur les enfants directs de `index`.
///
/// Les descendants de `p` ne suivent pas forcément `p` immédiatement :
/// `p.old` ou `p-1` s'intercalent (`.` et `-` précèdent `/`). En revanche,
/// les chemins de forme textuelle `p/…` forment une plage contiguë de
/// l'ordre des chemins : on la localise par dichotomie.
fn children(operations: &[Operation], index: usize) -> impl Iterator<Item = usize> + '_ {
    let parent = &operations[index].path;
    let prefix = format!("{}/", parent.as_str());
    let depth = parent.depth() + 1;
    let start = operations.partition_point(|op| op.path.as_str() < prefix.as_str());
    operations[start..]
        .iter()
        .take_while(move |op| op.path.as_str().starts_with(prefix.as_str()))
        .enumerate()
        .filter(move |(_, op)| op.path.depth() == depth)
        .map(move |(offset, _)| start + offset)
}

/// Vrai si le répertoire `path` existera à la destination lors de
/// l'exécution des opérations qui en dépendent.
fn directory_available(operations: &[Operation], path: &ResourcePath, target: PlanSide) -> bool {
    if path.is_root() {
        return true;
    }
    let Some(op) = index_of(operations, path).and_then(|i| operations.get(i)) else {
        return false;
    };
    match op.action {
        Action::Copy if op.target == target => op.kind == EntryKind::Directory,
        Action::Delete if op.target == target => false,
        _ => op
            .observation(target)
            .is_some_and(|d| d.kind() == EntryKind::Directory),
    }
}

fn expected_dependencies(operations: &[Operation], index: usize) -> Vec<OperationId> {
    let op = &operations[index];
    match op.action {
        Action::Copy | Action::Replace => op
            .path
            .parent()
            .filter(|parent| !parent.is_root())
            .and_then(|parent| index_of(operations, &parent))
            .and_then(|i| operations.get(i))
            .filter(|parent| parent.action == Action::Copy && parent.target == op.target)
            .map(|parent| vec![parent.id.clone()])
            .unwrap_or_default(),
        Action::Delete if op.kind == EntryKind::Directory => children(operations, index)
            .map(|i| operations[i].id.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// Construit les opérations numérotées et leurs champs dérivés.
pub(crate) fn derive_operations(
    mut specs: Vec<(PlanSide, OperationSpec)>,
) -> Result<Vec<Operation>, PlanError> {
    specs.sort_by(|a, b| a.1.path.cmp(&b.1.path));
    if let Some(pair) = specs
        .windows(2)
        .find(|pair| pair[0].1.path == pair[1].1.path)
    {
        return Err(PlanError::DuplicatePath {
            path: pair[1].1.path.clone(),
        });
    }
    let mut operations: Vec<Operation> = specs
        .into_iter()
        .enumerate()
        .map(|(index, (target, spec))| {
            let kind = derived_kind(spec.action, spec.source.as_ref(), spec.destination.as_ref())
                .unwrap_or(EntryKind::Other);
            Ok(Operation {
                target,
                id: operation_id(index)?,
                kind,
                risk: spec.action.risk(),
                preconditions: spec.action.preconditions().to_vec(),
                depends_on: Vec::new(),
                bytes: derived_bytes(spec.action, spec.source.as_ref()),
                path: spec.path,
                action: spec.action,
                reason: spec.reason,
                source: spec.source,
                destination: spec.destination,
            })
        })
        .collect::<Result<_, PlanError>>()?;
    let dependencies: Vec<Vec<OperationId>> = (0..operations.len())
        .map(|index| expected_dependencies(&operations, index))
        .collect();
    for (op, depends_on) in operations.iter_mut().zip(dependencies) {
        op.depends_on = depends_on;
    }
    Ok(operations)
}

/// Vérifie tous les invariants d'un plan.
pub(crate) fn validate(plan: &ExecutionPlan) -> Result<(), PlanError> {
    if plan.schema_version
        != if plan
            .operations
            .iter()
            .any(|op| op.source().is_some_and(|s| !s.origin_paths().is_empty()))
        {
            CASE_MERGE_PLAN_SCHEMA_VERSION
        } else if plan.mode == SyncMode::Bidirectional {
            2
        } else {
            PLAN_SCHEMA_VERSION
        }
    {
        return Err(PlanError::UnsupportedVersion {
            found: plan.schema_version,
        });
    }
    if let Some(base) = &plan.baseline {
        let binding = base.binding();
        if plan.mode != SyncMode::Bidirectional
            || plan.profile.as_ref() != Some(&binding.profile)
            || plan.source.connector() != &binding.source_connector
            || plan.destination.connector() != &binding.destination_connector
            || plan.source.location() != binding.source
            || plan.destination.location() != binding.destination
            || plan.filter != binding.filter
        {
            return Err(PlanError::BaselineEvidence);
        }
    }
    let operations = &plan.operations;
    for op in operations {
        if op
            .destination()
            .is_some_and(|entry| !entry.origin_paths().is_empty())
            || op.source().is_some_and(|entry| {
                !entry.origin_paths().is_empty()
                    && (plan.mode == SyncMode::Bidirectional
                        || (entry.kind() != EntryKind::Directory
                            && entry.origin_paths().len() != 1)
                        || entry.origin_paths().iter().any(|path| {
                            path.as_str().to_lowercase() != op.path().as_str().to_lowercase()
                        })
                        || entry
                            .origin_paths()
                            .windows(2)
                            .any(|pair| pair[0] >= pair[1]))
            })
        {
            return Err(PlanError::InvalidShape {
                id: op.id().clone(),
            });
        }
    }
    let filter =
        sync_filter::Filter::new(plan.filter.clone()).map_err(|_| PlanError::InvalidFilter)?;
    for pair in operations.windows(2) {
        match pair[0].path.cmp(&pair[1].path) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                return Err(PlanError::DuplicatePath {
                    path: pair[1].path.clone(),
                });
            }
            std::cmp::Ordering::Greater => {
                return Err(PlanError::UnsortedOperations {
                    path: pair[1].path.clone(),
                });
            }
        }
    }
    let mut ids = BTreeSet::new();
    for op in operations {
        if !ids.insert(&op.id) {
            return Err(PlanError::DuplicateOperationId { id: op.id.clone() });
        }
    }
    for (index, op) in operations.iter().enumerate() {
        if op.action.is_mutation() && !filter.evaluate(&op.path, op.kind).is_included() {
            return Err(PlanError::FilteredMutation { id: op.id.clone() });
        }
        validate_operation(plan, index, op)?;
    }
    validate_entry_counts(plan)?;
    validate_deletions(plan)
}

fn validate_operation(plan: &ExecutionPlan, index: usize, op: &Operation) -> Result<(), PlanError> {
    let operations = &plan.operations;
    let id = || op.id.clone();
    let mismatch = |field| PlanError::DerivedMismatch { id: id(), field };
    let source = op.source.as_ref();
    let destination = op.destination.as_ref();

    if plan.mode == SyncMode::Bidirectional && op.action.is_mutation() {
        let reliable = |entry: &EntrySnapshot| {
            entry.kind() == EntryKind::Directory
                || (entry.kind() == EntryKind::File
                    && entry.metadata().size().is_some()
                    && entry
                        .metadata()
                        .hash()
                        .is_some_and(|h| h.algorithm() == sync_core::HashAlgorithm::Blake3))
        };
        if !plan.source.is_complete()
            || !plan.destination.is_complete()
            || source.is_some_and(|e| !reliable(e))
            || destination.is_some_and(|e| !reliable(e))
        {
            return Err(PlanError::BaselineEvidence);
        }
        let previous = plan
            .baseline
            .as_ref()
            .and_then(|b| b.entries().get(&op.path));
        match op.action {
            Action::Copy if previous.is_some() => return Err(PlanError::BaselineEvidence),
            Action::Replace | Action::Delete => {
                let Some(Some(previous)) = previous else {
                    return Err(PlanError::BaselineEvidence);
                };
                if !destination.is_some_and(|current| crate::same_content(previous, current)) {
                    return Err(PlanError::BaselineEvidence);
                }
            }
            _ => {}
        }
    }
    if op.target == PlanSide::Source && plan.mode != SyncMode::Bidirectional {
        return Err(PlanError::InvalidShape { id: id() });
    }
    if !op.reason.allows(op.action) {
        return Err(PlanError::ReasonNotAllowed {
            id: id(),
            action: op.action,
        });
    }
    let action_shape = match op.action {
        Action::Copy => crate::operation::Shape::SourceOnly,
        Action::Replace => crate::operation::Shape::Both,
        Action::Delete => crate::operation::Shape::DestinationOnly,
        Action::Keep | Action::Conflict | Action::Skip => crate::operation::Shape::Any,
    };
    if !action_shape.accepts(source.is_some(), destination.is_some())
        || !op
            .reason
            .shape()
            .accepts(source.is_some(), destination.is_some())
    {
        return Err(PlanError::InvalidShape { id: id() });
    }
    let kinds_ok = match op.action {
        Action::Copy => source.is_some_and(|s| is_file_or_dir(s.kind())),
        Action::Replace => {
            source.is_some_and(|s| s.kind() == EntryKind::File)
                && destination.is_some_and(|d| d.kind() == EntryKind::File)
        }
        Action::Delete => destination.is_some_and(|d| is_file_or_dir(d.kind())),
        Action::Keep | Action::Conflict | Action::Skip => true,
    };
    if !kinds_ok {
        return Err(PlanError::UnsupportedKind {
            id: id(),
            action: op.action,
        });
    }
    match &op.reason {
        Reason::KindMismatch {
            source: s,
            destination: d,
        } if Some(*s) != source.map(EntrySnapshot::kind)
            || Some(*d) != destination.map(EntrySnapshot::kind)
            || s == d =>
        {
            return Err(mismatch("reason"));
        }
        Reason::Uncertain { causes } if causes.is_empty() => return Err(mismatch("reason")),
        _ => {}
    }

    if Some(op.kind) != derived_kind(op.action, source, destination) {
        return Err(mismatch("kind"));
    }
    if op.risk != op.action.risk() {
        return Err(mismatch("risk"));
    }
    if op.preconditions != op.action.preconditions() {
        return Err(mismatch("preconditions"));
    }
    if op.bytes != derived_bytes(op.action, source) {
        return Err(mismatch("bytes"));
    }
    if op.depends_on != expected_dependencies(operations, index) {
        return Err(mismatch("depends_on"));
    }

    match op.action {
        Action::Copy | Action::Replace => {
            let parent = op.path.parent().unwrap_or_default();
            if !directory_available(operations, &parent, op.target) {
                return Err(PlanError::ParentUnavailable { id: id() });
            }
        }
        Action::Delete
            if op.kind == EntryKind::Directory
                && children(operations, index).any(|i| {
                    operations[i].action != Action::Delete || operations[i].target != op.target
                }) =>
        {
            return Err(PlanError::DirectoryNotEmptied { id: id() });
        }
        _ => {}
    }

    if op.action == Action::Replace {
        let allowed = match plan.policy.overwrite {
            OverwritePolicy::SourceWins => true,
            OverwritePolicy::NewerSourceOnly => op.reason == Reason::SourceNewer,
            OverwritePolicy::KeepDestination => false,
        };
        if !allowed {
            return Err(PlanError::OverwriteNotAllowed { id: id() });
        }
    }
    Ok(())
}

fn is_file_or_dir(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::File | EntryKind::Directory)
}

fn validate_entry_counts(plan: &ExecutionPlan) -> Result<(), PlanError> {
    let sources = plan
        .operations
        .iter()
        .filter(|op| op.observation(PlanSide::Source).is_some())
        .count() as u64;
    let destinations = plan
        .operations
        .iter()
        .filter(|op| op.observation(PlanSide::Destination).is_some())
        .count() as u64;
    if plan.source.entries() != sources {
        return Err(PlanError::EntryCountMismatch {
            side: PlanSide::Source,
        });
    }
    if plan.destination.entries() != destinations {
        return Err(PlanError::EntryCountMismatch {
            side: PlanSide::Destination,
        });
    }
    Ok(())
}

fn validate_deletions(plan: &ExecutionPlan) -> Result<(), PlanError> {
    let mut deletions = plan
        .operations
        .iter()
        .filter(|op| op.action == Action::Delete);
    let Some(first) = deletions.next() else {
        return Ok(());
    };
    let block = |cause| PlanError::DeletionNotAllowed {
        id: first.id.clone(),
        cause,
    };
    if plan.mode == SyncMode::Backup {
        return Err(block(DeletionBlock::ModeNeverDeletes));
    }
    let guard = match &plan.policy.deletion {
        DeletionPolicy::Enabled { guard } => guard,
        DeletionPolicy::Disabled => return Err(block(DeletionBlock::PolicyDisabled)),
    };
    for target in [PlanSide::Source, PlanSide::Destination] {
        let (source, destination) = if target == PlanSide::Destination {
            (&plan.source, &plan.destination)
        } else {
            (&plan.destination, &plan.source)
        };
        let census = DeletionCensus {
            deletions: plan
                .operations
                .iter()
                .filter(|op| op.action == Action::Delete && op.target == target)
                .count() as u64,
            destination_entries: destination.entries(),
            source_entries: source.entries(),
            source_complete: source.is_complete(),
            destination_complete: destination.is_complete(),
        };
        guard
            .check(&census)
            .map_err(|refusal| block(DeletionBlock::Refused(refusal)))?;
    }
    Ok(())
}
