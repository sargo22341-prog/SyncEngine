//! Déroulement d'une exécution.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
mod parallel;

pub(crate) fn start(req: ExecutionRequest<'_>) -> Result<ExecutionReport, ExecutionError> {
    let workers = if req.plan.mode() == sync_plan::SyncMode::Bidirectional {
        1
    } else {
        req.options
            .copy_workers
            .get()
            .min(req.source.transfer_concurrency().get())
            .min(req.destination.transfer_concurrency().get())
            .min(
                req.versions
                    .map_or(16, |store| store.transfer_concurrency().get()),
            )
            .min(16)
    };
    if workers == 1 {
        return Run::start(req, None, 1);
    }
    let cancel = req.cancel.child_token();
    let journal = Mutex::new(req.journal);
    let mut proxy = parallel::JournalProxy(&journal);
    let req = ExecutionRequest {
        journal: &mut proxy,
        cancel: &cancel,
        ..req
    };
    Run::start(req, Some(&journal), workers)
}

use connector_api::{CommitMode, StorageError, StorageErrorKind, WriteOptions};
use sync_copy::{CopyError, CopyRequest, CopyStage, StagedCopy, stage_copy_observed};
use sync_core::{EntryKind, ExecutionStatus, OperationId, ResourcePath};
use sync_errors::{ClassifiedError, ErrorContext, ErrorReport, Severity};
use sync_events::{EmitOutcome, EventEmitter, EventKind, ProgressSnapshot};
use sync_journal::{JournalError, JournalRecord, JournalTotals};
use sync_plan::{Action, EntrySnapshot, Operation, PlanSide};
use sync_verify::{
    Mismatch, VerifyError, VerifyMode, check_reread, check_transfer, check_written,
    compare_observation, verify_content, verify_prefix,
};
use sync_versioning::VersioningError;

use crate::ExecutionRequest;
use crate::error::{ExecutionError, Fault, RefusalNotice};
use crate::report::{ExecutionReport, OperationOutcome, OutcomeStatus, Refusal, StopCause};

/// Raison pour laquelle une mutation n'a pas abouti.
enum Interrupt {
    Refused(Refusal),
    Failed {
        report: Box<ErrorReport>,
        stop: Option<StopCause>,
    },
    Cancelled,
}

/// Mutation réalisée : octets validés, version conservée.
type Done = (u64, Option<ResourcePath>);

/// Progression agrégée.
#[derive(Debug, Default)]
struct Progress {
    entries_done: u64,
    entries_total: u64,
    bytes_done: u64,
    bytes_total: Option<u64>,
    bytes_at_last_event: u64,
}

#[derive(Debug, Clone, Default)]
struct SharedProgress(Arc<Mutex<Progress>>);
impl SharedProgress {
    fn publish(&self, events: Option<&EventEmitter>) {
        let progress = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        emit(events, EventKind::Progress(progress.snapshot()));
    }
    fn completed(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries_done += 1;
    }
}

impl Progress {
    fn snapshot(&self) -> ProgressSnapshot {
        ProgressSnapshot {
            entries_done: self.entries_done,
            entries_total: Some(self.entries_total),
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
        }
    }
}

fn emit(events: Option<&EventEmitter>, kind: EventKind) {
    if let Some(events) = events {
        // Les événements sont informatifs : une perte est comptée et
        // signalée par l'émetteur lui-même, jamais bloquante.
        match events.emit(kind) {
            EmitOutcome::Delivered | EmitOutcome::Dropped(_) => {}
        }
    }
}

struct IoObserver<'a> {
    events: Option<&'a EventEmitter>,
    pending: u64,
    versions: bool,
}
impl IoObserver<'_> {
    fn advance(&mut self, bytes: u64) {
        self.pending = self.pending.saturating_add(bytes);
        if self.pending >= 1024 * 1024 {
            self.flush();
        }
    }
    fn flush(&mut self) {
        let bytes = std::mem::take(&mut self.pending);
        if bytes > 0 {
            emit(
                self.events,
                EventKind::IoActivity {
                    verification_bytes: if self.versions { 0 } else { bytes },
                    version_bytes: if self.versions { bytes } else { 0 },
                },
            );
        }
    }
}
impl Drop for IoObserver<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Signale la fin du transfert d'un fichier, quelle que soit la sortie.
struct TransferEnded<'a> {
    events: Option<&'a EventEmitter>,
    operation: &'a OperationId,
}
impl Drop for TransferEnded<'_> {
    fn drop(&mut self) {
        if self.events.is_some() {
            emit(
                self.events,
                EventKind::TransferEnded {
                    operation: self.operation.clone(),
                },
            );
        }
    }
}

