//! Bridge between the iced runtime and the blocking application facade.
//!
//! Every facade call runs on a dedicated OS thread and reports through a
//! one-shot channel, so neither the UI thread nor the async executor is ever
//! blocked by SQLite, scanning or transfers. Cancellation stays cooperative
//! (`Application::cancel`), and a dropped UI never aborts durable writes.

use crate::model::{self, PreviewView, RunView};
use iced::futures::channel::oneshot;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use sync_core::ProfileId;
use sync_orchestrator::{AppError, Application, DeletionCeiling};
use sync_profiles::{Preferences, Profile};
use sync_scheduler::{Completion, MissedJobs, Schedule, ScheduleState, Trigger};

pub(crate) type Shared = Arc<Application>;

/// Error surfaced to the user (already redacted by the facade).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
    /// Plain-language headline for an error code.
    pub(crate) fn title(&self) -> &'static str {
        match self.code.as_str() {
            "busy" => "Une autre opération est en cours",
            "stale" => "La simulation n’est plus à jour",
            "confirmation_required" => "Confirmation requise",
            "invalid_input" => "Configuration refusée",
            "not_supported" => "Non pris en charge",
            "not_found" => "Introuvable",
            "interrupted" => "Exécution interrompue à diagnostiquer",
            "integrity" => "Données d’état illisibles",
            "unavailable" => "Emplacement ou stockage indisponible",
            _ => "Opération impossible",
        }
    }
}

impl From<AppError> for Failure {
    fn from(e: AppError) -> Self {
        Self {
            code: e.code,
            message: e.message,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} : {}", self.title(), self.message)
    }
}

/// Runs a blocking closure on its own thread.
pub(crate) fn blocking<T, F>(job: F) -> impl Future<Output = Result<T, Failure>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Failure> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("syncengine-worker".into())
        .spawn(move || {
            // A closed receiver means the window is gone: the outcome is
            // already durable in the facade's history, nothing else to do.
            if tx.send(job()).is_err() {}
        });
    async move {
        spawned.map_err(|e| Failure::new("transport", format!("thread indisponible : {e}")))?;
        rx.await
            .map_err(|_| Failure::new("transport", "tâche interrompue avant sa réponse"))?
    }
}

/// Runs a facade call on its own thread.
pub(crate) fn call<T, F>(
    app: &Shared,
    job: F,
) -> impl Future<Output = Result<T, Failure>> + use<T, F>
where
    T: Send + 'static,
    F: FnOnce(&Application) -> Result<T, AppError> + Send + 'static,
{
    let app = Arc::clone(app);
    blocking(move || job(&app).map_err(Failure::from))
}

/// Default state directory, identical to the previous desktop shell so that
/// existing profiles, history and agent authorizations are reused.
///
/// `SYNCENGINE_STATE_DIR` (absolute path) selects another, isolated state
/// directory: demonstrations and manual tests never touch the user's data.
pub(crate) fn state_directory() -> Result<PathBuf, Failure> {
    const ID: &str = "org.syncengine.desktop";
    if let Some(dir) = std::env::var_os("SYNCENGINE_STATE_DIR").map(PathBuf::from) {
        return if dir.is_absolute() {
            Ok(dir)
        } else {
            Err(Failure::new(
                "invalid_input",
                "SYNCENGINE_STATE_DIR doit être un chemin absolu",
            ))
        };
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    let base = base
        .ok_or_else(|| Failure::new("unavailable", "dossier de données utilisateur introuvable"))?;
    Ok(base.join(ID))
}

pub(crate) async fn open() -> Result<Opened, Failure> {
    blocking(|| {
        let path = state_directory()?;
        std::fs::create_dir_all(&path)
            .map_err(|e| Failure::new("unavailable", format!("{} : {e}", path.display())))?;
        let app = Arc::new(Application::open(&path).map_err(Failure::from)?);
        let workspace = load(&app, Vec::new())?;
        Ok(Opened(app, workspace))
    })
    .await
}

/// Cloneable copy of a persisted authorization and its latest receipt.
#[derive(Debug, Clone)]
pub(crate) struct ScheduleView {
    pub(crate) schedule: Schedule,
    pub(crate) running_slot: Option<i64>,
    pub(crate) result: Option<Completion>,
}

impl From<ScheduleState> for ScheduleView {
    fn from(s: ScheduleState) -> Self {
        Self {
            schedule: s.schedule,
            running_slot: s.running_slot,
            result: s.result,
        }
    }
}

/// Facade handle and first workspace, delivered once at start-up.
#[derive(Clone)]
pub(crate) struct Opened(pub(crate) Shared, pub(crate) Workspace);

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Opened").field(&self.1).finish()
    }
}

