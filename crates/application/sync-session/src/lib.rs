//! Application lifecycle for bidirectional sync: hashed preview, durable
//! intent, execution of that exact plan, rescan and atomic state publication.
//! No GUI, scheduler or connector implementation is imported here.

use connector_api::StorageConnector;
use sync_bidirectional::{BidirectionalError, BidirectionalStrategy, binding, converged_baseline};
use sync_core::{CancellationToken, ExecutionId, ExecutionStatus, HashAlgorithm};
use sync_decisions::PlanContext;
use sync_executor::{
    ExecutionError, ExecutionOptions, ExecutionReport, ExecutionRequest, Executor, PlanExecutor,
};
use sync_filter::Filter;
use sync_history::{History, HistoryError, RetentionPolicy};
use sync_journal::JournalSink;
use sync_plan::{ExecutionPlan, PlanPolicy, StateBinding, SyncMode};
use sync_scanner::{ScanError, ScanOptions, scan};
use sync_versioning::VersionStore;
use thiserror::Error;

/// Lifecycle failure. Once intent is recorded, persistence failures leave
/// an unfinished record for explicit diagnosis; no baseline is assumed valid.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Scan could not establish an inventory.
    #[error(transparent)]
    Scan(#[from] ScanError),
    /// Planning or convergence failed.
    #[error(transparent)]
    Strategy(#[from] BidirectionalError),
    /// Durable state operation failed.
    #[error(transparent)]
    History(#[from] HistoryError),
    /// Executor refused the request before mutations.
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    /// Persisting the structured report failed.
    #[error(transparent)]
    Format(#[from] serde_json::Error),
    /// Incompatible request.
    #[error("expected a named bidirectional plan")]
    WrongPlan,
}

impl sync_errors::ClassifiedError for SessionError {
    fn class(&self) -> sync_errors::ErrorClass {
        use sync_errors::ErrorClass;
        match self {
            Self::Scan(error) => error.class(),
            Self::Strategy(error) => error.class(),
            Self::History(error) => error.class(),
            Self::Execution(error) => error.class(),
            Self::Format(_) => ErrorClass::Integrity,
            Self::WrongPlan => ErrorClass::InvalidInput,
        }
    }
}

/// Computes a hashed preview using the current reference, without mutating
/// files or publishing state. Locations in `context` must be stable canonical
/// identities; callers validate disjoint roots and keep the DB outside them.
///
/// # Errors
/// Unavailable root, cancellation, mismatching state identity or invalid plan.
pub fn preview(
    history: &History,
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
    context: &PlanContext,
    filter: &Filter,
    policy: &PlanPolicy,
    cancel: &CancellationToken,
) -> Result<ExecutionPlan, SessionError> {
    let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
    let left = scan(source, filter, &options, cancel)?;
    let right = scan(destination, filter, &options, cancel)?;
    let identity = binding(&left, &right, context)?;
    let baseline = history.baseline(&identity)?;
    Ok(BidirectionalStrategy.build_plan(&left, &right, context, baseline.as_ref(), policy)?)
}

/// One exact preview plan to execute. The caller must prevent simultaneous
/// runs touching these trees through different profiles/databases and validate
/// that the roots, database, journal and version store are disjoint.
pub struct SessionRequest<'a> {
    /// Exact validated preview plan.
    pub plan: &'a ExecutionPlan,
    /// Physical source (may receive mutations).
    pub source: &'a dyn StorageConnector,
    /// Physical destination (may receive mutations).
    pub destination: &'a dyn StorageConnector,
    /// Unique run identifier.
    pub execution: ExecutionId,
    /// Durable journal, outside synchronized trees.
    pub journal: &'a mut dyn JournalSink,
    /// Cooperative cancellation for scans and execution.
    pub cancel: &'a CancellationToken,
    /// Deletions require explicit confirmation here as well as in the plan.
    pub options: ExecutionOptions,
    /// Optional verified preservation before every overwrite/delete.
    /// Retention is conservative: content versions are never purged here.
    pub versions: Option<&'a VersionStore<'a>>,
}

/// Result including execution outcome and reference-state publication.
#[derive(Debug)]
pub struct SessionReport {
    /// All operation outcomes, including physical targets and partial work.
    pub execution: ExecutionReport,
    /// True only after successful rescan and committed state transaction.
    pub baseline_updated: bool,
    /// Explicit reason reference validation failed after execution.
    pub verification_error: Option<String>,
}

/// Executes the preview, then independently rescans both endpoints. Only a
/// completely successful, converged result publishes baseline and tombstones.
/// Warnings, conflicts, refusal, interruption or errors retain the old state.
///
/// # Errors
/// Intent cannot be recorded, preflight fails, or durable completion fails.
/// File mutations are not transactional; inspect history/journal on errors.
pub fn execute(
    history: &mut History,
    request: SessionRequest<'_>,
    retention: RetentionPolicy,
) -> Result<SessionReport, SessionError> {
    let plan = request.plan;
    if plan.mode() != SyncMode::Bidirectional {
        return Err(SessionError::WrongPlan);
    }
    let profile = plan.profile().cloned().ok_or(SessionError::WrongPlan)?;
    let identity = StateBinding {
        profile: profile.clone(),
        source_connector: plan.source().connector().clone(),
        destination_connector: plan.destination().connector().clone(),
        source: plan.source().location().to_owned(),
        destination: plan.destination().location().to_owned(),
        filter: plan.filter_rules().clone(),
    };
    let filter = Filter::new(plan.filter_rules().clone()).map_err(|_| SessionError::WrongPlan)?;
    let token = history.begin(request.execution.clone(), &identity, plan)?;
    let mut execution = ExecutionRequest::new(
        plan,
        request.source,
        request.destination,
        request.execution,
        request.journal,
        request.cancel,
    )
    .with_options(request.options);
    if let Some(versions) = request.versions {
        execution = execution.with_versions(versions);
    }
    let report = match Executor.execute(execution) {
        Ok(report) => report,
        Err(error) => {
            history.finish(
                token,
                ExecutionStatus::Failed,
                serde_json::json!({"preflight_error":error.to_string()}),
                None,
                retention,
            )?;
            return Err(error.into());
        }
    };
    let mut next = None;
    let mut verification_error = None;
    if report.status() == ExecutionStatus::Succeeded {
        let context = PlanContext {
            plan_id: plan.id().clone(),
            profile: Some(profile),
            created_at: plan.created_at(),
            source_location: identity.source,
            destination_location: identity.destination,
        };
        let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
        let result = (|| -> Result<_, SessionError> {
            let left = scan(request.source, &filter, &options, request.cancel)?;
            let right = scan(request.destination, &filter, &options, request.cancel)?;
            Ok(converged_baseline(
                &left,
                &right,
                &context,
                plan.baseline(),
            )?)
        })();
        match result {
            Ok(baseline) => next = Some(baseline),
            Err(error) => verification_error = Some(error.to_string()),
        }
    }
    let status = if verification_error.is_some() {
        ExecutionStatus::Failed
    } else {
        report.status()
    };
    history.finish(token,status,serde_json::json!({"execution":serde_json::to_value(&report)?,"verification_error":verification_error}),next.as_ref(),retention)?;
    Ok(SessionReport {
        execution: report,
        baseline_updated: next.is_some(),
        verification_error,
    })
}
