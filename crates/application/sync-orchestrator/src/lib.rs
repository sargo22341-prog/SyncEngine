//! Blocking application facade. Call on a dedicated worker, never the UI
//! thread. One job per instance, an exclusive command lock on its state directory,
//! and a shared per-user execution lock across facade state directories.
//! Engines own every comparison, policy decision and file mutation.

mod endpoints;
mod live;
mod resident;
pub use resident::{
    ResidentPhase, ResidentStatus, agent_stop_pending, clear_agent_stop, hold_agent_presence,
    publish_resident_failure, publish_resident_status, request_agent_stop, resident_status,
};
mod pages;
mod runs;
mod scheduled;
mod schedules;

use connector_local::{LocalConnector, LocalOptions};
pub use live::{
    LIVE_FORMAT, LiveJob, LiveQueue, agent_cancel_marker, clear_agent_cancel, desktop_present,
    hold_desktop_presence, request_agent_cancel, request_agent_pause,
};
pub use runs::Run;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};
use sync_core::{
    CancellationToken, ExecutionId, HashAlgorithm, PlanId, ProfileId, ResourcePath, Timestamp,
};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_events::{EmitOutcome, Event, EventEmitter, EventKind, EventSink};
use sync_executor::{ExecutionOptions, ExecutionRequest, Executor, PlanExecutor};
use sync_filter::Filter;
use sync_history::{History, RetentionPolicy};
use sync_journal::{FileJournal, JournalError, JournalRecord, JournalSink};
use sync_plan::{
    DeletionGuard, DeletionPolicy, ExecutionPlan, OverwritePolicy, PlanPolicy, Reason, SyncMode,
};
use sync_profiles::{CompareMethod, Preferences, Profile, Profiles};
use sync_scanner::{Inventory, RECOMMENDED_HASH_WORKERS, ScanError, ScanOptions, ScanStats};
use sync_versioning::VersionStore;

/// Serializable, redacted IPC error. A failed execution may have partial work;
/// consult its durable journal/history rather than interpreting this as rollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppError {
    /// Stable category for presentation.
    pub code: String,
    /// Redacted diagnostic.
    pub message: String,
    /// Profile context when available.
    pub profile: Option<ProfileId>,
}
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for AppError {}
fn error(code: &str, message: impl std::fmt::Display) -> AppError {
    AppError {
        code: code.into(),
        message: sync_errors::redact_secrets(&message.to_string()),
        profile: None,
    }
}
fn io(error_value: impl std::fmt::Display) -> AppError {
    error("unavailable", error_value)
}
fn state_lock_error(cause: std::fs::TryLockError) -> AppError {
    match cause {
        std::fs::TryLockError::WouldBlock => error("busy", "une autre commande utilise cet état"),
        std::fs::TryLockError::Error(cause) => io(cause),
    }
}
fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>, AppError> {
    value
        .lock()
        .map_err(|_| error("internal", "état verrouillé après une panne ; redémarrer"))
}

/// Bounded latest observation. No event queue grows with transfer size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    /// No job has started in this process.
    #[default]
    Idle,
    /// Reading and hashing both trees, without mutation.
    Scanning,
    /// Executing the inspected plan and persisting results.
    Executing,
    /// Command finished; inspect the report for actual success/warnings/failure.
    Finished,
    /// Command returned an error; partial work may exist.
    Error,
}

