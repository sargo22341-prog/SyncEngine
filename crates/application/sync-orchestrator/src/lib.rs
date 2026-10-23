//! Blocking local application facade. Call on a dedicated worker, never the UI
//! thread. One job per instance, an exclusive process lock on its state directory,
//! and a shared per-user execution lock across facade state directories.
//! Engines own every comparison, policy decision and file mutation.

mod runs;
mod scheduled;

use connector_local::{LocalConnector, LocalOptions};
pub use runs::Run;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use sync_core::{
    CancellationToken, ExecutionId, HashAlgorithm, PlanId, ProfileId, ResourcePath, Timestamp,
};
use sync_decisions::{PlanContext, PlanStrategy, PlanningInput};
use sync_executor::{ExecutionOptions, ExecutionRequest, Executor, PlanExecutor};
use sync_filter::Filter;
use sync_history::{History, RetentionPolicy};
use sync_journal::{FileJournal, JournalError, JournalRecord, JournalSink};
use sync_plan::{
    DeletionGuard, DeletionPolicy, ExecutionPlan, OverwritePolicy, PlanPolicy, SyncMode,
};
use sync_profiles::{Preferences, Profile, Profiles};
use sync_scanner::{ScanOptions, scan};
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
fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>, AppError> {
    value
        .lock()
        .map_err(|_| error("internal", "état verrouillé après une panne ; redémarrer"))
}

/// Bounded latest observation. No event queue grows with transfer size.
#[derive(Debug, Clone, Copy, Default, Serialize)]
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
#[derive(Debug, Clone, Default, Serialize)]
pub struct JobSnapshot {
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
}

struct Cached {
    profile: Profile,
    plan: ExecutionPlan,
}

