//! Pure bidirectional strategy. Absence requires a complete scan; deletion
//! additionally requires an unchanged survivor in a matching baseline.
//! Conflicts keep both contents in place. No timestamp chooses a winner.

use std::collections::{BTreeMap, BTreeSet};

use sync_core::{EntryKind, FileEntry, HashAlgorithm};
use sync_decisions::PlanContext;
use sync_errors::{ClassifiedError, ErrorClass};
use sync_plan::{
    Action, Baseline, BaselineError, DeletionCensus, EndpointSnapshot, EntrySnapshot,
    ExecutionPlan, OperationSpec, OverwritePolicy, PlanError, PlanHeader, PlanPolicy, PlanSide,
    Reason, StateBinding, SyncMode, UncertainCause, same_content,
};
use sync_scanner::{Inventory, Presence};
use thiserror::Error;

/// Planning or convergence failure, never a reason to erase reference state.
#[derive(Debug, Error)]
pub enum BidirectionalError {
    /// Profile, endpoints or filters changed.
    #[error("reference identity does not match profile, roots or filters")]
    BindingMismatch,
    /// Baseline cannot be established from partial or divergent scans.
    #[error("inventories are incomplete, uncertain or not converged")]
    NotConverged,
    /// Invalid reference representation.
    #[error(transparent)]
    Baseline(#[from] BaselineError),
    /// Plan construction invariant failed.
    #[error(transparent)]
    Plan(#[from] PlanError),
}

impl ClassifiedError for BidirectionalError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::BindingMismatch => ErrorClass::InvalidInput,
            Self::NotConverged => ErrorClass::Conflict,
            Self::Baseline(_) | Self::Plan(_) => ErrorClass::Integrity,
        }
    }
}

/// Stable reference identity derived from canonical context locations.
///
/// # Errors
/// Refuses an anonymous profile or mismatching scan filters.
pub fn binding(
    left: &Inventory,
    right: &Inventory,
    context: &PlanContext,
) -> Result<StateBinding, BidirectionalError> {
    let profile = context
        .profile
        .clone()
        .ok_or(BidirectionalError::BindingMismatch)?;
    if left.filter_rules() != right.filter_rules() {
        return Err(BidirectionalError::BindingMismatch);
    }
    Ok(StateBinding {
        profile,
        source_connector: left.connector().clone(),
        destination_connector: right.connector().clone(),
        source: context.source_location.clone(),
        destination: context.destination_location.clone(),
        filter: left.filter_rules().clone(),
    })
}

fn snapshot(entry: Option<&FileEntry>) -> Option<EntrySnapshot> {
    entry.map(EntrySnapshot::from)
}
fn uncertain() -> Reason {
    Reason::Uncertain {
        causes: vec![UncertainCause::InconsistentObservations],
    }
}
fn reliable(entry: &FileEntry) -> bool {
    entry.kind() == EntryKind::Directory
        || (entry.kind() == EntryKind::File
            && entry.metadata().size().is_some()
            && entry
                .metadata()
                .hash()
                .is_some_and(|h| h.algorithm() == HashAlgorithm::Blake3))
}

/// Stateless strategy; a baseline is supplied explicitly for every preview.
#[derive(Debug, Default, Clone, Copy)]
pub struct BidirectionalStrategy;

