//! Aides communes : construction de plans par le vrai pipeline (scan,
//! comparaison, stratégie) et exécution.

#![allow(
    dead_code,
    reason = "chaque binaire de test n'utilise qu'une partie des aides"
)]
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "aides de test : un échec est un échec de test"
)]

use std::sync::{Arc, Mutex, PoisonError};

use connector_api::StorageConnector;
use sync_compare::{CompareOptions, compare};
use sync_core::{CancellationToken, ExecutionId, HashAlgorithm, PlanId, ResourcePath, Timestamp};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_events::{EmitOutcome, Event, EventEmitter, EventKind, EventSink};
use sync_executor::{
    ExecutionError, ExecutionOptions, ExecutionReport, ExecutionRequest, Executor, OutcomeStatus,
    PlanExecutor, Refusal,
};
use sync_filter::Filter;
use sync_journal::MemoryJournal;
use sync_plan::{
    DeletionGuard, DeletionPolicy, ExecutionPlan, OverwritePolicy, PlanPolicy, SyncMode,
};
use sync_scanner::{ScanOptions, scan};
use sync_versioning::VersionStore;

/// Instant de planification et d'exécution des tests.
pub(crate) const NOW: i64 = 1_800_000_000;

pub(crate) fn now() -> Timestamp {
    Timestamp::new(NOW, 0).unwrap()
}

pub(crate) fn p(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap()
}

/// Construit un plan par le pipeline réel.
pub(crate) fn plan_with(
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
    mode: SyncMode,
    policy: &PlanPolicy,
    filter: &Filter,
    hash: bool,
) -> ExecutionPlan {
    let cancel = CancellationToken::new();
    let mut options = ScanOptions::new();
    if hash {
        options = options.with_hash(HashAlgorithm::Blake3);
    }
    let left = scan(source, filter, &options, &cancel).unwrap();
    let right = scan(destination, filter, &options, &cancel).unwrap();
    let report = compare(&left, &right, &CompareOptions::new()).unwrap();
    let context = PlanContext {
        plan_id: PlanId::new("plan-test").unwrap(),
        profile: None,
        created_at: now(),
        source_location: "source".to_owned(),
        destination_location: "destination".to_owned(),
    };
    let input = PlanningInput::new(&report, &context);
    match mode {
        SyncMode::Mirror => sync_mirror::MirrorStrategy::new().build_plan(&input, policy),
        _ => sync_backup::BackupStrategy::new().build_plan(&input, policy),
    }
    .unwrap()
}

pub(crate) fn backup_plan(
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
) -> ExecutionPlan {
    plan_with(
        source,
        destination,
        SyncMode::Backup,
        &sync_backup::BackupStrategy::default_policy(),
        &Filter::include_all(),
        false,
    )
}

/// Miroir, la source l'emporte, suppressions autorisées sans plafond relatif.
pub(crate) fn mirror_policy() -> PlanPolicy {
    PlanPolicy::new(
        OverwritePolicy::SourceWins,
        DeletionPolicy::enabled(DeletionGuard::new().with_max_percent(None).unwrap()),
    )
}

pub(crate) fn mirror_plan(
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
) -> ExecutionPlan {
    plan_with(
        source,
        destination,
        SyncMode::Mirror,
        &mirror_policy(),
        &Filter::include_all(),
        false,
    )
}

/// Options de test : suppressions confirmées, petit tampon.
pub(crate) fn confirmed() -> ExecutionOptions {
    ExecutionOptions::new()
        .with_deletions_confirmed(true)
        .with_buffer_size(1024)
}

/// Exécute avec un journal mémoire.
pub(crate) fn execute(
    plan: &ExecutionPlan,
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
    options: ExecutionOptions,
    journal: &mut MemoryJournal,
) -> Result<ExecutionReport, ExecutionError> {
    let cancel = CancellationToken::new();
    Executor::new().execute(
        ExecutionRequest::new(
            plan,
            source,
            destination,
            ExecutionId::new("exec-test").unwrap(),
            journal,
            &cancel,
        )
        .with_options(options)
        .at(now()),
    )
}

/// Exécute avec conservation des versions.
pub(crate) fn execute_with_versions(
    plan: &ExecutionPlan,
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
    versions: &VersionStore<'_>,
    journal: &mut MemoryJournal,
) -> Result<ExecutionReport, ExecutionError> {
    let cancel = CancellationToken::new();
    Executor::new().execute(
        ExecutionRequest::new(
            plan,
            source,
            destination,
            ExecutionId::new("exec-test").unwrap(),
            journal,
            &cancel,
        )
        .with_options(confirmed())
        .with_versions(versions)
        .at(now()),
    )
}

pub(crate) fn status<'r>(report: &'r ExecutionReport, path: &str) -> &'r OutcomeStatus {
    report.outcome(&p(path)).unwrap().status()
}

pub(crate) fn refusal(report: &ExecutionReport, path: &str) -> Refusal {
    match status(report, path) {
        OutcomeStatus::Refused { refusal } => refusal.clone(),
        other => panic!("{path} : refus attendu, obtenu {other:?}"),
    }
}

pub(crate) fn is_done(report: &ExecutionReport, path: &str) -> bool {
    matches!(status(report, path), OutcomeStatus::Done { .. })
}

/// Collecteur d'événements qui peut annuler l'exécution après `n` mutations.
pub(crate) struct Recorder {
    events: Mutex<Vec<Event>>,
    cancel_after: Option<(u64, CancellationToken)>,
}

impl Recorder {
    pub(crate) fn new(cancel_after: Option<(u64, CancellationToken)>) -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            cancel_after,
        })
    }

    pub(crate) fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn emitter(self: &Arc<Self>) -> EventEmitter {
        EventEmitter::new(
            ExecutionId::new("exec-test").unwrap(),
            None,
            Arc::clone(self) as Arc<dyn EventSink>,
        )
    }
}

impl EventSink for Recorder {
    fn emit(&self, event: Event) -> EmitOutcome {
        if let (Some((after, token)), EventKind::Progress(progress)) =
            (&self.cancel_after, event.kind())
            && progress.entries_done >= *after
        {
            token.cancel();
        }
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        EmitOutcome::Delivered
    }
}