/// Bounded latest observation, independent of durable execution truth.
/// Also published by the agent for other processes (see [`LiveJob`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobSnapshot {
    /// Unique identity of this command for cross-process pause requests.
    #[serde(default)]
    pub control_id: Option<String>,
    /// A cooperative pause was requested; in-flight system calls may finish.
    #[serde(default)]
    pub pause_requested: bool,
    /// True until the blocking command has completed, including final state writes.
    pub active: bool,
    /// Phase: idle, scanning, executing, finished, error.
    pub phase: JobPhase,
    /// Profile currently observed.
    pub profile: Option<ProfileId>,
    /// Current execution when real work has obtained an identity.
    pub execution: Option<ExecutionId>,
    /// Number of completed operation observations.
    pub entries_done: u64,
    /// Validated transferred bytes (updates between completed files).
    pub bytes_done: u64,
    /// Bytes read and staged so far, block by block, including the file in
    /// progress (about every [`LIVE_PROGRESS_BYTES`]). Basis of the live
    /// throughput; bytes of a transfer that later fails are not subtracted, so
    /// it can exceed `bytes_done`. Presentation only.
    pub bytes_transferred: u64,
    /// Entries observed during preview, including directories (both endpoints).
    pub entries_scanned: u64,
    /// Read/listing anomalies excluding consistency warnings. Unknown coverage
    /// never authorizes deletion; full issues remain in the inventory.
    pub scan_issues: u64,
    /// Directories with uncertain listing consistency, distinct from read failures.
    #[serde(default)]
    pub scan_consistency_warnings: u64,
    /// Content hashes computed during preview (both endpoints).
    pub entries_hashed: u64,
    /// Hashes this preview must compute when known in advance (files of equal
    /// size on both sides, content comparison); 0 when unknown or none.
    pub hash_target: u64,
    /// Failed/refused execution observations, after durable journal writes.
    pub operation_errors: u64,
    /// Planned mutations (copies, replacements, deletions) of the executed
    /// plan; 0 before execution. Lets an observer that did not see the preview
    /// compute an honest progress ratio.
    #[serde(default)]
    pub planned_operations: u64,
    /// Known bytes to transfer of the executed plan; 0 before execution.
    #[serde(default)]
    pub planned_bytes: u64,
}

/// Deletion ceilings applied by one preview of a profile that allows deletions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeletionCeiling {
    /// Default guards: empty source, incomplete inventories, and the relative
    /// ceiling (50 % of destination entries, from 10 deletions).
    #[default]
    Guarded,
    /// The user explicitly approved, for this preview only, deletions above the
    /// relative ceiling. Empty source and incomplete inventories still refuse
    /// every deletion; nothing is persisted, scheduled jobs always use
    /// [`DeletionCeiling::Guarded`], and execution still requires its own
    /// deletion confirmation. The lifted guard is recorded in the plan policy.
    Lifted,
}

impl DeletionCeiling {
    fn guard(self) -> DeletionGuard {
        match self {
            Self::Guarded => DeletionGuard::new(),
            // `None` is always a valid percentage; the unreachable error falls
            // back to the stricter default guard.
            Self::Lifted => DeletionGuard::new()
                .with_max_percent(None)
                .unwrap_or_default(),
        }
    }
}

struct Cached {
    profile: Profile,
    plan: ExecutionPlan,
}

/// Granularity of the block-level progress forwarded to [`JobSnapshot`]: four
/// 256 KiB copy buffers. Fine enough for a steady throughput curve on slow
/// links (1 MiB/s ⇒ one update per second), coarse enough to stay a few
/// hundred cheap atomic stores per second at disk speed.
pub const LIVE_PROGRESS_BYTES: u64 = 1024 * 1024;

/// Keeps only the latest transferred-byte count from executor progress
/// events: no queue, nothing grows with the transfer size.
struct LiveTransfer(Arc<AtomicU64>);
impl EventSink for LiveTransfer {
    fn emit(&self, event: Event) -> EmitOutcome {
        if let EventKind::Progress(progress) = event.kind() {
            self.0.fetch_max(progress.bytes_done, Relaxed);
        }
        EmitOutcome::Delivered
    }
}

/// Facade owner, safe to share with command workers. Never accepts a plan or
/// connector implementation from TypeScript. Store paths are application owned.
pub struct Application {
    state: PathBuf,
    lock_root: PathBuf,
    gate: Mutex<()>,
    cache: Mutex<Option<Cached>>,
    cancel: Mutex<CancellationToken>,
    snapshot: Mutex<JobSnapshot>,
    /// Live block-level byte count of the current execution (see
    /// [`JobSnapshot::bytes_transferred`]).
    transferred: Arc<AtomicU64>,
}
impl Application {
    /// Opens an existing state directory under an initialization lock and
    /// initializes versioned stores. Idle instances do not retain that lock.
    /// # Errors
    /// State path/lock/persistence error; concurrent commands are refused.
    pub fn open(state: impl AsRef<Path>) -> Result<Self, AppError> {
        Self::open_with_lock_root(state, scheduled::default_lock_root()?)
    }

