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
            "scheduler_install" => "Enregistrement de la planification Windows impossible",
            "trash_insufficient" => "Corbeille insuffisante",
            "trash_retention" => "Corbeille non nettoyée",
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

/// A busy command is a normal startup wait, not a failed application.
#[derive(Debug, Clone)]
pub(crate) enum Opening {
    Ready(Opened),
    Waiting(StartupObservation),
}

#[derive(Debug, Clone)]
pub(crate) struct StartupObservation {
    pub(crate) resident: sync_orchestrator::ResidentStatus,
    pub(crate) live: Option<sync_orchestrator::LiveJob>,
    pub(crate) queue: Option<sync_orchestrator::LiveQueue>,
}

fn opening_failure(path: &std::path::Path, error: AppError) -> Result<Opening, Failure> {
    if error.code != "busy" {
        return Err(error.into());
    }
    Ok(Opening::Waiting(StartupObservation {
        resident: sync_orchestrator::resident_status(path).map_err(Failure::from)?,
        live: sync_orchestrator::LiveJob::read(path).map_err(Failure::from)?,
        queue: sync_orchestrator::LiveQueue::read(path).map_err(Failure::from)?,
    }))
}

pub(crate) async fn open() -> Result<Opening, Failure> {
    blocking(|| {
        let path = state_directory()?;
        std::fs::create_dir_all(&path)
            .map_err(|e| Failure::new("unavailable", format!("{} : {e}", path.display())))?;
        let app = match Application::open(&path) {
            Ok(app) => Arc::new(app),
            Err(error) => return opening_failure(&path, error),
        };
        let mut workspace = load(&app, Vec::new())?;
        if cfg!(windows) && workspace.schedules.iter().any(|s| s.schedule.enabled) {
            workspace.supervision_error = crate::native::prepare_agent(app.state_directory()).err();
        }
        Ok(Opening::Ready(Opened(app, workspace)))
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
    pub(crate) resident: sync_orchestrator::ResidentStatus,
    pub(crate) supervision_error: Option<Failure>,
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
        agent_stopped: app.agent_cancel_pending()
            || sync_orchestrator::agent_stop_pending(app.state_directory()),
        resident: sync_orchestrator::resident_status(app.state_directory())
            .map_err(Failure::from)?,
        supervision_error: None,
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
    pub(crate) execution: String,
    pub(crate) problems_total: usize,
    pub(crate) problems: Vec<model::HistoryRow>,
    pub(crate) diagnostics_error: Option<String>,
    pub(crate) outcome: model::Outcome,
    pub(crate) label: String,
    pub(crate) totals: Vec<(String, u64)>,
    /// Journal or trash clean-up left for later (the report is complete).
    pub(crate) maintenance: Vec<String>,
}

pub(crate) fn execute(
    app: &Shared,
    id: ProfileId,
    plan: String,
    confirm_delete: bool,
    confirm_permanent: bool,
) -> impl Future<Output = Result<Executed, Failure>> + use<> {
    call(app, move |a| {
        let consent = sync_orchestrator::Consent {
            writes: true,
            deletions: confirm_delete,
            permanent_loss: confirm_permanent,
        };
        let value = a.execute_with(id.clone(), plan, consent)?;
        let maintenance = value["maintenance_warnings"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|w| w.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
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
        let execution = report["execution"].as_str().unwrap_or("").to_owned();
        let (problems_total, problems, diagnostics_error) = match a.history_page_filtered(
            id,
            &execution,
            0,
            sync_orchestrator::HistoryFilter::Problems,
            "",
        ) {
            Ok(page) => (
                page["total"].as_u64().unwrap_or(0) as usize,
                model::history_rows(&page).into_iter().take(5).collect(),
                None,
            ),
            Err(error) => (0, Vec::new(), Some(error.message)),
        };
        Ok(Executed {
            execution,
            problems_total,
            problems,
            diagnostics_error,
            outcome,
            label,
            totals,
            maintenance,
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
            ScheduleChange::Remove => crate::native::remove_schedule(a, id.clone()),
            ScheduleChange::Set {
                trigger,
                missed,
                approve_writes,
                approve_deletions,
                show_window,
            } => crate::native::prepare_schedule(a).and_then(|()| {
                a.schedule_save(
                    id.clone(),
                    trigger,
                    missed,
                    approve_writes,
                    approve_deletions,
                    show_window,
                )
            }),
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
            // One reconciliation for the batch, before any profile is moved.
            if moves.iter().any(|(_, schedule)| schedule.is_some())
                && let Err(e) = crate::native::prepare_agent(app.state_directory())
            {
                // If the window closed, there is no receiver left to notify.
                let _receiver_closed = tx.send(Err((done, e))).is_err();
                return;
            }
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
    pub(crate) generation: u64,
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
    filter: sync_orchestrator::HistoryFilter,
    query: String,
    generation: u64,
) -> impl Future<Output = Result<HistoryPage, Failure>> + use<> {
    call(app, move |a| {
        let value = a.history_page_filtered(id, &execution, offset, filter, &query)?;
        Ok(HistoryPage {
            generation,
            offset,
            total: value["total"].as_u64().unwrap_or(0) as usize,
            rows: model::history_rows(&value),
            execution,
        })
    })
}

/// One presentation poll: process availability and job progress are distinct.
pub(crate) type AgentObservation = (
    Option<sync_orchestrator::LiveJob>,
    sync_orchestrator::ResidentStatus,
    bool,
    Option<sync_orchestrator::LiveQueue>,
);

/// Resumes both pause and explicit process stop, then asks the existing native
/// preparation path to start the least-privilege task (never a child of GUI).
pub(crate) fn resume_agent(app: &Shared) -> impl Future<Output = Result<(), Failure>> + use<> {
    let app = Arc::clone(app);
    blocking(move || {
        sync_orchestrator::clear_agent_stop(app.state_directory())
            .map_err(|e| Failure::new("unavailable", e.to_string()))?;
        app.agent_cancel_clear().map_err(Failure::from)?;
        crate::native::prepare_agent(app.state_directory())
    })
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn startup_observes_a_busy_agent_without_initializing_or_unlocking_stores()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join("state");
        std::fs::create_dir(&state)?;
        let locks = temp.path().join("locks");
        let app = Application::open_with_lock_root(&state, &locks)?;
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(state.join("desktop.lock"))?;
        owner.try_lock()?;
        let _presence = sync_orchestrator::hold_agent_presence(&state)?;
        sync_orchestrator::publish_resident_status(
            &state,
            sync_orchestrator::ResidentPhase::Dispatching,
        )?;
        let live = sync_orchestrator::LiveJob {
            format: sync_orchestrator::LIVE_FORMAT,
            updated_at: 1,
            snapshot: sync_orchestrator::JobSnapshot {
                active: true,
                entries_done: 3,
                ..Default::default()
            },
        };
        std::fs::write(state.join("agent-live.json"), serde_json::to_vec(&live)?)?;
        let error = match Application::open_with_lock_root(&state, &locks) {
            Ok(_) => return Err("command lock was bypassed".into()),
            Err(error) => error,
        };
        let Opening::Waiting(wait) =
            opening_failure(&state, error).map_err(|e| std::io::Error::other(e.to_string()))?
        else {
            return Err("busy startup did not wait".into());
        };
        assert!(wait.resident.available);
        assert_eq!(
            wait.live
                .ok_or("missing live observation")?
                .snapshot
                .entries_done,
            3
        );
        assert!(!state.join("scheduler.sqlite").exists());
        assert!(app.preferences_save(app.preferences()?).is_err());
        assert!(app.schedules().is_err());
        drop(owner);
        let reopened = Application::open_with_lock_root(&state, &locks)?;
        assert!(
            load(&reopened, Vec::new())
                .map_err(|e| std::io::Error::other(e.to_string()))?
                .profiles
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn startup_does_not_retry_integrity_or_permission_failures() {
        for code in ["integrity", "unavailable", "invalid_input"] {
            let error = AppError {
                code: code.into(),
                message: "test failure".into(),
                profile: None,
            };
            let failure = opening_failure(std::path::Path::new("unused"), error);
            assert!(matches!(failure, Err(error) if error.code == code));
        }
    }
}
