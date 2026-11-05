//! Application state machine (Elm architecture). The update function only
//! orchestrates facade calls and presentation state; engines decide.

use crate::anim::{Ease, Tween};
use crate::backend::{
    self, Executed, Failure, HistoryPage, Opening, PlanPage, Saved, ScheduleView, Shared, Workspace,
};
use crate::charts;
use crate::draft::ScheduleDraft;
use crate::model::{self, PreviewView};
use crate::native;
use crate::theme::Palette;
use crate::titlebar::{self, Chrome};
use crate::tray::{self, Tray, TrayEvent};
use crate::ui_prefs::{CloseAction, ThemeChoice, UiPrefs};
use crate::wizard::{Wizard, WizardMsg};
use iced::keyboard::{self, Key, Modifiers};
use iced::{Point, Size, Subscription, Task, event, mouse, time, window};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use sync_core::ProfileId;
use sync_orchestrator::JobSnapshot;
use sync_plan::Action;
use sync_profiles::Profile;

/// Top-level sections of the rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nav {
    Profiles,
    Activity,
    History,
    Schedules,
    Trash,
    Settings,
}

impl Nav {
    pub(crate) const ALL: [Self; 6] = [
        Self::Profiles,
        Self::Activity,
        Self::History,
        Self::Schedules,
        Self::Trash,
        Self::Settings,
    ];
    pub(crate) fn index(self) -> usize {
        Self::ALL.iter().position(|n| *n == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobKind {
    Preview,
    Execute,
}

/// One live observation kept for the charts (bounded by time, see
/// [`crate::speed::RETAIN`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sample {
    pub(crate) at: Instant,
    /// Bytes transferred block by block (not only validated files).
    pub(crate) bytes: u64,
    /// Part of `bytes` written toward the source folder (bidirectional).
    pub(crate) to_source: u64,
    /// Completed operations (folders created, files validated, deletions).
    pub(crate) operations: u64,
    pub(crate) scanned: u64,
    /// Copies too short to be listed (see `JobSnapshot::quick_transfers`).
    pub(crate) quick: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub(crate) kind: JobKind,
    pub(crate) profile: ProfileId,
    pub(crate) name: String,
    pub(crate) started: Instant,
    pub(crate) snapshot: JobSnapshot,
    pub(crate) samples: VecDeque<Sample>,
    pub(crate) cancel_requested: bool,
    /// Scheduled run observed from the agent's published snapshot, not a job
    /// of this window: cancelling it leaves a stop request for the agent.
    pub(crate) agent: bool,
    /// Planned mutations (execution only), for an honest progress ratio.
    pub(crate) planned: Option<u64>,
    pub(crate) planned_bytes: Option<u64>,
}

impl Job {
    /// Overall progress in `0.0..=1.0`, `None` while unknown (scan phase).
    ///
    /// Execution weighs bytes and operations together, as FreeFileSync does
    /// (`(bytes + items) / (bytes total + items total)`): volume dominates,
    /// so thousands of thumbnails copied before a few large videos honestly
    /// stay near the start, while a plan of folders or deletions only still
    /// moves. Block-level bytes, like the throughput curve.
    pub(crate) fn progress(&self) -> Option<f32> {
        let s = &self.snapshot;
        match self.kind {
            JobKind::Execute => model::fraction(
                s.bytes_transferred.saturating_add(s.entries_done),
                self.planned_bytes
                    .unwrap_or(0)
                    .saturating_add(self.planned.unwrap_or(0)),
            ),
            // Known only for the content phase (files of equal size).
            JobKind::Preview => model::fraction(s.entries_hashed, s.hash_target),
        }
    }

    /// « reste ≈ 6 min », once the pace is meaningful.
    pub(crate) fn remaining_label(&self, now: Instant) -> Option<String> {
        model::remaining_secs(
            self.progress(),
            now.saturating_duration_since(self.started).as_secs(),
        )
        .map(model::remaining_label)
    }
}

/// Exact cached preview plus its current filtered page.
#[derive(Debug, Clone)]
pub(crate) struct PreviewState {
    pub(crate) view: PreviewView,
    pub(crate) filter: Option<Action>,
    pub(crate) query: String,
    pub(crate) ops: Vec<model::OpView>,
    pub(crate) total: usize,
    pub(crate) loading: bool,
    pub(crate) reveal: Tween,
    /// Folder view of the same filtered operations.
    pub(crate) tree: crate::tree::Tree<model::OpView>,
}

/// Result of the last execution launched from this window.
#[derive(Debug, Clone)]
pub(crate) struct LastRun {
    pub(crate) profile: ProfileId,
    pub(crate) name: String,
    pub(crate) result: Result<Executed, Failure>,
    pub(crate) duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct Toast {
    pub(crate) id: u64,
    pub(crate) kind: ToastKind,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) born: Instant,
    pub(crate) life: Duration,
    pub(crate) enter: Tween,
}

/// What a right-click targeted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MenuTarget {
    Profile(ProfileId),
    Group(String),
    Background,
}

#[derive(Debug, Clone)]
pub(crate) struct Menu {
    pub(crate) target: MenuTarget,
    pub(crate) at: Point,
    pub(crate) enter: Tween,
}

#[derive(Debug, Clone)]
pub(crate) struct Drag {
    pub(crate) profile: ProfileId,
    pub(crate) origin: Point,
    pub(crate) active: bool,
}

/// Modal dialogs.
#[derive(Debug, Clone)]
pub(crate) enum Dialog {
    Execute {
        profile: ProfileId,
        confirm_delete: bool,
        /// « Je comprends, supprimer définitivement » (trash too small).
        confirm_permanent: bool,
    },
    /// Content of a profile trash (« Voir la corbeille »).
    Trash(Box<crate::trash::TrashDialog>),
    /// « Exclure du profil » from a row of the history.
    Exclusion(Box<crate::exclusion::ExclusionDialog>),
    /// Executions interrupted by a crash, shown at start-up.
    Interrupted(Box<crate::interrupted::InterruptedDialog>),
    /// « Supprimer toutes les données de SyncEngine ».
    Purge(Box<crate::purge_view::PurgeDialog>),
    Remove {
        profile: ProfileId,
    },
    Diagnose {
        profile: ProfileId,
    },
    Group {
        original: Option<String>,
        name: String,
        /// Profile to move into the new group once named (drag to « new »).
        pending: Option<ProfileId>,
        error: Option<String>,
    },
    /// « Synchroniser tout le groupe » confirmation.
    GroupRun {
        group: String,
        profiles: Vec<ProfileId>,
        allow_deletions: bool,
    },
    /// Outcome of each profile of a group run.
    GroupReport {
        group: String,
        results: Vec<crate::group_run::GroupResult>,
        cancelled: bool,
    },
    Schedule {
        targets: Vec<ProfileId>,
        title: String,
        draft: ScheduleDraft,
        error: Option<String>,
        saving: bool,
    },
    Shortcuts,
    /// « Nouvelle mise à jour disponible » (see [`crate::app_update`]).
    Update,
}

/// Runs added to the global history per page.
pub(crate) const HISTORY_PAGE: usize = 30;

/// Messages of the whole application.
#[derive(Debug, Clone)]
pub(crate) enum Message {
    Booted(Result<Opening, Failure>),
    Retry,
    Frame(Instant),
    Cursor(Point),
    Released,
    /// Logical key, layout-independent digit row index, modifiers, captured.
    Key(Key, Option<usize>, Modifiers, bool),
    Resized(Size),
    SystemTheme(iced::theme::Mode),
    WindowOpened(window::Id),
    /// Size of the monitor of the window, to place the tracker window.
    MonitorKnown(window::Id, Option<Size>),
    CloseRequested(window::Id),
    Tray(TrayEvent),
    /// Integrated title bar and resize grips.
    Chrome(Chrome),
    /// Explicit exit (icon menu, settings, Ctrl+Q), whatever the close setting.
    Quit,
    Nav(Nav),
    Select(Option<ProfileId>),
    Search(String),
    ToggleGroup(String),
    Hover(Option<ProfileId>),
    Refresh,
    Refreshed(Result<Workspace, Failure>),
    // Drag and drop.
    Press(ProfileId),
    EnterZone(String),
    LeaveZone(String),
    EnterNewZone,
    // Context menu.
    OpenMenu(MenuTarget),
    CloseMenu,
    Menu(Box<Message>),
    // Profiles.
    Simulate(ProfileId),
    /// Re-simulates once with the deletion ceiling lifted (explicit user choice).
    LiftDeletionCeiling(ProfileId),
    Previewed(ProfileId, Result<PreviewView, Failure>),
    PlanFilter(Option<Action>),
    /// Adds a folder of the plan to the profile's excluded directories, then
    /// simulates again (the filters stay identical in preview and execution).
    ExcludeFolder(String),
    Exclusion(crate::exclusion::ExclusionMsg),
    /// Enlarges or shrinks the agent tracker card.
    MiniToggle,
    /// What to keep of a conflicting file; choosing the same again clears
    /// the choice.
    ConflictChoose(String, sync_orchestrator::ConflictKeep),
    /// Planning page: week calendar (`true`) or list.
    OverviewFilter(OverviewFilter),
    SettingsTab(SettingsTab),
    /// Performance settings (saved at once, read by every execution).
    PerfWorkers(u8),
    PerfLarge(u8),
    PerfRate(Option<u32>),
    RateInput(String),
    RateApply,
    /// Simulates again with the chosen conflicts resolved (nothing is written).
    ResolveConflicts,
    PlanQuery(String),
    PlanMore,
    PlanPage(u64, Result<PlanPage, Failure>),
    PlanTree(crate::tree::TreeMsg),
    PlanTreeLevel(
        crate::tree::Request,
        Result<crate::tree::Level<model::OpView>, Failure>,
    ),
    DiscardPreview,
    AskExecute,
    Execute,
    Executed(Result<Executed, Failure>),
    Cancel,
    SetPaused {
        agent: bool,
        control_id: Option<String>,
        paused: bool,
    },
    PauseSent(Result<(), Failure>),
    /// Periodic read of the agent's published snapshot.
    AgentTick,
    AgentLive(Result<backend::AgentObservation, Failure>),
    /// Stop request for the scheduled run followed in this window.
    CancelAgent,
    StopAgent,
    AgentCancelSent(Result<(), Failure>),
    /// Lifts a pending stop request so scheduled runs start again.
    ResumeAgent,
    AgentResumed(Result<(), Failure>),
    New(Option<String>),
    Edit(ProfileId),
    Duplicate(ProfileId),
    AskRemove(ProfileId),
    Remove(ProfileId),
    Removed(Result<(), Failure>),
    AskDiagnose(ProfileId),
    Diagnose(ProfileId),
    /// Journal clean-up warnings once acknowledged.
    Diagnosed(Result<Vec<String>, Failure>),
    Interrupted(crate::interrupted::InterruptedMsg),
    Purge(crate::purge_view::PurgeMsg),
    /// Application update (Réglages › « Mises à jour » and its dialog).
    AppUpdate(crate::app_update::UpdateMsg),
    MoveTo(ProfileId, String),
    Moved(backend::Batch),
    // Groups.
    NewGroup(Option<ProfileId>),
    RenameGroup(String),
    GroupName(String),
    SubmitGroup,
    ScheduleGroup(String),
    /// Simulates then executes every profile of a group, in a chain.
    AskGroupRun(String),
    GroupRunDeletions(bool),
    StartGroupRun,
    // Wizard.
    Wizard(WizardMsg),
    Saved(Result<Saved, Failure>, bool),
    // Schedules.
    OpenSchedule(ProfileId),
    ScheduleEdit(crate::schedules::ScheduleMsg),
    SubmitSchedule,
    ScheduleSaved(backend::Batch),
    RemoveSchedule(ProfileId),
    ScheduleRemoved(Result<(), Failure>),
    // History.
    ShowHistory(ProfileId),
    /// Shows the next page of the global history.
    HistoryMore,
    ShowRunProblems(ProfileId, String),
    HistoryFilter(sync_orchestrator::HistoryFilter),
    HistoryQuery(String),
    HistorySearch,
    ExpandRun(String),
    HistoryPage(Result<HistoryPage, Failure>),
    HistoryOffset(usize),
    /// « Dossiers » or « Liste » for the simulation and history lists.
    ListMode(crate::tree::ListMode),
    HistoryTree(crate::tree::TreeMsg),
    HistoryTreeLevel(
        String,
        crate::tree::Request,
        Result<crate::tree::Level<model::HistoryRow>, Failure>,
    ),
    // Settings.
    Theme(ThemeChoice),
    Compact(bool),
    CloseBehavior(CloseAction),
    PreferencesSaved(Result<(), Failure>),
    StartupLoaded(Result<bool, Failure>),
    Startup(bool),
    StartupSaved(bool, Result<(), Failure>),
    // Dialogs, toasts, misc.
    CloseDialog,
    ToggleExecuteDelete(bool),
    ToggleExecutePermanent(bool),
    Trash(crate::trash::TrashMsg),
    ShowShortcuts,
    DismissToast(u64),
    CopyText(String),
    /// Opens the state folder in the system file manager.
    OpenStateFolder,
}

/// Section of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SettingsTab {
    #[default]
    Appearance,
    Window,
    Performance,
    Keyboard,
    Guarantees,
    Data,
    Updates,
    About,
}