/// Everything the main screens display, read in one worker pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct Workspace {
    pub(crate) profiles: Vec<Profile>,
    pub(crate) schedules: Vec<ScheduleView>,
    /// False when schedules could not be read because a job holds the gate.
    pub(crate) schedules_fresh: bool,
    pub(crate) runs: HashMap<ProfileId, Vec<RunView>>,
    pub(crate) preferences: Preferences,
    pub(crate) providers: Vec<&'static str>,
    /// A stop request blocks every scheduled run until lifted.
    pub(crate) agent_stopped: bool,
}

fn load(app: &Application, previous: Vec<ScheduleView>) -> Result<Workspace, Failure> {
    let profiles = app.profiles_list().map_err(Failure::from)?;
    let (schedules, schedules_fresh) = match app.schedules() {
        Ok(list) => (list.into_iter().map(ScheduleView::from).collect(), true),
        Err(e) if e.code == "busy" => (previous, false),
        Err(e) => return Err(e.into()),
    };
    let mut runs = HashMap::new();
    for profile in &profiles {
        let list = app
            .history_overview(profile.id.clone())
            .map_err(Failure::from)?;
        runs.insert(
            profile.id.clone(),
            list.iter().map(RunView::from_run).collect(),
        );
    }
    Ok(Workspace {
        profiles,
        schedules,
        schedules_fresh,
        runs,
        preferences: app.preferences().map_err(Failure::from)?,
        providers: app.providers(),
        agent_stopped: app.agent_cancel_pending(),
    })
}

pub(crate) fn refresh(
    app: &Shared,
    previous: Vec<ScheduleView>,
) -> impl Future<Output = Result<Workspace, Failure>> + use<> {
    let app = Arc::clone(app);
    blocking(move || load(&app, previous))
}

pub(crate) fn preview(
    app: &Shared,
    id: ProfileId,
    ceiling: DeletionCeiling,
) -> impl Future<Output = Result<PreviewView, Failure>> + use<> {
    let app = Arc::clone(app);
    blocking(move || {
        let value = app.preview_paged_with(id, ceiling).map_err(Failure::from)?;
        PreviewView::parse(&value).map_err(|m| Failure::new("format", m))
    })
}

/// One page (≤ 100) of the exact cached plan.
#[derive(Debug, Clone)]
pub(crate) struct PlanPage {
    pub(crate) offset: usize,
    pub(crate) total: usize,
    pub(crate) ops: Vec<model::OpView>,
}

pub(crate) fn plan_page(
    app: &Shared,
    plan: String,
    offset: usize,
    query: String,
    action: &'static str,
) -> impl Future<Output = Result<PlanPage, Failure>> + use<> {
    call(app, move |a| {
        let value = a.plan_page(&plan, offset, &query, action)?;
        Ok(PlanPage {
            offset,
            total: value["total"].as_u64().unwrap_or(0) as usize,
            ops: model::ops(&value["operations"]),
        })
    })
}

/// Final classification of an execution started from this window.
#[derive(Debug, Clone)]
pub(crate) struct Executed {
    pub(crate) outcome: model::Outcome,
    pub(crate) label: String,
    pub(crate) totals: Vec<(String, u64)>,
}

pub(crate) fn execute(
    app: &Shared,
    id: ProfileId,
    plan: String,
    confirm_delete: bool,
) -> impl Future<Output = Result<Executed, Failure>> + use<> {
    call(app, move |a| {
        let value = a.execute(id, plan, true, confirm_delete)?;
        let report = value
            .get("execution")
            .filter(|v| v.is_object())
            .unwrap_or(&value);
        let mut merged = report.clone();
        if let Some(v) = value.get("verification_error") {
            merged["verification_error"] = v.clone();
        }
        let (outcome, label) = model::classify("finished", &merged);
        let totals = report["totals"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n)))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Executed {
            outcome,
            label,
            totals,
        })
    })
}