impl BidirectionalStrategy {
    /// Creates the same inspectable `ExecutionPlan` consumed by the executor.
    /// Only `SourceWins` denotes permission to apply the direction already
    /// proven by the baseline; it never chooses the physical source on a conflict.
    /// Other overwrite policies preserve conflicting contents without mutation.
    ///
    /// # Errors
    /// Refuses mismatching identities/filters or an invalid generated plan.
    pub fn build_plan(
        &self,
        left: &Inventory,
        right: &Inventory,
        context: &PlanContext,
        baseline: Option<&Baseline>,
        policy: &PlanPolicy,
    ) -> Result<ExecutionPlan, BidirectionalError> {
        let identity = binding(left, right, context)?;
        if baseline.is_some_and(|b| b.binding() != &identity) {
            return Err(BidirectionalError::BindingMismatch);
        }
        let paths: BTreeSet<_> = left
            .entries()
            .chain(right.entries())
            .map(|e| e.path().clone())
            .chain(
                baseline
                    .into_iter()
                    .flat_map(|b| b.entries().keys().cloned()),
            )
            .collect();
        let report = sync_compare::compare(left, right, &sync_compare::CompareOptions::new())
            .map_err(|_| BidirectionalError::BindingMismatch)?;
        let complete = left.is_complete()
            && right.is_complete()
            && left.issues().is_empty()
            && right.issues().is_empty();
        let mut specs = BTreeMap::new();
        for path in paths {
            let l = left.get(&path);
            let r = right.get(&path);
            if l.is_none() && r.is_none() {
                continue;
            }
            let mut target = PlanSide::Destination;
            let mut source = snapshot(l);
            let mut destination = snapshot(r);
            let previous = baseline.and_then(|b| b.entries().get(&path));
            let known = matches!(
                left.presence(&path),
                Presence::Present(_) | Presence::Absent
            ) && matches!(
                right.presence(&path),
                Presence::Present(_) | Presence::Absent
            );
            let collision = report
                .get(&path)
                .is_some_and(|row| row.outcome().is_uncertain());
            let (action, reason) =
                if !complete || !known || collision || l.into_iter().chain(r).any(|e| !reliable(e))
                {
                    (Action::Skip, uncertain())
                } else if let (Some(a), Some(b)) = (&source, &destination) {
                    if same_content(a, b) {
                        (
                            Action::Keep,
                            Reason::Equivalent {
                                verified: a.kind() == EntryKind::File,
                            },
                        )
                    } else if let Some(Some(old)) = previous {
                        if a.kind() == EntryKind::File
                            && b.kind() == EntryKind::File
                            && same_content(b, old)
                            && policy.overwrite == OverwritePolicy::SourceWins
                        {
                            (Action::Replace, Reason::ContentDiffers)
                        } else if a.kind() == EntryKind::File
                            && b.kind() == EntryKind::File
                            && same_content(a, old)
                            && policy.overwrite == OverwritePolicy::SourceWins
                        {
                            target = PlanSide::Source;
                            std::mem::swap(&mut source, &mut destination);
                            (Action::Replace, Reason::ContentDiffers)
                        } else {
                            (Action::Conflict, Reason::ConcurrentChange)
                        }
                    } else {
                        (Action::Conflict, Reason::ConcurrentChange)
                    }
                } else {
                    let present = source.as_ref().or(destination.as_ref());
                    match previous {
                        None => {
                            if source.is_none() {
                                target = PlanSide::Source;
                                std::mem::swap(&mut source, &mut destination);
                            }
                            (Action::Copy, Reason::MissingAtDestination)
                        }
                        Some(Some(old)) if present.is_some_and(|p| same_content(p, old)) => {
                            if destination.is_none() {
                                target = PlanSide::Source;
                                std::mem::swap(&mut source, &mut destination);
                            }
                            if policy.deletion.guard().is_some() {
                                (Action::Delete, Reason::AbsentFromSource)
                            } else {
                                (Action::Keep, Reason::DeletionDisabled)
                            }
                        }
                        _ => (Action::Conflict, Reason::ConcurrentChange),
                    }
                };
            specs.insert(
                path.clone(),
                (
                    target,
                    OperationSpec {
                        path,
                        action,
                        reason,
                        source,
                        destination,
                    },
                ),
            );
        }
        // A directory is never deleted over a preserved/modified descendant.
        let blocked: Vec<_> = specs
            .iter()
            .filter(|(_, (_, s))| {
                s.action == Action::Delete
                    && s.destination
                        .as_ref()
                        .is_some_and(|e| e.kind() == EntryKind::Directory)
            })
            .filter(|(path, (target, _))| {
                specs.iter().any(|(child, (ct, cs))| {
                    child.as_str().starts_with(&format!("{}/", path.as_str()))
                        && (*target != *ct || cs.action != Action::Delete)
                })
            })
            .map(|(p, _)| p.clone())
            .collect();
        for path in blocked {
            if let Some((_, s)) = specs.get_mut(&path) {
                s.action = Action::Skip;
                s.reason = Reason::DirectoryNotEmptied;
            }
        }
        // All directional guards must pass, otherwise no deletion survives.
        let refusal = policy.deletion.guard().and_then(|guard| {
            [PlanSide::Source, PlanSide::Destination]
                .into_iter()
                .find_map(|target| {
                    let (src, dst) = if target == PlanSide::Destination {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    guard
                        .check(&DeletionCensus {
                            deletions: specs
                                .values()
                                .filter(|(t, s)| *t == target && s.action == Action::Delete)
                                .count() as u64,
                            source_entries: src.len() as u64,
                            destination_entries: dst.len() as u64,
                            source_complete: complete,
                            destination_complete: complete,
                        })
                        .err()
                })
        });
        if let Some(refusal) = refusal {
            for (_, spec) in specs.values_mut() {
                if spec.action == Action::Delete {
                    spec.action = Action::Skip;
                    spec.reason = Reason::DeletionRefused { refusal };
                }
            }
        }
        // Refusing a parent deletion may require refusing its ancestors too;
        // derive the final directory and copy-parent safety bottom-up.
        let keys: Vec<_> = specs.keys().cloned().collect();
        for path in keys.iter().rev() {
            let Some((target, spec)) = specs.get(path) else {
                continue;
            };
            let prefix = format!("{}/", path.as_str());
            if spec.action == Action::Delete
                && spec
                    .destination
                    .as_ref()
                    .is_some_and(|e| e.kind() == EntryKind::Directory)
                && specs.iter().any(|(p, (t, s))| {
                    p.as_str().starts_with(&prefix) && (*t != *target || s.action != Action::Delete)
                })
                && let Some((_, s)) = specs.get_mut(path)
            {
                s.action = Action::Skip;
                s.reason = Reason::DirectoryNotEmptied;
            }
        }
        for path in &keys {
            let Some((target, spec)) = specs.get(path) else {
                continue;
            };
            if !matches!(spec.action, Action::Copy | Action::Replace) {
                continue;
            }
            let available = path.parent().is_none_or(|p| {
                p.is_root()
                    || specs.get(&p).is_some_and(|(t, s)| {
                        if s.action == Action::Copy && t == target {
                            s.source
                                .as_ref()
                                .is_some_and(|e| e.kind() == EntryKind::Directory)
                        } else if s.action == Action::Delete && t == target {
                            false
                        } else {
                            (if t == target {
                                s.destination.as_ref()
                            } else {
                                s.source.as_ref()
                            })
                            .is_some_and(|e| e.kind() == EntryKind::Directory)
                        }
                    })
            });
            if !available && let Some((_, s)) = specs.get_mut(path) {
                s.action = Action::Skip;
                s.reason = Reason::ParentUnavailable;
            }
        }
        let header = PlanHeader {
            id: context.plan_id.clone(),
            profile: context.profile.clone(),
            mode: SyncMode::Bidirectional,
            created_at: context.created_at,
            source: EndpointSnapshot::new(
                left.connector().clone(),
                &context.source_location,
                complete,
            ),
            destination: EndpointSnapshot::new(
                right.connector().clone(),
                &context.destination_location,
                complete,
            ),
            filter: left.filter_rules().clone(),
            policy: *policy,
        };
        Ok(ExecutionPlan::new_bidirectional(
            header,
            specs.into_values().collect(),
            baseline.cloned(),
        )?)
    }
}

/// Establishes a new generation only from complete, fully converged scans.
/// Carries previous tombstones forward and records newly absent paths.
///
/// # Errors
/// Any mismatch, missing hash, scan issue, divergence or generation overflow.
pub fn converged_baseline(
    left: &Inventory,
    right: &Inventory,
    context: &PlanContext,
    previous: Option<&Baseline>,
) -> Result<Baseline, BidirectionalError> {
    let identity = binding(left, right, context)?;
    if previous.is_some_and(|p| p.binding() != &identity) {
        return Err(BidirectionalError::BindingMismatch);
    }
    if !left.is_complete()
        || !right.is_complete()
        || !left.issues().is_empty()
        || !right.issues().is_empty()
    {
        return Err(BidirectionalError::NotConverged);
    }
    let report = sync_compare::compare(left, right, &sync_compare::CompareOptions::new())
        .map_err(|_| BidirectionalError::BindingMismatch)?;
    if report.rows().iter().any(|row| row.outcome().is_uncertain()) {
        return Err(BidirectionalError::NotConverged);
    }
    let mut entries = BTreeMap::new();
    let paths: BTreeSet<_> = left
        .entries()
        .chain(right.entries())
        .map(|e| e.path().clone())
        .chain(
            previous
                .into_iter()
                .flat_map(|p| p.entries().keys().cloned()),
        )
        .collect();
    for path in paths {
        match (left.presence(&path), right.presence(&path)) {
            (Presence::Present(a), Presence::Present(b))
                if reliable(a)
                    && reliable(b)
                    && same_content(&EntrySnapshot::from(a), &EntrySnapshot::from(b)) =>
            {
                entries.insert(path, Some(EntrySnapshot::from(a)));
            }
            (Presence::Absent, Presence::Absent) => {
                entries.insert(path, None);
            }
            _ => return Err(BidirectionalError::NotConverged),
        }
    }
    let generation = previous
        .map_or(Some(1), |p| p.generation().checked_add(1))
        .ok_or(BidirectionalError::NotConverged)?;
    Ok(Baseline::new(identity, generation, entries)?)
}