impl SettingsTab {
    pub(crate) const ALL: [Self; 8] = [
        Self::Appearance,
        Self::Window,
        Self::Performance,
        Self::Keyboard,
        Self::Guarantees,
        Self::Data,
        Self::Updates,
        Self::About,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Apparence",
            Self::Window => "Fenêtre et démarrage",
            Self::Performance => "Performances",
            Self::Keyboard => "Clavier",
            Self::Guarantees => "Garanties d’exécution",
            Self::Data => "Données",
            Self::Updates => "Mises à jour",
            Self::About => "Cette version",
        }
    }
}

/// Which profiles the overview lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum OverviewFilter {
    #[default]
    All,
    Running,
    Review,
}

/// Root state.
pub(crate) struct App {
    /// Filter chosen on the overview.
    pub(crate) overview_filter: OverviewFilter,
    /// Section shown by the settings page.
    pub(crate) settings_tab: SettingsTab,
    pub(crate) backend: Option<Shared>,
    pub(crate) boot_error: Option<Failure>,
    pub(crate) boot_wait: Option<backend::StartupObservation>,
    /// The tracker card is shown enlarged.
    pub(crate) mini_large: bool,
    /// Text typed for a custom rate limit (MiB/s).
    pub(crate) rate_input: String,
    /// Choices made on the conflicts of the displayed preview.
    pub(crate) conflict_choices: std::collections::HashMap<String, sync_orchestrator::ConflictKeep>,
    /// The window started as the compact tracker.
    pub(crate) started_mini: bool,
    pub(crate) boot_polling: bool,
    /// The search for interrupted executions found the state busy (a
    /// scheduled run owned it): it is repeated once the agent is idle.
    pub(crate) interruptions_unchecked: bool,
    pub(crate) boot_cancel_requested: bool,
    pub(crate) boot_control_error: Option<Failure>,
    pub(crate) agent_queue: Option<sync_orchestrator::LiveQueue>,
    pub(crate) state_dir: Option<PathBuf>,
    pub(crate) now: Instant,
    pub(crate) started: Instant,
    pub(crate) prefs: UiPrefs,
    pub(crate) system_dark: bool,
    pub(crate) theme_t: Tween,
    pub(crate) nav: Nav,
    pub(crate) nav_t: Tween,
    pub(crate) page_t: Tween,
    pub(crate) ws: Workspace,
    pub(crate) selected: Option<ProfileId>,
    pub(crate) search: String,
    pub(crate) hovered: Option<ProfileId>,
    pub(crate) lift: HashMap<ProfileId, Tween>,
    pub(crate) job: Option<Job>,
    /// Group simulated and executed profile after profile.
    pub(crate) group_run: Option<crate::group_run::GroupRun>,
    /// Scheduled run of the agent, followed live (see [`Job::agent`]).
    pub(crate) agent_job: Option<Job>,
    /// A read of the agent snapshot is in flight (no overlapping reads).
    pub(crate) agent_polling: bool,
    /// Presence lock of this window: the agent does not open a second one.
    pub(crate) presence: Option<std::fs::File>,
    pub(crate) last_poll: Instant,
    pub(crate) preview: Option<PreviewState>,
    pub(crate) plan_generation: u64,
    pub(crate) last_run: Option<LastRun>,
    pub(crate) expanded_run: Option<String>,
    /// Runs shown in the global history (grows by one page at a time).
    pub(crate) history_limit: usize,
    pub(crate) history: Option<HistoryPage>,
    /// Folder view of the expanded run, with the same filter and search.
    pub(crate) history_tree: crate::tree::Tree<model::HistoryRow>,
    /// Folders or flat list, for this session.
    pub(crate) list_mode: crate::tree::ListMode,
    pub(crate) history_filter: sync_orchestrator::HistoryFilter,
    pub(crate) history_query: String,
    pub(crate) history_generation: u64,
    pub(crate) wizard: Option<Wizard>,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) dialog_t: Tween,
    pub(crate) menu: Option<Menu>,
    pub(crate) drag: Option<Drag>,
    pub(crate) drop_zone: Option<String>,
    pub(crate) drop_new: bool,
    pub(crate) cursor: Point,
    pub(crate) window: Size,
    pub(crate) toasts: Vec<Toast>,
    pub(crate) next_toast: u64,
    pub(crate) startup: Option<bool>,
    pub(crate) close_after: Option<window::Id>,
    /// Main window, known once opened (minimize/restore/close target).
    pub(crate) window_id: Option<window::Id>,
    /// Logical size of the monitor showing the window, once known.
    pub(crate) monitor: Option<Size>,
    /// Integrated frame state (maximized, focused, pending bar press).
    pub(crate) frame: titlebar::Frame,
    /// Notification-area icon, or why none could be created.
    pub(crate) tray: Result<Tray, String>,
    pub(crate) busy_refresh: bool,
    /// Every trace was deleted: nothing may be read or written any more,
    /// the application only shows the report and closes.
    pub(crate) purged: bool,
    /// Update lookup and installation (nothing persisted).
    pub(crate) updates: crate::app_update::Updates,
}