/// Facade owner, safe to share with command workers. Never accepts a plan or
/// connector implementation from TypeScript. Store paths are application owned.
pub struct Application {
    state: PathBuf,
    lock_root: PathBuf,
    _owner: File,
    gate: Mutex<()>,
    cache: Mutex<Option<Cached>>,
    cancel: Mutex<CancellationToken>,
    snapshot: Mutex<JobSnapshot>,
}
impl Application {
    /// Opens an existing state directory, locks it and initializes versioned stores.
    /// # Errors
    /// State path/lock/persistence error; second desktop process is refused.
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
        owner
            .try_lock()
            .map_err(|_| error("busy", "un autre desktop utilise ce répertoire d’état"))?;
        Profiles::open(state.join("profiles.sqlite")).map_err(io)?;
        runs::open(&state)?;
        History::open(state.join("state.sqlite")).map_err(io)?;
        Ok(Self {
            state,
            lock_root,
            _owner: owner,
            gate: Mutex::new(()),
            cache: Mutex::new(None),
            cancel: Mutex::new(CancellationToken::new()),
            snapshot: Mutex::new(JobSnapshot::default()),
        })
    }

    fn gate(&self) -> Result<MutexGuard<'_, ()>, AppError> {
        self.gate
            .try_lock()
            .map_err(|_| error("busy", "une commande est déjà en cours"))
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
    /// Persistence/format error.
    pub fn preferences_save(&self, preferences: Preferences) -> Result<(), AppError> {
        self.profiles()?.save_preferences(&preferences).map_err(io)
    }

    fn profile(&self, id: &ProfileId) -> Result<Profile, AppError> {
        self.profiles_list()?
            .into_iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| error("not_found", "profil absent"))
    }
    fn roots(&self, profile: &Profile) -> Result<(PathBuf, PathBuf), AppError> {
        profile.validate().map_err(|e| error("invalid_input", e))?;
        let source = profile.source.canonicalize().map_err(io)?;
        let destination = profile.destination.canonicalize().map_err(io)?;
        for (a, b) in [
            (&source, &destination),
            (&source, &self.state),
            (&destination, &self.state),
            (&source, &self.lock_root),
            (&destination, &self.lock_root),
        ] {
            if a.starts_with(b) || b.starts_with(a) {
                return Err(error(
                    "invalid_input",
                    "source, destination et état doivent être disjoints",
                ));
            }
        }
        if !source.is_dir() || !destination.is_dir() {
            return Err(error(
                "invalid_input",
                "les racines doivent être des dossiers existants",
            ));
        }
        Ok((source, destination))
    }
    fn begin(
        &self,
        id: &ProfileId,
        phase: JobPhase,
        token: Option<&CancellationToken>,
    ) -> Result<CancellationToken, AppError> {
        let cancel = token.cloned().unwrap_or_default();
        *lock(&self.cancel)? = cancel.clone();
        *lock(&self.snapshot)? = JobSnapshot {
            active: true,
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
        self.preview_locked(id, None)
    }
    fn preview_locked(
        &self,
        id: ProfileId,
        token: Option<&CancellationToken>,
    ) -> Result<serde_json::Value, AppError> {
        *lock(&self.cache)? = None;
        let cancel = self.begin(&id, JobPhase::Scanning, token)?;
        let result = (|| {
            let profile = self.profile(&id)?;
            let (source, destination) = self.roots(&profile)?;
            let left = LocalConnector::new(&source, LocalOptions::new()).map_err(io)?;
            let right = LocalConnector::new(&destination, LocalOptions::new()).map_err(io)?;
            let now = Timestamp::now().map_err(io)?;
            let context = PlanContext {
                plan_id: PlanId::new(unique("plan")?).map_err(io)?,
                profile: Some(id.clone()),
                created_at: now,
                source_location: source
                    .to_str()
                    .ok_or_else(|| error("invalid_input", "racine non UTF-8"))?
                    .into(),
                destination_location: destination
                    .to_str()
                    .ok_or_else(|| error("invalid_input", "racine non UTF-8"))?
                    .into(),
            };
            let policy = PlanPolicy::new(
                OverwritePolicy::SourceWins,
                if profile.allow_delete {
                    DeletionPolicy::enabled(DeletionGuard::new())
                } else {
                    DeletionPolicy::Disabled
                },
            );
            let filter = Filter::new(profile.filter.clone()).map_err(io)?;
            let plan = if profile.mode == SyncMode::Bidirectional {
                sync_session::preview(
                    &History::open(self.state.join("state.sqlite")).map_err(io)?,
                    &left,
                    &right,
                    &context,
                    &filter,
                    &policy,
                    &cancel,
                )
                .map_err(io)?
            } else {
                let options = ScanOptions::new().with_hash(HashAlgorithm::Blake3);
                let a = scan(&left, &filter, &options, &cancel).map_err(io)?;
                let b = scan(&right, &filter, &options, &cancel).map_err(io)?;
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
            let summary = plan.summary();
            let value: serde_json::Value =
                serde_json::from_str(&plan.to_json().map_err(io)?).map_err(io)?;
            let view = serde_json::json!({"plan":value,"summary":{"copy":summary.copy,"replace":summary.replace,"delete":summary.delete,"keep":summary.keep,"conflict":summary.conflict,"skip":summary.skip,"bytes":summary.bytes_to_transfer,"deletions_refused":summary.deletions_refused}});
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
        let (source, destination) = self.roots(&cached.profile)?;
        if source.to_str() != Some(cached.plan.source().location())
            || destination.to_str() != Some(cached.plan.destination().location())
        {
            return Err(error("stale", "racines modifiées"));
        }
        let left = LocalConnector::new(
            &source,
            if cached.profile.mode == SyncMode::Bidirectional {
                LocalOptions::new().with_mutations()
            } else {
                LocalOptions::new()
            },
        )
        .map_err(io)?;
        let right =
            LocalConnector::new(&destination, LocalOptions::new().with_mutations()).map_err(io)?;
        let execution = ExecutionId::new(unique("run")?).map_err(io)?;
        lock(&self.snapshot)?.execution = Some(execution.clone());
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
        let options = ExecutionOptions::new().with_deletions_confirmed(confirm_delete);
        let result = if cached.profile.mode == SyncMode::Bidirectional {
            let mut history = History::open(self.state.join("state.sqlite")).map_err(io)?;
            let request = sync_session::SessionRequest {
                plan: &cached.plan,
                source: &left,
                destination: &right,
                execution: execution.clone(),
                journal: &mut journal,
                cancel,
                options,
                versions: Some(&store),
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
                        &left,
                        &right,
                        execution.clone(),
                        &mut journal,
                        cancel,
                    )
                    .with_options(options)
                    .with_versions(&store),
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

    /// Latest bounded observation, independent of the worker gate.
    /// # Errors
    /// Internal state poisoned.
    pub fn snapshot(&self) -> Result<JobSnapshot, AppError> {
        Ok(lock(&self.snapshot)?.clone())
    }

    /// Last 100 attempts for a profile; unfinished work remains visibly running.
    /// # Errors
    /// Persistence/format failure.
    pub fn history(&self, id: ProfileId) -> Result<Vec<Run>, AppError> {
        runs::list(&self.state, &id)
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
        if let JournalRecord::OperationFinished { bytes, .. } = record {
            // Presentation-only poison recovery cannot change execution decisions.
            let mut value = self
                .snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            value.entries_done = value.entries_done.saturating_add(1);
            value.bytes_done = value.bytes_done.saturating_add(bytes);
        }
        Ok(())
    }
}