/// Avance la progression d'un bloc transféré ; émet au plus un événement
/// par intervalle d'octets.
fn advance(
    progress: &mut SharedProgress,
    events: Option<&EventEmitter>,
    interval: u64,
    bytes: u64,
) {
    let mut progress = progress
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    progress.bytes_done = progress.bytes_done.saturating_add(bytes);
    if progress.bytes_done - progress.bytes_at_last_event >= interval {
        progress.bytes_at_last_event = progress.bytes_done;
        emit(events, EventKind::Progress(progress.snapshot()));
    }
}

fn stop_for(kind: StorageErrorKind, side: PlanSide) -> Option<StopCause> {
    match kind {
        StorageErrorKind::Cancelled => Some(StopCause::Cancelled),
        StorageErrorKind::InsufficientSpace => Some(StopCause::InsufficientSpace),
        StorageErrorKind::Disconnected => Some(match side {
            PlanSide::Source => StopCause::SourceUnavailable,
            PlanSide::Destination => StopCause::DestinationUnavailable,
        }),
        _ => None,
    }
}

pub(crate) struct Run<'a> {
    req: ExecutionRequest<'a>,
    context: ErrorContext,
    outcomes: Vec<Option<OutcomeStatus>>,
    index: HashMap<&'a OperationId, usize>,
    stop: Option<StopCause>,
    deletions_blocked: bool,
    progress: SharedProgress,
    leftovers: Vec<ResourcePath>,
    independent: bool,
}

impl<'a> Run<'a> {
    fn start(
        req: ExecutionRequest<'a>,
        journal: Option<&Mutex<&mut dyn sync_journal::JournalSink>>,
        workers: usize,
    ) -> Result<ExecutionReport, ExecutionError> {
        let plan = req.plan;
        let summary = plan.summary();
        req.journal
            .append(JournalRecord::ExecutionStarted {
                execution: req.execution_id.clone(),
                plan: plan.id().clone(),
                profile: plan.profile().cloned(),
                operations: plan.operations().len() as u64,
                mutations: summary.mutations(),
                source: plan.source().location().to_owned(),
                destination: plan.destination().location().to_owned(),
                versions: req.versions.map(|store| store.folder().clone()),
            })
            .map_err(ExecutionError::Journal)?;
        let mut context = ErrorContext::new().with_execution(req.execution_id.clone());
        if let Some(profile) = plan.profile() {
            context = context.with_profile(profile.clone());
        }
        let operations = plan.operations();
        let mut run = Run {
            context,
            outcomes: vec![None; operations.len()],
            index: operations
                .iter()
                .enumerate()
                .map(|(index, op)| (op.id(), index))
                .collect(),
            stop: None,
            deletions_blocked: false,
            progress: SharedProgress(Arc::new(Mutex::new(Progress {
                entries_total: summary.mutations(),
                bytes_total: (summary.unknown_sizes == 0).then_some(summary.bytes_to_transfer),
                ..Progress::default()
            }))),
            leftovers: Vec::new(),
            independent: false,
            req,
        };
        emit(
            run.req.events,
            EventKind::StatusChanged {
                status: ExecutionStatus::Running,
            },
        );
        run.progress.publish(run.req.events);

        // Copies, créations, remplacements : dans l'ordre des chemins.
        if let Some(journal) = journal {
            parallel::execute(&mut run, journal, workers);
        } else {
            for (index, op) in operations.iter().enumerate() {
                if op.action() != Action::Delete {
                    run.visit(index, op);
                }
            }
        }
        // Suppressions : en dernier, du plus profond au moins profond.
        if summary.delete > 0 && run.stop.is_none() {
            run.check_source_before_deletions();
        }
        for (index, op) in operations.iter().enumerate().rev() {
            if op.action() == Action::Delete {
                run.visit(index, op);
            }
        }
        Ok(run.finish())
    }

    fn ctx(&self, op: &Operation) -> ErrorContext {
        self.context
            .clone()
            .with_operation(op.id().clone())
            .with_path(op.path().clone())
    }

    fn fail(
        &self,
        op: &Operation,
        error: &dyn ClassifiedError,
        stop: Option<StopCause>,
    ) -> Interrupt {
        Interrupt::Failed {
            report: Box::new(ErrorReport::capture(error, Severity::Error, &self.ctx(op))),
            stop,
        }
    }

    fn storage(&self, op: &Operation, error: &StorageError, side: PlanSide) -> Interrupt {
        if error.kind() == StorageErrorKind::Cancelled {
            return Interrupt::Cancelled;
        }
        self.fail(op, error, stop_for(error.kind(), side))
    }

    fn journal_failure(&self, op: &Operation, error: &JournalError) -> Interrupt {
        self.fail(op, error, Some(StopCause::JournalFailure))
    }

