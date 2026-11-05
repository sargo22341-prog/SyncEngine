//! Dependency-aware bounded workers for independent files and directories.
//! The coordinator owns outcomes. Deletions start after all workers join.
use super::*;
use crate::options::LARGE_TRANSFER_BYTES;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::mpsc;
use sync_journal::JournalSink;

pub(super) struct JournalProxy<'s, 'j>(pub(super) &'s Mutex<&'j mut dyn JournalSink>);
impl JournalSink for JournalProxy<'_, '_> {
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError> {
        self.0
            .lock()
            .map_err(|_| JournalError::Io(std::io::Error::other("shared journal poisoned")))?
            .append(record)
    }
}

/// Threads de l'exécution et part qui peut copier en même temps.
///
/// `copy_slots` borne les opérations en phase de **copie** (lecture de la
/// source, écriture du dépôt). Une copie finie cède son emplacement avant
/// d'être relue et publiée : jusqu'à [`VERIFY_THREADS`] threads de plus
/// vérifient les fichiers déjà copiés pendant que la copie suivante avance,
/// de sorte que la relecture d'un très gros fichier ne suspend pas le débit.
#[derive(Clone, Copy, Debug)]
pub(super) struct Pool {
    pub(super) copy_slots: usize,
    pub(super) threads: usize,
}

/// Vérifications simultanées derrière les copies.
const VERIFY_THREADS: usize = 2;

impl Pool {
    pub(super) fn of(req: &ExecutionRequest<'_>) -> Self {
        // Plafond imposé par les deux connecteurs et le magasin de versions.
        let bound = req
            .source
            .transfer_concurrency()
            .get()
            .min(req.destination.transfer_concurrency().get())
            .min(
                req.versions
                    .map_or(16, |store| store.transfer_concurrency().get()),
            )
            .min(16);
        // Le bidirectionnel reste ordonné : une seule copie à la fois, mais
        // sa vérification peut chevaucher la copie suivante.
        let copy_slots = if req.plan.mode() == sync_plan::SyncMode::Bidirectional {
            1
        } else {
            req.options.copy_workers.get()
        }
        .min(bound);
        let extra = if req.options.verify == VerifyMode::Content {
            VERIFY_THREADS
        } else {
            0
        };
        Self {
            copy_slots,
            threads: (copy_slots + extra).min(bound),
        }
    }
}

/// Messages des workers vers le coordinateur.
enum Report {
    /// La copie de l'opération est terminée ; la vérification continue.
    Staged(usize),
    Done {
        worker: usize,
        index: usize,
        result: (OutcomeStatus, Option<StopCause>, Vec<ResourcePath>),
    },
}