    /// Opens with an explicit common lock directory. Every client on the same
    /// user account MUST use the same directory. Tests use isolated temporary roots.
    /// # Errors
    /// Same as `open`, or invalid/unavailable lock directory.
    pub fn open_with_lock_root(
        state: impl AsRef<Path>,
        lock_root: impl AsRef<Path>,
    ) -> Result<Self, AppError> {
        let state = state.as_ref().canonicalize().map_err(io)?;
        std::fs::create_dir_all(lock_root.as_ref()).map_err(io)?;
        let lock_root = lock_root.as_ref().canonicalize().map_err(io)?;
        if lock_root.starts_with(&state) {
            return Err(error(
                "invalid_input",
                "verrous communs hors de l’état requis",
            ));
        }
        let owner = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state.join("desktop.lock"))
            .map_err(io)?;
        owner.try_lock().map_err(state_lock_error)?;
        Profiles::open(state.join("profiles.sqlite")).map_err(io)?;
        runs::open(&state)?;
        History::open(state.join("state.sqlite")).map_err(io)?;
        Ok(Self {
            state,
            lock_root,
            gate: Mutex::new(()),
            cache: Mutex::new(None),
            cancel: Mutex::new(CancellationToken::new()),
            snapshot: Mutex::new(JobSnapshot::default()),
            transferred: Arc::new(AtomicU64::new(0)),
        })
    }

    fn gate(&self) -> Result<(MutexGuard<'_, ()>, File), AppError> {
        let gate = self.gate.try_lock().map_err(|cause| match cause {
            std::sync::TryLockError::WouldBlock => error("busy", "une commande est déjà en cours"),
            std::sync::TryLockError::Poisoned(_) => error(
                "internal",
                "commande interrompue par une panne ; redémarrer",
            ),
        })?;
        let owner = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.state.join("desktop.lock"))
            .map_err(io)?;
        owner.try_lock().map_err(state_lock_error)?;
        Ok((gate, owner))
    }
    fn profiles(&self) -> Result<Profiles, AppError> {
        Profiles::open(self.state.join("profiles.sqlite")).map_err(io)
    }

    /// Reads profiles. Errors never become an empty successful list.
    /// # Errors
    /// Invalid stored configuration or persistence failure.
    pub fn profiles_list(&self) -> Result<Vec<Profile>, AppError> {
        self.profiles()?.list().map_err(io)
    }

    /// Saves configuration, invalidating every pending preview.
    /// # Errors
    /// Busy, invalid profile, overlapping roots/state, or persistence failure.
    pub fn profile_save(&self, profile: Profile) -> Result<(), AppError> {
        let _gate = self.gate()?;
        profile.validate().map_err(|e| error("invalid_input", e))?;
        self.roots(&profile)?;
        self.profiles()?.save(&profile).map_err(io)?;
        *lock(&self.cache)? = None;
        Ok(())
    }

    /// Removes configuration only and invalidates preview. Content is never deleted.
    /// # Errors
    /// Busy or persistence failure.
    pub fn profile_remove(&self, id: ProfileId) -> Result<(), AppError> {
        let _gate = self.gate()?;
        self.profiles()?.remove(&id).map_err(io)?;
        *lock(&self.cache)? = None;
        Ok(())
    }

    /// Reads persisted presentation settings.
    /// # Errors
    /// Persistence/format error.
    pub fn preferences(&self) -> Result<Preferences, AppError> {
        self.profiles()?.preferences().map_err(io)
    }

    /// Saves presentation settings, without changing synchronization policies.
    /// # Errors
    /// Busy, persistence or format error.
    pub fn preferences_save(&self, preferences: Preferences) -> Result<(), AppError> {
        let _gate = self.gate()?;
        self.profiles()?.save_preferences(&preferences).map_err(io)
    }

    fn profile(&self, id: &ProfileId) -> Result<Profile, AppError> {
        self.profiles_list()?
            .into_iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| error("not_found", "profil absent"))
    }
    fn roots(&self, profile: &Profile) -> Result<(String, String), AppError> {
        profile.validate().map_err(|e| error("invalid_input", e))?;
        let mut locals = Vec::new();
        let mut locations = Vec::new();
        for (path, remote) in [
            (&profile.source, &profile.remote_source),
            (&profile.destination, &profile.remote_destination),
        ] {
            if let Some(remote) = remote {
                endpoints::supported(remote)?;
                if let sync_profiles::RemoteEndpoint::Smb { root } = remote {
                    locals.push(root.canonicalize().map_err(io)?);
                }
                locations.push(remote.location().map_err(io)?);
            } else {
                let root = path.canonicalize().map_err(io)?;
                if !root.is_dir() {
                    return Err(error("invalid_input", "racine dossier existant requise"));
                }
                locations.push(
                    root.to_str()
                        .ok_or_else(|| error("invalid_input", "racine non UTF-8"))?
                        .into(),
                );
                locals.push(root);
            }
        }
        for (index, root) in locals.iter().enumerate() {
            for other in locals
                .iter()
                .skip(index + 1)
                .chain([&self.state, &self.lock_root])
            {
                if root.starts_with(other) || other.starts_with(root) {
                    return Err(error(
                        "invalid_input",
                        "source, destination, ?tat et verrous doivent ?tre disjoints",
                    ));
                }
            }
        }
        if profile.remote_source.is_some() && profile.remote_destination.is_some() {
            return Err(error("not_supported", "une extr?mit? locale est requise"));
        }
        Ok((locations.remove(0), locations.remove(0)))
    }

    /// Providers compiled into this application; optional features never imply
    /// successful authentication or provider certification.
    #[must_use]
    pub fn providers(&self) -> Vec<&'static str> {
        endpoints::available()
    }

    fn begin(
        &self,
        id: &ProfileId,
        phase: JobPhase,
        token: Option<&CancellationToken>,
    ) -> Result<CancellationToken, AppError> {
        let cancel = token.cloned().unwrap_or_default();
        *lock(&self.cancel)? = cancel.clone();
        self.transferred.store(0, Relaxed);
        *lock(&self.snapshot)? = JobSnapshot {
            active: true,
            control_id: Some(unique("control")?),
            phase,
            profile: Some(id.clone()),
            ..JobSnapshot::default()
        };
        Ok(cancel)
    }
    fn end<T>(&self, result: Result<T, AppError>, id: &ProfileId) -> Result<T, AppError> {
        let mut snapshot = lock(&self.snapshot)?;
        snapshot.active = false;
        snapshot.phase = if result.is_ok() {
            JobPhase::Finished
        } else {
            JobPhase::Error
        };
        result.map_err(|mut e| {
            e.profile = Some(id.clone());
            e
        })
    }

    /// Computes and caches an immutable plan; synchronized files are never mutated.
    /// Starting any new preview invalidates the previous one even if scanning fails.
    /// # Errors
    /// Busy, unavailable root, scan/planning/persistence failure or cancellation.
    pub fn preview(&self, id: ProfileId) -> Result<serde_json::Value, AppError> {
        let _gate = self.gate()?;
        self.preview_locked(id, None, false, DeletionCeiling::Guarded)
    }
    fn preview_locked(
        &self,
        id: ProfileId,
        token: Option<&CancellationToken>,
        paged: bool,
        ceiling: DeletionCeiling,
    ) -> Result<serde_json::Value, AppError> {
        *lock(&self.cache)? = None;
        let cancel = self.begin(&id, JobPhase::Scanning, token)?;
        let result = (|| {
            let profile = self.profile(&id)?;
            let (source, destination) = self.roots(&profile)?;
            let left = endpoints::build_for_scan(
                Path::new(&source),
                profile.remote_source.as_ref(),
                &cancel,
            )?;
            let right = endpoints::build_for_scan(
                Path::new(&destination),
                profile.remote_destination.as_ref(),
                &cancel,
            )?;
            let now = Timestamp::now().map_err(io)?;
            let context = PlanContext {
                plan_id: PlanId::new(unique("plan")?).map_err(io)?,
                profile: Some(id.clone()),
                created_at: now,
                source_location: source,
                destination_location: destination,
            };
            let policy = PlanPolicy::new(
                OverwritePolicy::SourceWins,
                if profile.allow_delete {
                    DeletionPolicy::enabled(ceiling.guard())
                } else {
                    DeletionPolicy::Disabled
                },
            );
            let filter = Filter::new(profile.filter.clone()).map_err(io)?;
            // Presentation counters only, updated from both scan threads.
            let progress = Progress::default();
            let observe = |side: bool, stats: &ScanStats, issues: usize| {
                progress.record(side, stats, issues);
                let mut snapshot = self
                    .snapshot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                progress.publish(&mut snapshot);
            };
            let plan = if profile.mode == SyncMode::Bidirectional {
                sync_session::preview_observed(
                    &History::open(self.state.join("state.sqlite")).map_err(io)?,
                    left.as_ref(),
                    right.as_ref(),
                    &context,
                    &filter,
                    &policy,
                    &cancel,
                    &observe,
                )
                .map_err(io)?
            } else {
                // Listings only, both endpoints at once. The scanner bounds
                // directory workers by each connector's advertised concurrency.
                let listing = ScanOptions::new();
                let (a, b) = both(
                    || {
                        sync_scanner::scan_observed(
                            left.as_ref(),
                            &filter,
                            &listing,
                            &cancel,
                            &|stats, issues| observe(false, stats, issues),
                        )
                    },
                    || {
                        sync_scanner::scan_observed(
                            right.as_ref(),
                            &filter,
                            &listing,
                            &cancel,
                            &|stats, issues| observe(true, stats, issues),
                        )
                    },
                )
                .map_err(io)?;
                // Content: hash only files whose equal sizes leave the
                // question open; other verdicts cannot change.
                let (a, b) = match profile.compare {
                    CompareMethod::SizeAndDate => (a, b),
                    CompareMethod::Content => {
                        let candidates = sync_compare::content_candidates(&a, &b);
                        // Counters are cumulative: hashes already provided
                        // by the listings are part of the target.
                        progress.target(
                            (2 * candidates.len() as u64)
                                .saturating_add(a.stats().hashed)
                                .saturating_add(b.stats().hashed),
                        );
                        let hashing = ScanOptions::new()
                            .with_hash(HashAlgorithm::Blake3)
                            .with_hash_workers(RECOMMENDED_HASH_WORKERS);
                        both(
                            || {
                                sync_scanner::hash_selected(
                                    left.as_ref(),
                                    a,
                                    &candidates,
                                    &hashing,
                                    &cancel,
                                    &|stats, issues| observe(false, stats, issues),
                                )
                            },
                            || {
                                sync_scanner::hash_selected(
                                    right.as_ref(),
                                    b,
                                    &candidates,
                                    &hashing,
                                    &cancel,
                                    &|stats, issues| observe(true, stats, issues),
                                )
                            },
                        )
                        .map_err(io)?
                    }
                };
                if cancel.checkpoint() {
                    return Err(error("cancelled", "analyse annulée"));
                }
                let comparison =
                    sync_compare::compare(&a, &b, &sync_compare::CompareOptions::new())
                        .map_err(io)?;
                let input = PlanningInput::new(&comparison, &context);
                match profile.mode {
                    SyncMode::Backup => {
                        sync_backup::BackupStrategy::new().build_plan(&input, &policy)
                    }
                    SyncMode::Mirror => {
                        sync_mirror::MirrorStrategy::new().build_plan(&input, &policy)
                    }
                    _ => return Err(error("not_supported", "mode inconnu")),
                }
                .map_err(io)?
            };
            if cancel.checkpoint() {
                return Err(error("cancelled", "analyse annulée"));
            }
            let summary = plan.summary();
            let value: serde_json::Value = if paged {
                serde_json::json!({"id":plan.id(), "profile":plan.profile(), "mode":plan.mode(), "source":plan.source(), "destination":plan.destination(), "operations":plan.operations().iter().take(100).collect::<Vec<_>>()})
            } else {
                serde_json::to_value(&plan).map_err(io)?
            };
            // Guards are all-or-nothing: every refused deletion carries the same cause.
            let refusal = plan.operations().iter().find_map(|op| match op.reason() {
                Reason::DeletionRefused { refusal } => Some(*refusal),
                _ => None,
            });
            let lifted = profile.allow_delete && ceiling == DeletionCeiling::Lifted;
            let view = serde_json::json!({"plan":value,"summary":{"copy":summary.copy,"replace":summary.replace,"delete":summary.delete,"keep":summary.keep,"conflict":summary.conflict,"skip":summary.skip,"bytes":summary.bytes_to_transfer,"deletions_refused":summary.deletions_refused,"deletion_refusal":refusal,"ceiling_lifted":lifted}});
            *lock(&self.cache)? = Some(Cached { profile, plan });
            Ok(view)
        })();
        self.end(result, &id)
    }

    /// Consumes exactly the cached preview, once. A modified profile, mismatching
    /// roots or plan ID is refused. Versions and durable intent precede mutations.
    /// Deletions require separate confirmation even when enabled in the profile.
    /// # Errors
    /// Busy/stale/confirmation, I/O, executor or durable completion failure.
    /// Errors after execution began can leave partial work; inspect history.
    pub fn execute(
        &self,
        id: ProfileId,
        plan_id: String,
        confirm: bool,
        confirm_delete: bool,
    ) -> Result<serde_json::Value, AppError> {
        let _gate = self.gate()?;
        self.execute_locked(id, plan_id, confirm, confirm_delete, None)
    }
    fn execute_locked(
        &self,
        id: ProfileId,
        plan_id: String,
        confirm: bool,
        confirm_delete: bool,
        token: Option<&CancellationToken>,
    ) -> Result<serde_json::Value, AppError> {
        if !confirm {
            return Err(error(
                "confirmation_required",
                "confirmer les écritures réelles",
            ));
        }
        let cached = lock(&self.cache)?
            .take()
            .ok_or_else(|| error("stale", "nouvelle simulation requise"))?;
        if cached.profile.id != id
            || cached.plan.id().as_str() != plan_id
            || self.profile(&id)? != cached.profile
        {
            return Err(error("stale", "profil ou plan modifié ; simuler à nouveau"));
        }
        if cached.plan.summary().delete > 0 && !confirm_delete {
            return Err(error(
                "confirmation_required",
                "confirmer les suppressions affichées",
            ));
        }
        let cancel = self.begin(&id, JobPhase::Executing, token)?;
        let result = self.execute_cached(cached, &cancel, confirm_delete);
        self.end(result, &id)
    }
    fn execute_cached(
        &self,
        cached: Cached,
        cancel: &CancellationToken,
        confirm_delete: bool,
    ) -> Result<serde_json::Value, AppError> {
        let _job_lock = scheduled::job_lock(&self.lock_root)?;
        if runs::unfinished(&self.state, &cached.profile.id)? {
            return Err(error(
                "interrupted",
                "unfinished execution: explicit diagnosis required",
            ));
        }
        let (source, destination) = self.roots(&cached.profile)?;
        if source != cached.plan.source().location()
            || destination != cached.plan.destination().location()
        {
            return Err(error("stale", "racines modifiées"));
        }
        let left = endpoints::build(
            Path::new(&source),
            cached.profile.remote_source.as_ref(),
            cached.profile.mode == SyncMode::Bidirectional,
            cancel,
        )?;
        let right = endpoints::build(
            Path::new(&destination),
            cached.profile.remote_destination.as_ref(),
            true,
            cancel,
        )?;
        let execution = ExecutionId::new(unique("run")?).map_err(io)?;
        {
            let summary = cached.plan.summary();
            let mut snapshot = lock(&self.snapshot)?;
            snapshot.execution = Some(execution.clone());
            snapshot.planned_operations = summary.copy + summary.replace + summary.delete;
            snapshot.planned_bytes = summary.bytes_to_transfer;
        }
        let file =
            FileJournal::create(self.state.join(format!("{execution}.jsonl"))).map_err(io)?;
        let mut journal = ObservedJournal {
            file,
            snapshot: &self.snapshot,
        };
        let versions =
            LocalConnector::new(&self.state, LocalOptions::new().with_mutations()).map_err(io)?;
        let store = VersionStore::new(
            &versions,
            ResourcePath::parse(&format!("versions/{execution}")).map_err(io)?,
        )
        .map_err(io)?;
        runs::begin(&self.state, &execution, &cached.profile.id, &cached.plan)?;
        let options = ExecutionOptions::new()
            .with_deletions_confirmed(confirm_delete)
            .with_progress_interval(LIVE_PROGRESS_BYTES);
        let events = EventEmitter::new(
            execution.clone(),
            Some(cached.profile.id.clone()),
            Arc::new(LiveTransfer(Arc::clone(&self.transferred))),
        );
        let result = if cached.profile.mode == SyncMode::Bidirectional {
            let mut history = History::open(self.state.join("state.sqlite")).map_err(io)?;
            let request = sync_session::SessionRequest {
                plan: &cached.plan,
                source: left.as_ref(),
                destination: right.as_ref(),
                execution: execution.clone(),
                journal: &mut journal,
                cancel,
                options,
                versions: Some(&store),
                events: Some(&events),
            };
            sync_session::execute(&mut history, request, RetentionPolicy::default())
                .map_err(io)
                .map(|report| {
                    serde_json::json!({
                        "execution": report.execution,
                        "baseline_updated": report.baseline_updated,
                        "verification_error": report.verification_error,
                    })
                })
        } else {
            Executor
                .execute(
                    ExecutionRequest::new(
                        &cached.plan,
                        left.as_ref(),
                        right.as_ref(),
                        execution.clone(),
                        &mut journal,
                        cancel,
                    )
                    .with_options(options)
                    .with_versions(&store)
                    .with_events(&events),
                )
                .map_err(io)
                .and_then(|r| serde_json::to_value(r).map_err(io))
        };
        runs::finish(&self.state, &execution, &result)?;
        result
    }

    /// Cancels the current scan/transfer cooperatively. Does not release ownership.
    /// # Errors
    /// Internal state poisoned.
    pub fn cancel(&self) -> Result<(), AppError> {
        lock(&self.cancel)?.cancel();
        Ok(())
    }

    /// Pauses or resumes the current scan/transfer without releasing ownership.
    /// # Errors
    /// Internal state poisoned.
    pub fn set_paused(&self, paused: bool) -> Result<(), AppError> {
        lock(&self.cancel)?.set_paused(paused);
        Ok(())
    }

    /// Latest bounded observation, independent of the worker gate.
    /// # Errors
    /// Internal state poisoned.
    pub fn snapshot(&self) -> Result<JobSnapshot, AppError> {
        let paused = lock(&self.cancel)?.is_paused();
        let mut snapshot = lock(&self.snapshot)?.clone();
        snapshot.pause_requested = snapshot.active && paused;
        // Validated bytes were transferred too, even if an event was coalesced.
        snapshot.bytes_transferred = self.transferred.load(Relaxed).max(snapshot.bytes_done);
        Ok(snapshot)
    }

    /// Last 100 attempts for a profile; unfinished work remains visibly running.
    /// # Errors
    /// Persistence/format failure.
    pub fn history(&self, id: ProfileId) -> Result<Vec<Run>, AppError> {
        runs::list(&self.state, &id)
    }
}