    fn visit(&mut self, index: usize, op: &'a Operation) {
        let reverse = op.target() == PlanSide::Source;
        if reverse {
            std::mem::swap(&mut self.req.source, &mut self.req.destination);
        }
        let status = self.status_of(op);
        if reverse {
            std::mem::swap(&mut self.req.source, &mut self.req.destination);
        }
        let mutation = op.action().is_mutation();
        if mutation && !matches!(status, OutcomeStatus::NotStarted) {
            self.progress.completed();
            self.report_outcome(op, &status);
        }
        if let Some(slot) = self.outcomes.get_mut(index) {
            *slot = Some(status);
        }
    }

    fn status_of(&mut self, op: &'a Operation) -> OutcomeStatus {
        if !op.action().is_mutation() {
            return OutcomeStatus::NoAction;
        }
        if self.stop.is_some() {
            return OutcomeStatus::NotStarted;
        }
        if self.req.cancel.checkpoint() {
            self.stop = Some(StopCause::Cancelled);
            return OutcomeStatus::NotStarted;
        }
        if !self.independent
            && let Some(dependency) = op
                .depends_on()
                .iter()
                .find(|dependency| !self.is_done(dependency))
        {
            return OutcomeStatus::Refused {
                refusal: Refusal::DependencyNotDone {
                    operation: dependency.clone(),
                },
            };
        }
        if self.req.plan.mode() == sync_plan::SyncMode::Bidirectional
            && let Err(error) = self.req.journal.append(JournalRecord::OperationDirection {
                operation: op.id().clone(),
                target: op.target(),
            })
        {
            self.stop = Some(StopCause::JournalFailure);
            self.diagnostic(&error, Severity::Fatal, op);
            return OutcomeStatus::NotStarted;
        }
        if let Err(error) = self.req.journal.append(JournalRecord::OperationStarted {
            operation: op.id().clone(),
            path: op.path().clone(),
            action: op.action(),
        }) {
            self.stop = Some(StopCause::JournalFailure);
            self.diagnostic(&error, Severity::Fatal, op);
            return OutcomeStatus::NotStarted;
        }
        let result = match op.action() {
            Action::Copy => self.copy(op),
            Action::Replace => self.replace(op),
            Action::Delete => self.delete(op),
            _ => Err(self.fail(op, &Fault::UnsupportedKind(op.kind()), None)),
        };
        match result {
            Ok((bytes, version)) => OutcomeStatus::Done { bytes, version },
            Err(Interrupt::Refused(refusal)) => OutcomeStatus::Refused { refusal },
            Err(Interrupt::Cancelled) => {
                self.stop = Some(StopCause::Cancelled);
                OutcomeStatus::Cancelled
            }
            Err(Interrupt::Failed { report, stop }) => {
                if stop.is_some() && self.stop.is_none() {
                    self.stop = stop;
                }
                if stop == Some(StopCause::SourceUnavailable) {
                    self.deletions_blocked = true;
                }
                OutcomeStatus::Failed { error: *report }
            }
        }
    }

    fn is_done(&self, id: &OperationId) -> bool {
        self.index
            .get(id)
            .and_then(|&index| self.outcomes.get(index))
            .is_some_and(|status| matches!(status, Some(OutcomeStatus::Done { .. })))
    }

    /// Consigne l'issue, émet diagnostic et progression.
    fn report_outcome(&mut self, op: &Operation, status: &OutcomeStatus) {
        let (bytes, error_class, detail) = match status {
            OutcomeStatus::Done { bytes, .. } => (*bytes, None, None),
            OutcomeStatus::Refused { refusal } => {
                self.diagnostic(&RefusalNotice(refusal.clone()), Severity::Warning, op);
                (0, None, serde_json::to_string(refusal).ok())
            }
            OutcomeStatus::Failed { error } => {
                emit(self.req.events, EventKind::Diagnostic(error.clone()));
                (0, Some(error.class()), Some(error.message().to_owned()))
            }
            _ => (0, None, None),
        };
        if let Err(error) = self.req.journal.append(JournalRecord::OperationFinished {
            operation: op.id().clone(),
            path: op.path().clone(),
            action: op.action(),
            outcome: status.code(),
            bytes,
            error_class,
            detail,
        }) {
            if self.stop.is_none() {
                self.stop = Some(StopCause::JournalFailure);
            }
            self.diagnostic(&error, Severity::Fatal, op);
        }
        self.progress.publish(self.req.events);
    }

    fn diagnostic(&self, error: &dyn ClassifiedError, severity: Severity, op: &Operation) {
        emit(
            self.req.events,
            EventKind::Diagnostic(ErrorReport::capture(error, severity, &self.ctx(op))),
        );
    }

    // --- Préconditions -------------------------------------------------------