pub(super) fn execute(run: &mut Run<'_>, journal: &Mutex<&mut dyn JournalSink>, pool: Pool) {
    let (workers, copy_slots) = (pool.threads, pool.copy_slots);
    let operations = run.req.plan.operations();
    let mut waiting = vec![0_usize; operations.len()];
    let mut children = vec![Vec::new(); operations.len()];
    let mut ready = BinaryHeap::new();
    for (index, op) in operations
        .iter()
        .enumerate()
        .filter(|(_, op)| op.action() != Action::Delete)
    {
        waiting[index] = op.depends_on().len();
        for dependency in op.depends_on() {
            if let Some(&parent) = run.index.get(dependency) {
                children[parent].push(index);
            }
        }
        if waiting[index] == 0 {
            ready.push(Reverse(index));
        }
    }
    let plan = run.req.plan;
    let source = run.req.source;
    let destination = run.req.destination;
    let cancel = run.req.cancel;
    let events = run.req.events;
    let options = run.req.options;
    let versions = run.req.versions;
    let now = run.req.now;
    let execution_id = run.req.execution_id.clone();
    let progress = run.progress.clone();
    let context = run.context.clone();
    let throttle = run.throttle.clone();
    std::thread::scope(|scope| {
        let (reply, receive) = mpsc::sync_channel(workers * 2);
        let mut senders = Vec::new();
        let mut handles = Vec::new();
        for worker in 0..workers {
            let (send, jobs) = mpsc::sync_channel::<usize>(1);
            let reply = reply.clone();
            let context = context.clone();
            let execution_id = execution_id.clone();
            let progress = progress.clone();
            let throttle = throttle.clone();
            handles.push(scope.spawn(move || {
                while let Ok(index) = jobs.recv() {
                    let op = &operations[index];
                    let staged = || {
                        // Un coordinateur parti n'a plus besoin du signal.
                        let _gone = reply.send(Report::Staged(index)).is_err();
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut proxy = JournalProxy(journal);
                        // Une opération vers la source lit la destination.
                        let (source, destination) = if op.target() == PlanSide::Source {
                            (destination, source)
                        } else {
                            (source, destination)
                        };
                        let req = ExecutionRequest {
                            plan,
                            source,
                            destination,
                            execution_id: execution_id.clone(),
                            journal: &mut proxy,
                            cancel,
                            options,
                            versions,
                            events,
                            now,
                        };
                        let mut task = Run {
                            req,
                            context: context.clone(),
                            outcomes: Vec::new(),
                            index: HashMap::new(),
                            stop: None,
                            deletions_blocked: false,
                            progress: progress.clone(),
                            leftovers: Vec::new(),
                            independent: true,
                            throttle: throttle.clone(),
                            staged: Some(&staged),
                        };
                        let status = task.status_of(op);
                        (status, task.stop, task.leftovers)
                    }));
                    let result = result.unwrap_or_else(|_| {
                        (
                            OutcomeStatus::Failed {
                                error: ErrorReport::capture(
                                    &Fault::WorkerFailed,
                                    Severity::Fatal,
                                    &context
                                        .clone()
                                        .with_operation(op.id().clone())
                                        .with_path(op.path().clone()),
                                ),
                            },
                            Some(StopCause::WorkerFailure),
                            Vec::new(),
                        )
                    });
                    if result.1.is_some() {
                        cancel.cancel();
                    }
                    if reply
                        .send(Report::Done {
                            worker,
                            index,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            senders.push(send);
        }
        drop(reply);
        let mut free: Vec<_> = (0..workers).collect();
        let mut busy = 0;
        // Large transfers wait for a slot; small ones keep the other workers.
        let large = |index: usize| {
            let op = &operations[index];
            op.kind() == EntryKind::File
                && op
                    .bytes()
                    .is_some_and(|bytes| bytes >= LARGE_TRANSFER_BYTES)
        };
        let large_slots = options.large_transfers.get();
        let mut large_busy = 0;
        // Opérations encore en phase de copie : elles occupent un
        // emplacement de copie (et de gros transfert) jusqu'à leur signal
        // `Staged` ou, à défaut, jusqu'à leur résultat.
        let mut copying = vec![false; operations.len()];
        let mut copy_busy = 0;
        let mut deferred = VecDeque::new();
        loop {
            if large_busy < large_slots {
                // Eligibility is checked again when they are popped.
                ready.extend(deferred.drain(..));
            }
            while !free.is_empty() && copy_busy < copy_slots {
                let Some(Reverse(index)) = ready.pop() else {
                    break;
                };
                let op = &operations[index];
                let eligible = run.stop.is_none()
                    && !cancel.is_cancelled()
                    && matches!(op.kind(), EntryKind::File | EntryKind::Directory)
                    && matches!(op.action(), Action::Copy | Action::Replace)
                    && op.depends_on().iter().all(|id| run.is_done(id));
                if eligible && large(index) && large_busy >= large_slots {
                    deferred.push_back(Reverse(index));
                } else if eligible {
                    let Some(worker) = free.pop() else {
                        break;
                    };
                    if senders[worker].send(index).is_err() {
                        run.stop = Some(StopCause::WorkerFailure);
                        cancel.cancel();
                        break;
                    }
                    busy += 1;
                    copying[index] = true;
                    copy_busy += 1;
                    if large(index) {
                        large_busy += 1;
                    }
                } else {
                    run.visit(index, op);
                    if run.stop.is_some() {
                        cancel.cancel();
                    }
                    release(index, &mut waiting, &children, &mut ready);
                }
            }
            if busy == 0 {
                break;
            }
            let Ok(report) = receive.recv() else {
                run.stop = Some(StopCause::WorkerFailure);
                cancel.cancel();
                break;
            };
            let (worker, index, (status, stop, leftovers)) = match report {
                Report::Staged(index) => {
                    if std::mem::take(&mut copying[index]) {
                        copy_busy -= 1;
                        if large(index) {
                            large_busy -= 1;
                        }
                    }
                    continue;
                }
                Report::Done {
                    worker,
                    index,
                    result,
                } => (worker, index, result),
            };
            busy -= 1;
            if std::mem::take(&mut copying[index]) {
                copy_busy -= 1;
                if large(index) {
                    large_busy -= 1;
                }
            }
            free.push(worker);
            // An origin failure takes precedence over sibling cancellations.
            if let Some(stop) = stop
                && (run.stop.is_none()
                    || (run.stop == Some(StopCause::Cancelled) && stop != StopCause::Cancelled))
            {
                run.stop = Some(stop);
            }
            run.leftovers.extend(leftovers);
            if !matches!(status, OutcomeStatus::NotStarted) {
                run.progress.completed();
                run.report_outcome(&operations[index], &status);
            }
            run.outcomes[index] = Some(status);
            if run.stop.is_some() {
                cancel.cancel();
            }
            release(index, &mut waiting, &children, &mut ready);
        }
        drop(senders);
        drop(receive);
        for handle in handles {
            if handle.join().is_err() {
                run.stop = Some(StopCause::WorkerFailure);
                cancel.cancel();
            }
        }
    });
    // Covers unavailable dependencies (including deletion dependencies) and
    // undispatched work after a fatal stop, using the ordinary refusal rules.
    for (index, op) in operations.iter().enumerate() {
        if op.action() != Action::Delete && run.outcomes[index].is_none() {
            run.visit(index, op);
        }
    }
}

fn release(
    index: usize,
    waiting: &mut [usize],
    children: &[Vec<usize>],
    ready: &mut BinaryHeap<Reverse<usize>>,
) {
    for &child in &children[index] {
        waiting[child] -= 1;
        if waiting[child] == 0 {
            ready.push(Reverse(child));
        }
    }
}