/// Runs the source and destination halves of a preview concurrently (one
/// thread each, like FreeFileSync's per-device traversal). Both always run to
/// completion; the source error is reported first. A panic is re-raised.
fn both(
    source: impl FnOnce() -> Result<Inventory, ScanError> + Send,
    destination: impl FnOnce() -> Result<Inventory, ScanError>,
) -> Result<(Inventory, Inventory), ScanError> {
    let (source, destination) = std::thread::scope(|threads| {
        let source = threads.spawn(source);
        let destination = destination();
        (source.join(), destination)
    });
    let source = source.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    Ok((source?, destination?))
}

/// Latest per-endpoint scan counters, merged into the job snapshot.
#[derive(Default)]
struct Progress {
    entries: [AtomicU64; 2],
    issues: [AtomicU64; 2],
    consistency: [AtomicU64; 2],
    hashed: [AtomicU64; 2],
    target: AtomicU64,
}

impl Progress {
    fn record(&self, side: bool, stats: &ScanStats, issues: usize) {
        let side = usize::from(side);
        let entries = stats
            .files
            .saturating_add(stats.directories)
            .saturating_add(stats.others);
        self.entries[side].store(entries, Relaxed);
        self.issues[side].store(
            (issues as u64).saturating_sub(stats.consistency_warnings),
            Relaxed,
        );
        self.consistency[side].store(stats.consistency_warnings, Relaxed);
        self.hashed[side].store(stats.hashed, Relaxed);
    }
    fn target(&self, hashes: u64) {
        self.target.store(hashes, Relaxed);
    }
    fn publish(&self, snapshot: &mut JobSnapshot) {
        let sum =
            |pair: &[AtomicU64; 2]| pair[0].load(Relaxed).saturating_add(pair[1].load(Relaxed));
        snapshot.entries_scanned = sum(&self.entries);
        snapshot.scan_issues = sum(&self.issues);
        snapshot.scan_consistency_warnings = sum(&self.consistency);
        snapshot.entries_hashed = sum(&self.hashed);
        snapshot.hash_target = self.target.load(Relaxed);
    }
}