    fn snapshot<'s>(
        &self,
        op: &Operation,
        snapshot: Option<&'s EntrySnapshot>,
    ) -> Result<&'s EntrySnapshot, Interrupt> {
        snapshot.ok_or_else(|| self.fail(op, &Fault::MissingObservation, None))
    }

    /// `source_unchanged` : la source correspond encore à l'observation.
    fn verify_source(&self, op: &Operation) -> Result<(), Interrupt> {
        let snapshot = self.snapshot(op, op.source())?;
        self.verify_source_snapshot(op, snapshot)
    }

    fn verify_source_snapshot(
        &self,
        op: &Operation,
        snapshot: &EntrySnapshot,
    ) -> Result<(), Interrupt> {
        match self.req.source.stat(op.source_path(), self.req.cancel) {
            Ok(entry) => {
                compare_observation(snapshot.kind(), snapshot.metadata(), &entry)
                    .map_err(|mismatch| Interrupt::Refused(Refusal::SourceChanged { mismatch }))?;
                self.verify_hash(op, snapshot, self.req.source, true)
            }
            Err(error) if error.kind() == StorageErrorKind::NotFound => {
                Err(Interrupt::Refused(Refusal::SourceVanished))
            }
            Err(error) => Err(self.storage(op, &error, PlanSide::Source)),
        }
    }

    fn transfer_source(&self, op: &Operation) -> Result<EntrySnapshot, Interrupt> {
        let snapshot = self.snapshot(op, op.source())?;
        if self.req.options.refresh_mirror_source
            && self.req.plan.mode() == sync_plan::SyncMode::Mirror
            && snapshot.kind() == EntryKind::File
        {
            let entry = self
                .req
                .source
                .stat(op.source_path(), self.req.cancel)
                .map_err(|error| {
                    if error.kind() == StorageErrorKind::NotFound {
                        Interrupt::Refused(Refusal::SourceVanished)
                    } else {
                        self.storage(op, &error, PlanSide::Source)
                    }
                })?;
            if entry.kind() != EntryKind::File
                || entry.metadata().size().is_none()
                || entry.metadata().modified().is_none()
            {
                return Err(Interrupt::Refused(Refusal::SourceChanged {
                    mismatch: Mismatch::Unverifiable,
                }));
            }
            return Ok(EntrySnapshot::new(entry.kind(), entry.metadata().clone()));
        }
        self.verify_source_snapshot(op, snapshot)?;
        Ok(snapshot.clone())
    }

    /// `destination_absent` : rien n'est apparu à la destination.
    fn verify_destination_absent(&self, op: &Operation) -> Result<(), Interrupt> {
        match self.req.destination.stat(op.path(), self.req.cancel) {
            Err(error) if error.kind() == StorageErrorKind::NotFound => Ok(()),
            Ok(_) => Err(Interrupt::Refused(Refusal::DestinationAppeared)),
            Err(error) => Err(self.storage(op, &error, PlanSide::Destination)),
        }
    }

    /// `destination_unchanged` : la destination correspond encore à
    /// l'observation.
    fn verify_destination_unchanged(&self, op: &Operation) -> Result<(), Interrupt> {
        let snapshot = self.snapshot(op, op.destination())?;
        match self.req.destination.stat(op.path(), self.req.cancel) {
            Ok(entry) => {
                compare_observation(snapshot.kind(), snapshot.metadata(), &entry).map_err(
                    |mismatch| Interrupt::Refused(Refusal::DestinationChanged { mismatch }),
                )?;
                self.verify_hash(op, snapshot, self.req.destination, false)
            }
            Err(error) if error.kind() == StorageErrorKind::NotFound => {
                Err(Interrupt::Refused(Refusal::DestinationVanished))
            }
            Err(error) => Err(self.storage(op, &error, PlanSide::Destination)),
        }
    }

    /// Avant la première suppression : la source doit être lisible.
    fn verify_hash(
        &self,
        op: &Operation,
        snapshot: &EntrySnapshot,
        connector: &dyn connector_api::StorageConnector,
        source: bool,
    ) -> Result<(), Interrupt> {
        if self.req.plan.mode() != sync_plan::SyncMode::Bidirectional
            || snapshot.kind() != EntryKind::File
        {
            return Ok(());
        }
        let (Some(hash), Some(size)) = (snapshot.metadata().hash(), snapshot.metadata().size())
        else {
            return Err(Interrupt::Refused(if source {
                Refusal::SourceChanged {
                    mismatch: Mismatch::Unverifiable,
                }
            } else {
                Refusal::DestinationChanged {
                    mismatch: Mismatch::Unverifiable,
                }
            }));
        };
        verify_content(
            connector,
            op.path(),
            hash,
            size,
            self.req.options.buffer_size,
            self.req.cancel,
        )
        .map_err(|error| match error {
            VerifyError::Mismatch(mismatch) => Interrupt::Refused(if source {
                Refusal::SourceChanged { mismatch }
            } else {
                Refusal::DestinationChanged { mismatch }
            }),
            VerifyError::Storage(error) => self.storage(
                op,
                &error,
                if source {
                    PlanSide::Source
                } else {
                    PlanSide::Destination
                },
            ),
            other => self.fail(op, &other, None),
        })
    }

    fn check_source_before_deletions(&mut self) {
        let root = ResourcePath::root();
        for connector in [self.req.source, self.req.destination] {
            match connector.stat(&root, self.req.cancel) {
                Ok(entry) if entry.kind() == EntryKind::Directory => {}
                _ => self.deletions_blocked = true,
            }
        }
    }

    // --- Transfert -------------------------------------------------------------

    /// Abandonne un dépôt ; un échec du retrait est rapporté (fichier restant).
    fn abort(&mut self, staged: StagedCopy, op: &Operation) {
        let staging = staged.staging_path().cloned();
        if let Err(error) = staged.abort() {
            self.diagnostic(&error, Severity::Error, op);
            if let Some(staging) = staging {
                self.leftovers.push(staging);
            }
        }
    }

    fn copy_failure(&mut self, op: &Operation, error: &CopyError) -> Interrupt {
        if error.cleanup_error().is_some()
            && let Some(staging) = error.staging_path()
        {
            self.leftovers.push(staging.clone());
        }
        let side = match error.stage() {
            CopyStage::OpenSource | CopyStage::Read => PlanSide::Source,
            _ => PlanSide::Destination,
        };
        if error.kind() == StorageErrorKind::Cancelled {
            return Interrupt::Cancelled;
        }
        self.fail(op, error, stop_for(error.kind(), side))
    }

    /// Transfère dans un dépôt temporaire puis contrôle : taille et
    /// empreinte attendues, source inchangée après lecture, relecture du
    /// dépôt (mode `content`) par l'accès du writer lorsque le connecteur le
    /// permet, sinon en le rouvrant par son chemin.
    fn transfer(
        &mut self,
        op: &Operation,
        source: &EntrySnapshot,
    ) -> Result<StagedCopy, Interrupt> {
        let options = WriteOptions::new()
            .with_modified(source.metadata().modified())
            .with_size_hint(source.metadata().size());
        let live_mirror = self.req.options.refresh_mirror_source
            && self.req.plan.mode() == sync_plan::SyncMode::Mirror;
        let mut request = CopyRequest::new(
            self.req.source,
            op.source_path(),
            self.req.destination,
            op.path(),
        )
        .with_options(options)
        .with_buffer_size(self.req.options.buffer_size);
        if live_mirror && let Some(bytes) = source.metadata().size() {
            request = request.with_read_limit(bytes);
        }
        let events = self.req.events;
        let interval = self.req.options.progress_interval_bytes;
        // Per-file presentation, opt-in: start, every interval, end (guard).
        let file_events = events.filter(|_| self.req.options.transfer_events);
        let file_progress = |bytes_done| EventKind::TransferProgress {
            operation: op.id().clone(),
            path: op.path().clone(),
            bytes_done,
            bytes_total: source.metadata().size(),
        };
        if file_events.is_some() {
            emit(file_events, file_progress(0));
        }
        let _ended = TransferEnded {
            events: file_events,
            operation: op.id(),
        };
        let (mut file_bytes, mut file_reported) = (0_u64, 0_u64);
        let progress = &mut self.progress;
        let mut journal_error = None;
        let journal = &mut self.req.journal;
        let result = stage_copy_observed(
            &request,
            &mut |bytes| {
                advance(progress, events, interval, bytes);
                file_bytes = file_bytes.saturating_add(bytes);
                if file_events.is_some() && file_bytes - file_reported >= interval {
                    file_reported = file_bytes;
                    emit(file_events, file_progress(file_bytes));
                }
            },
            &mut |staging| {
                if let Some(staging) = staging
                    && let Err(error) = journal.append(JournalRecord::StagingCreated {
                        operation: op.id().clone(),
                        staging: staging.clone(),
                    })
                {
                    journal_error = Some(error);
                    return Err(StorageError::new(
                        StorageErrorKind::Permanent,
                        connector_api::StorageOperation::Write,
                    )
                    .with_message("staging journal unavailable"));
                }
                Ok(())
            },
            self.req.cancel,
        );
        let staged = match result {
            Ok(staged) => staged,
            Err(error) => {
                let failure = self.copy_failure(op, &error);
                return Err(match journal_error {
                    Some(error) => self.journal_failure(op, &error),
                    None => failure,
                });
            }
        };
        if let Err(mismatch) = check_transfer(source.metadata(), staged.bytes(), staged.hash()) {
            self.abort(staged, op);
            return Err(Interrupt::Refused(Refusal::SourceChangedDuringTransfer {
                mismatch,
            }));
        }
        // La source a-t-elle changé pendant la lecture ?
        if let Err(interrupt) = self.verify_source_snapshot(op, source) {
            let failure = match interrupt {
                Interrupt::Refused(Refusal::SourceChanged { .. }) if live_mirror => {
                    self.verify_live_prefix(op, &staged).err()
                }
                Interrupt::Refused(Refusal::SourceChanged { mismatch }) => {
                    Some(Interrupt::Refused(Refusal::SourceChangedDuringTransfer {
                        mismatch,
                    }))
                }
                other => Some(other),
            };
            if let Some(failure) = failure {
                self.abort(staged, op);
                return Err(failure);
            }
        }
        let mut staged = staged;
        let reread = if self.req.options.verify == VerifyMode::Content {
            let mut observer = IoObserver {
                events: self.req.events,
                pending: 0,
                versions: false,
            };
            match staged.reread_observed(self.req.options.buffer_size, &mut |bytes| {
                observer.advance(bytes)
            }) {
                Ok(reread) => reread,
                Err(error) => {
                    self.abort(staged, op);
                    return Err(self.storage(op, &error, PlanSide::Destination));
                }
            }
        } else {
            None
        };
        if let Some((bytes, hash)) = reread {
            if let Err(mismatch) = check_reread(staged.hash(), staged.bytes(), &hash, bytes) {
                self.abort(staged, op);
                return Err(self.fail(op, &Fault::StagedContent(mismatch), None));
            }
        } else if self.req.options.verify == VerifyMode::Content
            && let Some(staging) = staged.staging_path().cloned()
            && let Err(error) = verify_content(
                self.req.destination,
                &staging,
                staged.hash(),
                staged.bytes(),
                self.req.options.buffer_size,
                self.req.cancel,
            )
        {
            self.abort(staged, op);
            return Err(match error {
                VerifyError::Mismatch(mismatch) => {
                    self.fail(op, &Fault::StagedContent(mismatch), None)
                }
                VerifyError::Storage(error) => self.storage(op, &error, PlanSide::Destination),
                other => self.fail(op, &other, None),
            });
        }
        Ok(staged)
    }

    fn verify_live_prefix(&self, op: &Operation, staged: &StagedCopy) -> Result<(), Interrupt> {
        let entry = self
            .req
            .source
            .stat(op.source_path(), self.req.cancel)
            .map_err(|error| self.storage(op, &error, PlanSide::Source))?;
        if entry.kind() != EntryKind::File
            || entry
                .metadata()
                .size()
                .is_none_or(|size| size < staged.bytes())
        {
            return Err(Interrupt::Refused(Refusal::SourceChangedDuringTransfer {
                mismatch: Mismatch::Unverifiable,
            }));
        }
        match verify_prefix(
            self.req.source,
            op.source_path(),
            staged.hash(),
            staged.bytes(),
            self.req.options.buffer_size,
            self.req.cancel,
        ) {
            Ok(()) => Ok(()),
            Err(VerifyError::Mismatch(mismatch)) => {
                Err(Interrupt::Refused(Refusal::SourceChangedDuringTransfer {
                    mismatch,
                }))
            }
            Err(VerifyError::Storage(error)) => Err(self.storage(op, &error, PlanSide::Source)),
            Err(other) => Err(self.fail(op, &other, None)),
        }
    }

    /// Valide un dépôt et contrôle l'entrée publiée.
    fn publish(
        &mut self,
        op: &Operation,
        staged: StagedCopy,
        mode: CommitMode,
    ) -> Result<u64, Interrupt> {
        if self.req.cancel.checkpoint() {
            self.abort(staged, op);
            return Err(Interrupt::Cancelled);
        }
        let bytes = staged.bytes();
        let hash = staged.hash().clone();
        let had_staging = staged.staging_path().is_some();
        let entry = match staged.commit(mode) {
            Ok(entry) => entry,
            Err(error) if error.kind() == StorageErrorKind::Conflict => {
                return Err(Interrupt::Refused(match mode {
                    CommitMode::CreateNew => Refusal::DestinationAppeared,
                    _ => Refusal::DestinationVanished,
                }));
            }
            Err(error) => return Err(self.storage(op, &error, PlanSide::Destination)),
        };
        check_written(&entry, bytes)
            .map_err(|mismatch| self.fail(op, &Fault::PublishedEntry(mismatch), None))?;
        // Sans dépôt relisible, la relecture se fait après publication.
        if self.req.options.verify == VerifyMode::Content && !had_staging {
            match verify_content(
                self.req.destination,
                op.path(),
                &hash,
                bytes,
                self.req.options.buffer_size,
                self.req.cancel,
            ) {
                Ok(()) => {}
                Err(VerifyError::Mismatch(mismatch)) => {
                    return Err(self.fail(op, &Fault::PublishedEntry(mismatch), None));
                }
                Err(error) => return Err(self.fail(op, &error, None)),
            }
        }
        Ok(bytes)
    }

    // --- Actions ---------------------------------------------------------------

    fn copy(&mut self, op: &Operation) -> Result<Done, Interrupt> {
        match op.kind() {
            EntryKind::Directory => {
                self.verify_source(op)?;
                self.verify_destination_absent(op)?;
                match self
                    .req
                    .destination
                    .create_directory(op.path(), self.req.cancel)
                {
                    Ok(_) => Ok((0, None)),
                    Err(error) if error.kind() == StorageErrorKind::Conflict => {
                        Err(Interrupt::Refused(Refusal::DestinationAppeared))
                    }
                    Err(error) => Err(self.storage(op, &error, PlanSide::Destination)),
                }
            }
            EntryKind::File => {
                let source = self.transfer_source(op)?;
                self.verify_destination_absent(op)?;
                let staged = self.transfer(op, &source)?;
                // Juste avant la publication.
                if let Err(interrupt) = self.verify_destination_absent(op) {
                    self.abort(staged, op);
                    return Err(interrupt);
                }
                let bytes = self.publish(op, staged, CommitMode::CreateNew)?;
                Ok((bytes, None))
            }
            other => Err(self.fail(op, &Fault::UnsupportedKind(other), None)),
        }
    }

    fn replace(&mut self, op: &Operation) -> Result<Done, Interrupt> {
        if op.kind() != EntryKind::File {
            return Err(self.fail(op, &Fault::UnsupportedKind(op.kind()), None));
        }
        let source = self.transfer_source(op)?;
        let destination = self.snapshot(op, op.destination())?;
        self.verify_destination_unchanged(op)?;
        let staged = self.transfer(op, &source)?;
        let version = match self.preserve(op, destination) {
            Ok(version) => version,
            Err(interrupt) => {
                self.abort(staged, op);
                return Err(interrupt);
            }
        };
        // Juste avant la publication.
        if let Err(interrupt) = self.verify_destination_unchanged(op) {
            self.abort(staged, op);
            return Err(interrupt);
        }
        let bytes = self.publish(op, staged, CommitMode::ReplaceExisting)?;
        Ok((bytes, version))
    }

    fn verify_source_absent(&mut self, op: &Operation) -> Result<(), Interrupt> {
        let mut candidates = vec![op.path().clone()];
        let operations = self.req.plan.operations();
        for parent in std::iter::successors(op.path().parent(), |p| p.parent()) {
            let Some(source) = operations
                .binary_search_by(|entry| entry.path().cmp(&parent))
                .ok()
                .and_then(|index| operations[index].source())
            else {
                continue;
            };
            if source.origin_paths().is_empty() {
                continue;
            }
            candidates.clear();
            for origin in source.origin_paths() {
                let candidate = ResourcePath::from_segments(
                    origin
                        .segments()
                        .chain(op.path().segments().skip(parent.depth())),
                )
                .map_err(|_| self.fail(op, &Fault::MissingObservation, None))?;
                candidates.push(candidate);
            }
            break;
        }
        for candidate in candidates {
            self.verify_source_path_absent(op, &candidate)?;
        }
        Ok(())
    }

    fn verify_source_path_absent(
        &mut self,
        op: &Operation,
        path: &ResourcePath,
    ) -> Result<(), Interrupt> {
        // `source_absent` : never inferred from a permission/network failure.
        match self.req.source.stat(path, self.req.cancel) {
            Err(error) if error.kind() == StorageErrorKind::NotFound => {}
            Ok(_) => return Err(Interrupt::Refused(Refusal::SourceReappeared)),
            Err(error) => {
                self.deletions_blocked = true;
                if matches!(
                    error.kind(),
                    StorageErrorKind::Disconnected | StorageErrorKind::Cancelled
                ) {
                    return Err(self.storage(op, &error, PlanSide::Source));
                }
                return Err(Interrupt::Refused(Refusal::SourceUnverifiable {
                    class: error.kind().class(),
                }));
            }
        }
        Ok(())
    }

    fn delete(&mut self, op: &Operation) -> Result<Done, Interrupt> {
        if self.deletions_blocked {
            return Err(Interrupt::Refused(Refusal::DeletionsBlocked));
        }
        let destination = self.snapshot(op, op.destination())?;
        self.verify_source_absent(op)?;
        self.verify_destination_unchanged(op)?;
        let version = if op.kind() == EntryKind::File {
            let version = self.preserve(op, destination)?;
            if version.is_some() {
                // Archiving a large old file can take seconds: the source may
                // have reappeared while the version was being preserved.
                self.verify_source_absent(op)?;
                self.verify_destination_unchanged(op)?;
            }
            version
        } else {
            None
        };
        match self
            .req
            .destination
            .delete(op.path(), op.kind(), self.req.cancel)
        {
            Ok(()) => Ok((0, version)),
            Err(error) if error.kind() == StorageErrorKind::Conflict => {
                Err(Interrupt::Refused(if op.kind() == EntryKind::Directory {
                    Refusal::DirectoryNotEmpty
                } else {
                    Refusal::DestinationChanged {
                        mismatch: Mismatch::Unverifiable,
                    }
                }))
            }
            Err(error) if error.kind() == StorageErrorKind::NotFound => {
                Err(Interrupt::Refused(Refusal::DestinationVanished))
            }
            Err(error) => Err(self.storage(op, &error, PlanSide::Destination)),
        }
    }

    /// Conserve la version actuelle de la destination, si demandé.
    fn preserve(
        &mut self,
        op: &Operation,
        destination: &EntrySnapshot,
    ) -> Result<Option<ResourcePath>, Interrupt> {
        let Some(store) = self.req.versions else {
            return Ok(None);
        };
        let mut observer = IoObserver {
            events: self.req.events,
            pending: 0,
            versions: true,
        };
        match store.preserve_observed(
            self.req.destination,
            op.path(),
            destination.metadata(),
            self.req.cancel,
            &mut |bytes| observer.advance(bytes),
        ) {
            Ok(version) => {
                let location = version.location().clone();
                if let Err(error) = self.req.journal.append(JournalRecord::VersionPreserved {
                    operation: op.id().clone(),
                    location: location.clone(),
                }) {
                    return Err(self.journal_failure(op, &error));
                }
                Ok(Some(location))
            }
            Err(VersioningError::Changed(mismatch)) => {
                Err(Interrupt::Refused(Refusal::DestinationChanged { mismatch }))
            }
            Err(error) => {
                let kind = match &error {
                    VersioningError::Store(error) => Some(error.kind()),
                    VersioningError::Copy(error) => Some(error.kind()),
                    _ => None,
                };
                if kind == Some(StorageErrorKind::Cancelled) {
                    return Err(Interrupt::Cancelled);
                }
                // Magasin plein ou déconnecté : rien ne peut plus être
                // remplacé ni supprimé en sécurité.
                let stop = match kind {
                    Some(StorageErrorKind::InsufficientSpace) => Some(StopCause::InsufficientSpace),
                    Some(StorageErrorKind::Disconnected) => Some(StopCause::DestinationUnavailable),
                    _ => None,
                };
                Err(self.fail(op, &error, stop))
            }
        }
    }

    // --- Fin -------------------------------------------------------------------

    fn finish(mut self) -> ExecutionReport {
        let plan = self.req.plan;
        let mut totals = JournalTotals::default();
        let mut outcomes = Vec::with_capacity(plan.operations().len());
        for (op, status) in plan.operations().iter().zip(self.outcomes.drain(..)) {
            let status = status.unwrap_or(OutcomeStatus::NotStarted);
            match &status {
                OutcomeStatus::Done { bytes, .. } => {
                    totals.done += 1;
                    totals.bytes = totals.bytes.saturating_add(*bytes);
                }
                OutcomeStatus::NoAction => totals.no_action += 1,
                OutcomeStatus::Refused { .. } => totals.refused += 1,
                OutcomeStatus::Failed { .. } => totals.failed += 1,
                OutcomeStatus::Cancelled => totals.cancelled += 1,
                _ => totals.not_started += 1,
            }
            outcomes.push(OperationOutcome {
                operation: op.id().clone(),
                target: op.target(),
                path: op.path().clone(),
                action: op.action(),
                status,
            });
        }
        let mut status = if self.stop == Some(StopCause::Cancelled) {
            ExecutionStatus::Cancelled
        } else if self.stop.is_some() || totals.failed > 0 {
            ExecutionStatus::Failed
        } else if totals.refused > 0 || plan.needs_attention() || !self.leftovers.is_empty() {
            ExecutionStatus::SucceededWithWarnings
        } else {
            ExecutionStatus::Succeeded
        };
        if let Err(error) = self
            .req
            .journal
            .append(JournalRecord::ExecutionFinished { status, totals })
        {
            emit(
                self.req.events,
                EventKind::Diagnostic(ErrorReport::capture(&error, Severity::Fatal, &self.context)),
            );
            if self.stop.is_none() {
                self.stop = Some(StopCause::JournalFailure);
            }
            if status != ExecutionStatus::Cancelled {
                status = ExecutionStatus::Failed;
            }
        }
        self.progress.publish(self.req.events);
        emit(self.req.events, EventKind::StatusChanged { status });
        ExecutionReport {
            execution: self.req.execution_id.clone(),
            plan: plan.id().clone(),
            profile: plan.profile().cloned(),
            status,
            stop: self.stop,
            totals,
            outcomes,
            leftovers: self.leftovers,
        }
    }
}