/// Outcome of a sequence of per-profile writes: names done, or the names
/// done before the first failure.
pub(crate) type Batch = Result<Vec<String>, (Vec<String>, Failure)>;

/// Result of saving a profile and, optionally, its schedule.
#[derive(Debug, Clone)]
pub(crate) struct Saved {
    pub(crate) id: ProfileId,
    pub(crate) schedule_error: Option<Failure>,
}

/// Schedule change requested together with a profile save.
#[derive(Debug, Clone)]
pub(crate) enum ScheduleChange {
    Keep,
    Remove,
    Set {
        trigger: Trigger,
        missed: MissedJobs,
        approve_writes: bool,
        approve_deletions: bool,
        show_window: bool,
    },
}

pub(crate) fn save(
    app: &Shared,
    profile: Profile,
    schedule: ScheduleChange,
) -> impl Future<Output = Result<Saved, Failure>> + use<> {
    call(app, move |a| {
        let id = profile.id.clone();
        a.profile_save(profile)?;
        let schedule_error = match schedule {
            ScheduleChange::Keep => Ok(()),
            ScheduleChange::Remove => a.schedule_remove(id.clone()),
            ScheduleChange::Set {
                trigger,
                missed,
                approve_writes,
                approve_deletions,
                show_window,
            } => a.schedule_save(
                id.clone(),
                trigger,
                missed,
                approve_writes,
                approve_deletions,
                show_window,
            ),
        }
        .err()
        .map(Failure::from);
        Ok(Saved { id, schedule_error })
    })
}

/// Moves profiles to another group, renewing existing authorizations with
/// their previous settings (the configuration change invalidates them).
/// Stops at the first failure and reports what was done.
pub(crate) fn regroup(
    app: &Shared,
    moves: Vec<(Profile, Option<ScheduleView>)>,
) -> impl Future<Output = Batch> + use<> {
    let app = Arc::clone(app);
    let (tx, rx) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("syncengine-worker".into())
        .spawn(move || {
            let mut done = Vec::new();
            let mut result = Ok(());
            for (profile, schedule) in moves {
                if schedule.as_ref().is_some_and(|s| s.running_slot.is_some()) {
                    result = Err(Failure::new(
                        "interrupted",
                        format!("« {} » a une tâche en cours ou interrompue", profile.name),
                    ));
                    break;
                }
                let name = profile.name.clone();
                let id = profile.id.clone();
                if let Err(e) = app.profile_save(profile) {
                    result = Err(e.into());
                    break;
                }
                if let Some(s) = schedule
                    && let Err(e) = app.schedule_save(
                        id,
                        s.schedule.trigger,
                        s.schedule.missed,
                        true,
                        s.schedule.allow_deletions,
                        s.schedule.show_window,
                    )
                {
                    result = Err(Failure::new(
                        &e.code,
                        format!(
                            "« {name} » déplacé, mais planification non renouvelée : {}",
                            e.message
                        ),
                    ));
                    done.push(name);
                    break;
                }
                done.push(name);
            }
            let outcome = match result {
                Ok(()) => Ok(done),
                Err(e) => Err((done, e)),
            };
            if tx.send(outcome).is_err() {}
        });
    async move {
        if let Err(e) = spawned {
            return Err((Vec::new(), Failure::new("transport", e.to_string())));
        }
        rx.await
            .map_err(|_| (Vec::new(), Failure::new("transport", "tâche interrompue")))?
    }
}

/// One page of a historical run.
#[derive(Debug, Clone)]
pub(crate) struct HistoryPage {
    pub(crate) execution: String,
    pub(crate) offset: usize,
    pub(crate) total: usize,
    pub(crate) rows: Vec<model::HistoryRow>,
}

pub(crate) fn history_page(
    app: &Shared,
    id: ProfileId,
    execution: String,
    offset: usize,
) -> impl Future<Output = Result<HistoryPage, Failure>> + use<> {
    call(app, move |a| {
        let value = a.history_page(id, &execution, offset)?;
        Ok(HistoryPage {
            offset,
            total: value["total"].as_u64().unwrap_or(0) as usize,
            rows: model::history_rows(&value),
            execution,
        })
    })
}