/// Compact tracker size (window opened by the agent) and normal window size.
pub(crate) const MINI_SIZE: iced::Size = iced::Size::new(400.0, 600.0);
/// Gap kept between the tracker window and the screen edges. The bottom one
/// also clears a standard taskbar (the work area is not exposed by the
/// toolkit): on a taller or side taskbar the window stays fully visible.
pub(crate) const MINI_MARGIN: iced::Size = iced::Size::new(14.0, 64.0);
pub(crate) const FULL_SIZE: iced::Size = iced::Size::new(1280.0, 820.0);
pub(crate) const FULL_MIN_SIZE: iced::Size = iced::Size::new(980.0, 640.0);
pub(crate) const MINI_MIN_SIZE: iced::Size = iced::Size::new(360.0, 420.0);
/// Enlarged tracker, still the agent card (the interface opens at the end).
pub(crate) const MINI_LARGE_SIZE: iced::Size = iced::Size::new(1000.0, 760.0);
pub(crate) const SEARCH_ID: &str = "profile-search";
/// Command-line flag passed by `syncengine-agent` when it opens the window.
pub(crate) const FOLLOW_AGENT_ARG: &str = "--suivre-agent";
/// An active agent snapshot older than this (seconds) means the agent stopped.
pub(crate) const AGENT_STALE_SECS: i64 = 10;

