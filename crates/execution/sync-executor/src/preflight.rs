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
    if request.cancel.checkpoint() {
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
    for op in request.plan.operations() {
        let (source, destination) = if op.target() == PlanSide::Destination {
            (request.source, request.destination)
        } else {
            (request.destination, request.source)
        };
        let mut reads = vec![StorageFeature::Stat];
        let mut writes = vec![StorageFeature::Stat];
        match (op.action(), op.kind()) {
            (Action::Copy | Action::Replace, EntryKind::File) => {
                reads.push(StorageFeature::Read);
                writes.push(StorageFeature::Write);
                if request.options.verify == sync_verify::VerifyMode::Content {
                    writes.push(StorageFeature::Read);
                }
            }
            (Action::Copy, EntryKind::Directory) => writes.push(StorageFeature::CreateDirectory),
            (Action::Delete, _) => writes.push(StorageFeature::Delete),
            _ => {}
        }
        if matches!(op.action(), Action::Replace | Action::Delete)
            && op.kind() == EntryKind::File
            && (request.versions.is_some()
                || request.plan.mode() == sync_plan::SyncMode::Bidirectional)
        {
            writes.push(StorageFeature::Read);
        }
        for (side, connector, features) in [
            (op.target().opposite(), source, reads),
            (op.target(), destination, writes),
        ] {
            if let Some(feature) = features
                .into_iter()
                .find(|f| !connector.capabilities().supports(*f))
            {
                return Err(ExecutionError::CapabilityMissing { side, feature });
            }
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
