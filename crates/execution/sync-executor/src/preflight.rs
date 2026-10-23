//! Contrôles préalables : aucun n'effectue de mutation.

use connector_api::{StorageConnector, StorageFeature};
use sync_core::{EntryKind, ResourcePath};
use sync_plan::{Action, PlanSide};

use crate::ExecutionRequest;
use crate::error::ExecutionError;

/// Écart en nanosecondes `later - earlier`.
fn nanos_between(earlier: sync_core::Timestamp, later: sync_core::Timestamp) -> i128 {
    let nanos = |ts: sync_core::Timestamp| {
        i128::from(ts.secs()) * 1_000_000_000 + i128::from(ts.subsec_nanos())
    };
    nanos(later) - nanos(earlier)
}

pub(crate) fn check(request: &ExecutionRequest<'_>) -> Result<(), ExecutionError> {
    if request.cancel.is_cancelled() {
        return Err(ExecutionError::Cancelled);
    }
    check_age(request)?;
    check_connectors(request)?;
    let summary = request.plan.summary();
    if summary.delete > 0 && !request.options.confirm_deletions {
        return Err(ExecutionError::DeletionsNotConfirmed {
            count: summary.delete,
        });
    }
    if summary.mutations() == 0 {
        return Ok(());
    }
    check_capabilities(request)?;
    check_root(request.source, PlanSide::Source, request)?;
    check_root(request.destination, PlanSide::Destination, request)
}

fn check_age(request: &ExecutionRequest<'_>) -> Result<(), ExecutionError> {
    let age = nanos_between(request.plan.created_at(), request.now);
    let skew = i128::try_from(request.options.max_clock_skew.as_nanos()).unwrap_or(i128::MAX);
    if age < -skew {
        return Err(ExecutionError::PlanFromFuture);
    }
    let max = i128::try_from(request.options.max_plan_age.as_nanos()).unwrap_or(i128::MAX);
    if age > max {
        return Err(ExecutionError::PlanExpired {
            age_secs: u64::try_from(age / 1_000_000_000).unwrap_or(u64::MAX),
            max_secs: request.options.max_plan_age.as_secs(),
        });
    }
    Ok(())
}

fn check_connectors(request: &ExecutionRequest<'_>) -> Result<(), ExecutionError> {
    for (side, expected, connector) in [
        (
            PlanSide::Source,
            request.plan.source().connector(),
            request.source,
        ),
        (
            PlanSide::Destination,
            request.plan.destination().connector(),
            request.destination,
        ),
    ] {
        if connector.connector_id() != expected {
            return Err(ExecutionError::ConnectorMismatch {
                side,
                expected: expected.clone(),
                found: connector.connector_id().clone(),
            });
        }
    }
    Ok(())
}

fn check_capabilities(request: &ExecutionRequest<'_>) -> Result<(), ExecutionError> {
    let mut source = vec![StorageFeature::Stat];
    let mut destination = vec![StorageFeature::Stat];
    for op in request.plan.operations() {
        match (op.action(), op.kind()) {
            (Action::Copy | Action::Replace, EntryKind::File) => {
                source.push(StorageFeature::Read);
                destination.push(StorageFeature::Write);
                if request.options.verify == sync_verify::VerifyMode::Content {
                    destination.push(StorageFeature::Read);
                }
            }
            (Action::Copy, EntryKind::Directory) => {
                destination.push(StorageFeature::CreateDirectory);
            }
            (Action::Delete, _) => destination.push(StorageFeature::Delete),
            _ => {}
        }
        if matches!(op.action(), Action::Replace | Action::Delete)
            && request.versions.is_some()
            && op.kind() == EntryKind::File
        {
            destination.push(StorageFeature::Read);
        }
    }
    for (side, connector, features) in [
        (PlanSide::Source, request.source, source),
        (PlanSide::Destination, request.destination, destination),
    ] {
        if let Some(feature) = features
            .into_iter()
            .find(|feature| !connector.capabilities().supports(*feature))
        {
            return Err(ExecutionError::CapabilityMissing { side, feature });
        }
    }
    Ok(())
}

fn check_root(
    connector: &dyn StorageConnector,
    side: PlanSide,
    request: &ExecutionRequest<'_>,
) -> Result<(), ExecutionError> {
    match connector.stat(&ResourcePath::root(), request.cancel) {
        Ok(entry) if entry.kind() == EntryKind::Directory => Ok(()),
        Ok(_) => Err(ExecutionError::EndpointNotDirectory { side }),
        Err(error) => Err(ExecutionError::EndpointUnavailable { side, error }),
    }
}