impl App {
    pub(crate) fn boot() -> (Self, Task<Message>) {
        let now = Instant::now();
        let state_dir = backend::state_directory().ok();
        let (prefs, prefs_error) = match state_dir.as_deref().map(UiPrefs::load) {
            Some(Ok(p)) => (p, None),
            Some(Err(e)) => (UiPrefs::default(), Some(e)),
            None => (UiPrefs::default(), None),
        };
        let mut app = Self {
            backend: None,
            boot_error: None,
            boot_wait: None,
            overview_filter: OverviewFilter::default(),
            settings_tab: SettingsTab::default(),
            mini_large: false,
            rate_input: String::new(),
            conflict_choices: std::collections::HashMap::new(),
            started_mini: std::env::args().skip(1).any(|a| a == FOLLOW_AGENT_ARG),
            boot_polling: true,
            interruptions_unchecked: false,
            boot_cancel_requested: false,
            boot_control_error: None,
            agent_queue: None,
            state_dir,
            now,
            started: now,
            theme_t: Tween::new(0.0, 450, Ease::InOutCubic),
            prefs,
            system_dark: true,
            nav: Nav::Profiles,
            nav_t: Tween::new(0.0, 260, Ease::OutBack),
            page_t: Tween::new(1.0, 260, Ease::OutCubic),
            ws: Workspace::default(),
            selected: None,
            search: String::new(),
            hovered: None,
            lift: HashMap::new(),
            job: None,
            group_run: None,
            agent_job: None,
            agent_polling: false,
            presence: None,
            last_poll: now,
            preview: None,
            plan_generation: 0,
            last_run: None,
            expanded_run: None,
            history_limit: crate::app::HISTORY_PAGE,
            history: None,
            history_tree: crate::tree::Tree::default(),
            list_mode: crate::tree::ListMode::default(),
            history_filter: sync_orchestrator::HistoryFilter::All,
            history_query: String::new(),
            history_generation: 0,
            wizard: None,
            dialog: None,
            dialog_t: Tween::new(0.0, 220, Ease::OutCubic),
            menu: None,
            drag: None,
            drop_zone: None,
            drop_new: false,
            cursor: Point::ORIGIN,
            window: Size::new(1280.0, 820.0),
            toasts: Vec::new(),
            next_toast: 0,
            startup: None,
            close_after: None,
            window_id: None,
            monitor: None,
            frame: titlebar::Frame::default(),
            tray: Tray::create(charts::icon_rgba(), charts::ICON_SIZE),
            busy_refresh: false,
            purged: false,
            updates: crate::app_update::Updates {
                check: if app_update::Target::current().is_some() {
                    crate::app_update::CheckState::Checking
                } else {
                    crate::app_update::CheckState::Unsupported
                },
                ..Default::default()
            },
        };
        app.theme_t = Tween::new(app.theme_target(), 450, Ease::InOutCubic);
        if let Some(e) = prefs_error {
            app.toast(ToastKind::Warning, "Préférences d’affichage", e);
        }
        // Started by the agent for a scheduled run: open on its live follow-up.
        if std::env::args().skip(1).any(|a| a == FOLLOW_AGENT_ARG) {
            app.nav = Nav::Activity;
            app.nav_t = Tween::new(Nav::Activity.index() as f32, 260, Ease::OutBack);
        }
        let tasks = Task::batch([
            Task::perform(backend::open(), Message::Booted),
            iced::system::theme().map(Message::SystemTheme),
            Task::perform(
                backend::blocking(native::startup_enabled),
                Message::StartupLoaded,
            ),
            // Background lookup at each opening; a failure stays silent.
            crate::app_update::lookup(false),
        ]);
        (app, tasks)
    }