fn unique(prefix: &str) -> Result<String, AppError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let now = Timestamp::now().map_err(io)?;
    Ok(format!(
        "{prefix}-{}-{}-{}-{}",
        std::process::id(),
        now.secs(),
        now.subsec_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

// Observations are derived only after durable journal writes. They never
// authorize mutations or state publication. One latest aggregate is retained.
struct ObservedJournal<'a> {
    file: FileJournal,
    snapshot: &'a Mutex<JobSnapshot>,
}
impl JournalSink for ObservedJournal<'_> {
    fn append(&mut self, record: JournalRecord) -> Result<(), JournalError> {
        self.file.append(record.clone())?;
        if let JournalRecord::OperationFinished { bytes, outcome, .. } = record {
            // Presentation-only poison recovery cannot change execution decisions.
            let mut value = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            value.entries_done = value.entries_done.saturating_add(1);
            value.bytes_done = value.bytes_done.saturating_add(bytes);
            if matches!(
                outcome,
                sync_journal::OutcomeCode::Failed | sync_journal::OutcomeCode::Refused
            ) {
                value.operation_errors = value.operation_errors.saturating_add(1);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod scan_progress_tests {
    use super::*;

    #[test]
    fn provider_uncertainty_does_not_hide_real_read_errors() {
        let progress = Progress::default();
        let stats = ScanStats {
            consistency_warnings: 10,
            files: 25,
            ..ScanStats::default()
        };
        progress.record(false, &stats, 12);
        progress.record(true, &ScanStats::default(), 3);
        let mut snapshot = JobSnapshot::default();
        progress.publish(&mut snapshot);
        assert_eq!(snapshot.scan_issues, 5);
        assert_eq!(snapshot.scan_consistency_warnings, 10);
        assert_eq!(snapshot.entries_scanned, 25);
    }

    #[test]
    fn older_live_snapshots_default_to_zero_consistency_warnings()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(JobSnapshot::default())?;
        value
            .as_object_mut()
            .ok_or("not an object")?
            .remove("scan_consistency_warnings");
        let snapshot: JobSnapshot = serde_json::from_value(value)?;
        assert_eq!(snapshot.scan_consistency_warnings, 0);
        Ok(())
    }
}