    pub(crate) fn wants_light(&self) -> bool {
        match self.prefs.theme {
            ThemeChoice::Light => true,
            ThemeChoice::Dark => false,
            ThemeChoice::System => !self.system_dark,
        }
    }

    /// Position of the chosen theme on the `Palette::blend` axis.
    pub(crate) fn theme_target(&self) -> f32 {
        if self.wants_light() { 1.0 } else { 0.0 }
    }

    pub(crate) fn palette(&self) -> Palette {
        Palette::blend(self.theme_t.at(self.now))
    }

    pub(crate) fn title(&self) -> String {
        match &self.job {
            Some(job) if job.kind == JobKind::Execute => {
                format!("SyncEngine — exécution de {}", job.name)
            }
            Some(job) => format!("SyncEngine — simulation de {}", job.name),
            None => match &self.agent_job {
                Some(job) => format!("SyncEngine — synchronisation planifiée de {}", job.name),
                None => "SyncEngine".into(),
            },
        }
    }

    /// Notification-area tooltip: the window title, with the progress and the
    /// time left while a job runs.
    pub(crate) fn tray_tooltip(&self) -> String {
        let title = self.title();
        let Some(job) = self.live_job() else {
            return title;
        };
        let Some(progress) = job.progress() else {
            return title;
        };
        match job.remaining_label(self.now) {
            Some(left) => format!("{title} — {} · {left}", model::percent(progress)),
            None => format!("{title} — {}", model::percent(progress)),
        }
    }

    pub(crate) fn profile(&self, id: &ProfileId) -> Option<&Profile> {
        self.ws.profiles.iter().find(|p| &p.id == id)
    }

    pub(crate) fn schedule_of(&self, id: &ProfileId) -> Option<&ScheduleView> {
        self.ws
            .schedules
            .iter()
            .find(|s| &s.schedule.approved.id == id)
    }

    /// An authorization is valid only for the exact current configuration.
    pub(crate) fn schedule_valid(&self, id: &ProfileId) -> Option<bool> {
        let s = self.schedule_of(id)?;
        Some(self.profile(id).is_some_and(|p| *p == s.schedule.approved))
    }

    pub(crate) fn last_run(&self, id: &ProfileId) -> Option<&model::RunView> {
        self.ws.runs.get(id).and_then(|r| r.first())
    }

    /// Groups in display order: named groups alphabetically, then ungrouped,
    /// including empty groups created in this UI.
    pub(crate) fn groups(&self) -> Vec<String> {
        let mut set: Vec<String> = self
            .ws
            .profiles
            .iter()
            .map(|p| p.group.clone())
            .chain(self.prefs.groups.iter().cloned())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        set.sort_by(|a, b| match (a.is_empty(), b.is_empty()) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a.to_lowercase().cmp(&b.to_lowercase()),
        });
        set
    }

    pub(crate) fn members(&self, group: &str) -> Vec<&Profile> {
        let q = self.search.trim().to_lowercase();
        let mut list: Vec<&Profile> = self
            .ws
            .profiles
            .iter()
            .filter(|p| p.group == group)
            .filter(|p| {
                q.is_empty()
                    || p.name.to_lowercase().contains(&q)
                    || model::source_of(p).to_lowercase().contains(&q)
                    || model::destination_of(p).to_lowercase().contains(&q)
            })
            .collect();
        list.sort_by_key(|p| p.name.to_lowercase());
        list
    }

    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.group_run.is_some()
    }

    /// All data is being or has been deleted: no read may recreate a file
    /// in the state directory (the agent observation creates its lock file).
    pub(crate) fn purging(&self) -> bool {
        self.purged || matches!(&self.dialog, Some(Dialog::Purge(d)) if d.running)
    }

    /// Job shown live: the one of this window first, else the agent's.
    /// Mode of the profile of a live job: from the profile list, or, while
    /// the window only follows the agent (the store is not opened), from the
    /// queue the agent publishes.
    pub(crate) fn job_mode(&self, job: &Job) -> Option<sync_plan::SyncMode> {
        self.profile(&job.profile).map(|p| p.mode).or_else(|| {
            self.agent_queue
                .as_ref()
                .and_then(|q| q.current.as_ref())
                .filter(|current| current.id == job.profile)
                .and_then(|current| current.mode)
        })
    }

    pub(crate) fn live_job(&self) -> Option<&Job> {
        self.job.as_ref().or(self.agent_job.as_ref())
    }

    /// Whether a live job (this window's or the agent's) runs on `id`.
    pub(crate) fn running_on(&self, id: &ProfileId) -> bool {
        [&self.job, &self.agent_job]
            .into_iter()
            .flatten()
            .any(|j| &j.profile == id)
    }

    pub(crate) fn toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<String>,
        body: impl Into<String>,
    ) {
        self.next_toast += 1;
        let mut enter = Tween::new(0.0, 380, Ease::OutBack);
        enter.go(1.0, self.now);
        let life = match kind {
            ToastKind::Error => Duration::from_secs(9),
            ToastKind::Warning => Duration::from_secs(7),
            _ => Duration::from_millis(4500),
        };
        self.toasts.push(Toast {
            id: self.next_toast,
            kind,
            title: title.into(),
            body: body.into(),
            born: self.now,
            life,
            enter,
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub(crate) fn fail(&mut self, failure: &Failure) {
        self.toast(ToastKind::Error, failure.title(), failure.message.clone());
    }

    pub(crate) fn refresh(&mut self) -> Task<Message> {
        let Some(app) = self.backend.as_ref().filter(|_| !self.purged) else {
            return Task::none();
        };
        self.busy_refresh = true;
        Task::perform(
            backend::refresh(app, self.ws.schedules.clone()),
            Message::Refreshed,
        )
    }

    pub(crate) fn save_prefs(&mut self) {
        if let Some(dir) = self.state_dir.as_ref().filter(|_| !self.purged)
            && let Err(e) = self.prefs.save(dir)
        {
            self.toast(
                ToastKind::Warning,
                "Préférences d’affichage non enregistrées",
                e,
            );
        }
    }

    pub(crate) fn open_dialog(&mut self, dialog: Dialog) {
        self.menu = None;
        self.dialog = Some(dialog);
        self.dialog_t.replay(0.0, 1.0, self.now);
    }

    pub(crate) fn go(&mut self, nav: Nav) {
        if self.nav != nav {
            self.nav = nav;
            self.nav_t.go(nav.index() as f32, self.now);
            self.page_t.replay(0.0, 1.0, self.now);
        }
    }

    pub(crate) fn set_hover(&mut self, id: Option<ProfileId>) {
        let now = self.now;
        if let Some(old) = self.hovered.take()
            && let Some(t) = self.lift.get_mut(&old)
        {
            t.go(0.0, now);
        }
        if let Some(id) = &id {
            self.lift
                .entry(id.clone())
                .or_insert_with(|| Tween::new(0.0, 180, Ease::OutCubic))
                .go(1.0, now);
        }
        self.hovered = id;
    }

    /// Whether any tween is still moving (drives the frame subscription).
    pub(crate) fn animating(&self) -> bool {
        let n = self.now;
        self.theme_t.running(n)
            || self.nav_t.running(n)
            || self.page_t.running(n)
            || self.dialog_t.running(n)
            || !self.toasts.is_empty()
            || self.menu.as_ref().is_some_and(|m| m.enter.running(n))
            || self.lift.values().any(|t| t.running(n))
            || self.preview.as_ref().is_some_and(|p| p.reveal.running(n))
            || self.wizard.as_ref().is_some_and(|w| w.animating(n))
            || self.drag.as_ref().is_some_and(|d| d.active)
    }

    pub(crate) fn subscription(&self) -> Subscription<Message> {
        let input = event::listen_with(|event, status, _window| match event {
            iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Some(Message::Cursor(position))
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                Some(Message::Released)
            }
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) => {
                use keyboard::key::{Code, Physical};
                // Physical digits so Ctrl+1…6 also work on AZERTY layouts.
                let digit = match physical_key {
                    Physical::Code(Code::Digit1 | Code::Numpad1) => Some(0),
                    Physical::Code(Code::Digit2 | Code::Numpad2) => Some(1),
                    Physical::Code(Code::Digit3 | Code::Numpad3) => Some(2),
                    Physical::Code(Code::Digit4 | Code::Numpad4) => Some(3),
                    Physical::Code(Code::Digit5 | Code::Numpad5) => Some(4),
                    Physical::Code(Code::Digit6 | Code::Numpad6) => Some(5),
                    _ => None,
                };
                Some(Message::Key(
                    key,
                    digit,
                    modifiers,
                    status == event::Status::Captured,
                ))
            }
            iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
            iced::Event::Window(window::Event::Focused) => {
                Some(Message::Chrome(Chrome::Focused(true)))
            }
            iced::Event::Window(window::Event::Unfocused) => {
                Some(Message::Chrome(Chrome::Focused(false)))
            }
            _ => None,
        });
        let mut subs = vec![
            input,
            window::open_events().map(Message::WindowOpened),
            window::close_requests().map(Message::CloseRequested),
            tray::events().map(Message::Tray),
            iced::system::theme_changes().map(Message::SystemTheme),
        ];
        if self.backend.is_none() && self.boot_wait.is_some() {
            subs.push(time::every(Duration::from_secs(1)).map(|_| Message::Retry));
        }
        if self.animating() {
            subs.push(window::frames().map(Message::Frame));
        } else if self.live_job().is_some() || self.backend.is_none() {
            // Live spinners and polling at a power-friendly ~25 fps.
            subs.push(time::every(Duration::from_millis(40)).map(Message::Frame));
        }
        if self.backend.is_some() && !self.purging() {
            // Scheduled runs of the agent: about as often as it publishes
            // while one is followed, rarely otherwise (one tiny file read).
            let every = if self.agent_job.is_some() { 500 } else { 1500 };
            subs.push(time::every(Duration::from_millis(every)).map(|_| Message::AgentTick));
        }
        if self.update_installing() {
            subs.push(
                time::every(Duration::from_millis(200))
                    .map(|_| Message::AppUpdate(crate::app_update::UpdateMsg::Tick)),
            );
        }
        Subscription::batch(subs)
    }
}
