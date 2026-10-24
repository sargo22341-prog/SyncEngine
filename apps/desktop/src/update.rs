//! Message dispatch: the single place where messages change state.

use crate::anim::{Ease, Tween};
use crate::app::{App, Dialog, Drag, LastRun, Menu, Message, Nav, PreviewState, ToastKind};
use crate::backend::{self, Failure, Opened};
use crate::draft::{ProfileDraft, ScheduleDraft};
use crate::model::{self, Outcome};
use crate::native;
use crate::titlebar;
use crate::tray::TrayEvent;
use crate::ui_prefs::CloseAction;
use crate::wizard::Wizard;
use iced::{Task, window};
use std::time::Instant;
use sync_core::ProfileId;
use sync_orchestrator::DeletionCeiling;

impl App {
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.dispatch(message);
        let title = self.title();
        if let Ok(tray) = &mut self.tray {
            tray.set_tooltip(&title);
        }
        task
    }

    fn dispatch(&mut self, message: Message) -> Task<Message> {
        self.now = Instant::now();
        match message {
            Message::Booted(Ok(Opened(app, ws))) => {
                // Without the marker the agent may open a second window: only
                // a presentation nuisance, never a data risk.
                self.presence = sync_orchestrator::hold_desktop_presence(app.state_directory())
                    .ok()
                    .flatten();
                self.backend = Some(app);
                self.apply_workspace(ws);
                self.page_t.replay(0.0, 1.0, self.now);
                if let Some(id) = self.ws.profiles.first().map(|p| p.id.clone())
                    && self.ws.profiles.len() == 1
                {
                    self.selected = Some(id);
                }
                Task::none()
            }
            Message::Booted(Err(e)) => {
                self.boot_error = Some(e);
                Task::none()
            }
            Message::Retry => {
                self.boot_error = None;
                Task::perform(backend::open(), Message::Booted)
            }
            Message::Frame(at) => {
                self.now = self.now.max(at);
                self.tick();
                Task::none()
            }
            Message::Cursor(p) => {
                // Content coordinates: overlays are laid out below the bar.
                let p = iced::Point::new(p.x, p.y - titlebar::HEIGHT);
                self.cursor = p;
                if self.frame.press.is_some() {
                    return self.drag_window(p);
                }
                if let Some(d) = &mut self.drag
                    && !d.active
                    && (p.x - d.origin.x).hypot(p.y - d.origin.y) > 6.0
                {
                    d.active = true;
                    self.menu = None;
                }
                Task::none()
            }
            Message::Released => {
                self.frame.press = None;
                self.release()
            }
            Message::Key(key, digit, modifiers, captured) => {
                self.key(key, digit, modifiers, captured)
            }
            Message::Resized(size) => {
                // Windows reports a minimized window (taskbar, Win+D, system
                // menu) as resized to nothing: send it to the notification area.
                if size.width == 0.0
                    && size.height == 0.0
                    && let Some(id) = self.window_id
                {
                    return self.minimize_window(id);
                }
                self.window =
                    iced::Size::new(size.width, (size.height - titlebar::HEIGHT).max(0.0));
                self.sync_maximized()
            }
            Message::SystemTheme(mode) => {
                self.system_dark = mode != iced::theme::Mode::Light;
                self.apply_theme();
                Task::none()
            }
            Message::WindowOpened(id) => {
                self.window_id = Some(id);
                self.sync_maximized()
            }
            Message::Chrome(action) => self.chrome(action),
            Message::CloseRequested(id) => {
                self.window_id = Some(id);
                match self.prefs.close {
                    CloseAction::Minimize => self.minimize_window(id),
                    CloseAction::Quit => self.quit(),
                }
            }
            Message::Tray(TrayEvent::Show) => self.show_window(),
            Message::Tray(TrayEvent::Quit) | Message::Quit => self.quit(),
            Message::AgentTick => self.poll_agent(),
            Message::AgentLive(result) => self.agent_observed(result),
            Message::CancelAgent => self.cancel_agent(),
            Message::AgentCancelSent(result) => {
                match result {
                    Ok(()) => self.toast(
                        ToastKind::Info,
                        "Arrêt demandé à l’agent",
                        "La synchronisation planifiée s’arrête proprement à la fin de l’opération en cours.",
                    ),
                    Err(e) => {
                        if let Some(job) = &mut self.agent_job {
                            job.cancel_requested = false;
                        }
                        self.fail(&e);
                    }
                }
                Task::none()
            }
            Message::ResumeAgent => match &self.backend {
                Some(app) => Task::perform(
                    backend::call(app, sync_orchestrator::Application::agent_cancel_clear),
                    Message::AgentResumed,
                ),
                None => Task::none(),
            },
            Message::AgentResumed(result) => {
                if let Err(e) = result {
                    self.fail(&e);
                }
                self.refresh()
            }
            Message::Nav(nav) => {
                self.go(nav);
                if nav == Nav::History {
                    self.expanded_run = None;
                    self.history = None;
                }
                Task::none()
            }
            Message::Select(id) => {
                if self.selected != id {
                    self.selected = id;
                    self.expanded_run = None;
                    self.history = None;
                    self.page_t.replay(0.0, 1.0, self.now);
                }
                if matches!(self.nav, Nav::Settings | Nav::Activity) {
                    self.go(Nav::Profiles);
                }
                Task::none()
            }
            Message::Search(q) => {
                self.search = q;
                Task::none()
            }
            Message::ToggleGroup(g) => {
                if let Some(i) = self.prefs.collapsed.iter().position(|c| *c == g) {
                    self.prefs.collapsed.remove(i);
                } else {
                    self.prefs.collapsed.push(g);
                }
                self.save_prefs();
                Task::none()
            }
            Message::Hover(id) => {
                if self.hovered != id && self.drag.as_ref().is_none_or(|d| !d.active) {
                    self.set_hover(id);
                }
                Task::none()
            }
            Message::Refresh => self.refresh(),
            Message::Refreshed(result) => {
                self.busy_refresh = false;
                match result {
                    Ok(ws) => self.apply_workspace(ws),
                    Err(e) => self.fail(&e),
                }
                if let Some(id) = self.close_after.filter(|_| self.job.is_none()) {
                    return window::close(id);
                }
                Task::none()
            }
            Message::Press(id) => {
                self.drag = Some(Drag {
                    profile: id,
                    origin: self.cursor,
                    active: false,
                });
                Task::none()
            }
            Message::EnterZone(g) => {
                self.drop_zone = Some(g);
                self.drop_new = false;
                Task::none()
            }
            Message::LeaveZone(g) => {
                if self.drop_zone.as_deref() == Some(g.as_str()) {
                    self.drop_zone = None;
                }
                Task::none()
            }
            Message::EnterNewZone => {
                self.drop_new = true;
                self.drop_zone = None;
                Task::none()
            }
            Message::OpenMenu(target) => {
                self.drag = None;
                let mut enter = Tween::new(0.0, 160, Ease::OutCubic);
                enter.go(1.0, self.now);
                self.menu = Some(Menu {
                    target,
                    at: self.cursor,
                    enter,
                });
                Task::none()
            }
            Message::CloseMenu => {
                self.menu = None;
                Task::none()
            }
            Message::Menu(inner) => {
                self.menu = None;
                self.update(*inner)
            }
            Message::Simulate(id) => self.simulate(id, DeletionCeiling::Guarded),
            Message::LiftDeletionCeiling(id) => self.simulate(id, DeletionCeiling::Lifted),
            Message::Previewed(id, result) => {
                self.job = None;
                match result {
                    Ok(view) => {
                        let mut reveal = Tween::new(0.0, 900, Ease::OutCubic);
                        reveal.go(1.0, self.now);
                        self.preview = Some(PreviewState {
                            ops: view.first.clone(),
                            total: usize::try_from(view.summary.total()).unwrap_or(usize::MAX),
                            view,
                            filter: None,
                            query: String::new(),
                            loading: false,
                            reveal,
                        });
                        let all = self
                            .preview
                            .as_ref()
                            .map(|p| p.view.summary)
                            .unwrap_or_default();
                        self.toast(
                            ToastKind::Info,
                            "Simulation terminée",
                            format!(
                                "{} changement(s) prévu(s), aucun fichier modifié.",
                                model::count(all.mutations())
                            ),
                        );
                        self.selected = Some(id);
                        if self.nav != Nav::Profiles {
                            self.go(Nav::Profiles);
                        }
                    }
                    Err(e) => {
                        self.fail(&e);
                    }
                }
                self.finish_job_tasks()
            }
            Message::PlanFilter(filter) => {
                if let Some(p) = &mut self.preview {
                    p.filter = filter;
                }
                self.load_plan_page(0)
            }
            Message::PlanQuery(q) => {
                if let Some(p) = &mut self.preview {
                    p.query = q.chars().take(512).collect();
                }
                self.load_plan_page(0)
            }
            Message::PlanMore => {
                let offset = self.preview.as_ref().map_or(0, |p| p.ops.len());
                self.load_plan_page(offset)
            }
            Message::PlanPage(generation, result) => {
                if generation != self.plan_generation {
                    return Task::none();
                }
                match result {
                    Ok(page) => {
                        if let Some(p) = &mut self.preview {
                            if page.offset == 0 {
                                p.ops = page.ops;
                            } else {
                                p.ops.extend(page.ops);
                            }
                            p.total = page.total;
                            p.loading = false;
                        }
                    }
                    Err(e) => {
                        if let Some(p) = &mut self.preview {
                            p.loading = false;
                        }
                        if e.code == "stale" {
                            self.preview = None;
                        }
                        self.fail(&e);
                    }
                }
                Task::none()
            }
            Message::DiscardPreview => {
                self.preview = None;
                Task::none()
            }
            Message::AskExecute => {
                if let Some(p) = &self.preview {
                    let profile = ProfileId::new(p.view.profile.clone()).ok();
                    if let Some(profile) = profile {
                        self.open_dialog(Dialog::Execute {
                            profile,
                            confirm_delete: false,
                        });
                    }
                }
                Task::none()
            }
            Message::ToggleExecuteDelete(v) => {
                if let Some(Dialog::Execute { confirm_delete, .. }) = &mut self.dialog {
                    *confirm_delete = v;
                }
                Task::none()
            }
            Message::Execute => self.execute(),
            Message::Executed(result) => {
                let Some(job) = self.job.take() else {
                    return Task::none();
                };
                match &result {
                    Ok(done) => {
                        let kind = match done.outcome {
                            Outcome::Success => ToastKind::Success,
                            Outcome::Warning => ToastKind::Warning,
                            _ => ToastKind::Error,
                        };
                        self.toast(
                            kind,
                            format!("{} — {}", job.name, done.label),
                            "Le détail complet est conservé dans l’historique.",
                        );
                    }
                    Err(e) => self.fail(e),
                }
                self.last_run = Some(LastRun {
                    profile: job.profile,
                    name: job.name,
                    result,
                    duration: self.now.saturating_duration_since(job.started),
                });
                self.finish_job_tasks()
            }
            Message::Cancel => {
                if let (Some(job), Some(app)) = (&mut self.job, &self.backend) {
                    job.cancel_requested = true;
                    if let Err(e) = app.cancel() {
                        let f = Failure::from(e);
                        self.fail(&f);
                    }
                }
                Task::none()
            }
            Message::New(group) => {
                let group = group.unwrap_or_default();
                self.wizard = Some(Wizard::open(
                    ProfileDraft::new(&group),
                    self.groups(),
                    self.now,
                ));
                self.menu = None;
                Task::none()
            }
            Message::Edit(id) => {
                if let Some(p) = self.profile(&id) {
                    let draft = ProfileDraft::edit(p, self.schedule_of(&id).map(|s| &s.schedule));
                    self.wizard = Some(Wizard::open(draft, self.groups(), self.now));
                }
                Task::none()
            }
            Message::Duplicate(id) => {
                if let Some(p) = self.profile(&id) {
                    let draft = ProfileDraft::duplicate(p);
                    self.wizard = Some(Wizard::open(draft, self.groups(), self.now));
                }
                Task::none()
            }
            Message::AskRemove(id) => {
                self.open_dialog(Dialog::Remove { profile: id });
                Task::none()
            }
            Message::Remove(id) => {
                self.close_dialog();
                let Some(app) = &self.backend else {
                    return Task::none();
                };
                let scheduled = self.schedule_of(&id).is_some();
                if self.selected.as_ref() == Some(&id) {
                    self.selected = None;
                }
                if self
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.view.profile == id.as_str())
                {
                    self.preview = None;
                }
                Task::perform(
                    backend::call(app, move |a| {
                        if scheduled {
                            a.schedule_remove(id.clone())?;
                        }
                        a.profile_remove(id)
                    }),
                    Message::Removed,
                )
            }
            Message::Removed(result) => {
                match result {
                    Ok(()) => self.toast(
                        ToastKind::Success,
                        "Profil retiré",
                        "Les fichiers, l’historique et les versions sont conservés.",
                    ),
                    Err(e) => self.fail(&e),
                }
                self.refresh()
            }
            Message::MoveTo(id, group) => self.move_to(vec![id], &group),
            Message::Moved(result) => {
                match result {
                    Ok(names) if !names.is_empty() => {
                        self.toast(ToastKind::Success, "Profil déplacé", names.join(", "));
                    }
                    Ok(_) => {}
                    Err((done, e)) => {
                        let body = if done.is_empty() {
                            e.message.clone()
                        } else {
                            format!("{} Déjà déplacés : {}.", e.message, done.join(", "))
                        };
                        self.toast(ToastKind::Error, e.title(), body);
                    }
                }
                self.refresh()
            }
            Message::NewGroup(pending) => {
                self.open_dialog(Dialog::Group {
                    original: None,
                    name: String::new(),
                    pending,
                    error: None,
                });
                Task::none()
            }
            Message::RenameGroup(g) => {
                self.open_dialog(Dialog::Group {
                    original: Some(g.clone()),
                    name: g,
                    pending: None,
                    error: None,
                });
                Task::none()
            }
            Message::GroupName(v) => {
                if let Some(Dialog::Group { name, error, .. }) = &mut self.dialog {
                    *name = v.chars().take(200).collect();
                    *error = None;
                }
                Task::none()
            }
            Message::SubmitGroup => self.submit_group(),
            Message::ScheduleGroup(g) => {
                let targets: Vec<ProfileId> = self
                    .ws
                    .profiles
                    .iter()
                    .filter(|p| p.group == g)
                    .map(|p| p.id.clone())
                    .collect();
                if targets.is_empty() {
                    self.toast(
                        ToastKind::Info,
                        "Groupe vide",
                        "Ajoutez d’abord des profils à ce groupe.",
                    );
                    return Task::none();
                }
                let draft = ScheduleDraft::from_state(
                    targets
                        .first()
                        .and_then(|id| self.schedule_of(id))
                        .map(|s| &s.schedule),
                );
                let title = if g.is_empty() {
                    "Sans groupe".to_owned()
                } else {
                    g
                };
                self.open_dialog(Dialog::Schedule {
                    targets,
                    title: format!("Planifier le groupe « {title} »"),
                    draft,
                    error: None,
                    saving: false,
                });
                Task::none()
            }
            Message::Wizard(msg) => self.wizard_update(msg),
            Message::Saved(result, simulate) => {
                if let Some(w) = &mut self.wizard {
                    w.saving = false;
                }
                match result {
                    Ok(saved) => {
                        let name = self
                            .wizard
                            .as_ref()
                            .map(|w| w.draft.name.clone())
                            .unwrap_or_default();
                        if let Some(e) = &saved.schedule_error {
                            self.toast(
                                ToastKind::Warning,
                                "Profil enregistré, planification refusée",
                                e.message.clone(),
                            );
                        } else {
                            self.toast(ToastKind::Success, "Profil enregistré", name);
                        }
                        if let Some(w) = self.wizard.as_ref() {
                            let g = w.draft.group.trim().to_owned();
                            self.prefs.groups.retain(|x| *x != g);
                        }
                        self.wizard = None;
                        self.selected = Some(saved.id.clone());
                        self.preview = None;
                        self.go(Nav::Profiles);
                        let refresh = self.refresh();
                        if simulate {
                            return Task::batch([refresh, Task::done(Message::Simulate(saved.id))]);
                        }
                        refresh
                    }
                    Err(e) => {
                        if let Some(w) = &mut self.wizard {
                            w.error = Some(format!("{} — {}", e.title(), e.message));
                        }
                        Task::none()
                    }
                }
            }
            Message::OpenSchedule(id) => {
                let Some(p) = self.profile(&id) else {
                    return Task::none();
                };
                let title = format!("Planifier « {} »", p.name);
                let draft = ScheduleDraft::from_state(self.schedule_of(&id).map(|s| &s.schedule));
                self.open_dialog(Dialog::Schedule {
                    targets: vec![id],
                    title,
                    draft,
                    error: None,
                    saving: false,
                });
                Task::none()
            }
            Message::ScheduleEdit(msg) => {
                if let Some(Dialog::Schedule { draft, error, .. }) = &mut self.dialog {
                    crate::schedules::apply(draft, msg);
                    *error = None;
                }
                Task::none()
            }
            Message::SubmitSchedule => self.submit_schedule(),
            Message::ScheduleSaved(result) => {
                match result {
                    Ok(names) => {
                        self.close_dialog();
                        self.toast(
                            ToastKind::Success,
                            "Planification enregistrée",
                            names.join(", "),
                        );
                    }
                    Err((done, e)) => {
                        let message = if done.is_empty() {
                            e.message.clone()
                        } else {
                            format!("{} Déjà enregistrés : {}.", e.message, done.join(", "))
                        };
                        if let Some(Dialog::Schedule { error, saving, .. }) = &mut self.dialog {
                            *error = Some(message);
                            *saving = false;
                        } else {
                            self.toast(ToastKind::Error, e.title(), message);
                        }
                    }
                }
                self.refresh()
            }
            Message::RemoveSchedule(id) => {
                let Some(app) = &self.backend else {
                    return Task::none();
                };
                Task::perform(
                    backend::call(app, move |a| a.schedule_remove(id)),
                    Message::ScheduleRemoved,
                )
            }
            Message::ScheduleRemoved(result) => {
                match result {
                    Ok(()) => self.toast(
                        ToastKind::Info,
                        "Planification retirée",
                        "Le profil ne sera plus lancé automatiquement.",
                    ),
                    Err(e) => self.fail(&e),
                }
                self.refresh()
            }
            Message::ActivateAgent => {
                let Some(dir) = self
                    .backend
                    .as_ref()
                    .map(|a| a.state_directory().to_path_buf())
                else {
                    return Task::none();
                };
                Task::perform(
                    backend::blocking(move || native::activate_agent(&dir)),
                    Message::AgentActivated,
                )
            }
            Message::AgentActivated(result) => {
                match result {
                    Ok(()) => self.toast(
                        ToastKind::Success,
                        "Agent activé",
                        "Les tâches Windows de votre compte déclencheront les horaires autorisés, même fenêtre fermée.",
                    ),
                    Err(e) => self.fail(&e),
                }
                Task::none()
            }
            Message::ShowHistory(id) => {
                self.selected = Some(id);
                self.expanded_run = None;
                self.history = None;
                self.go(Nav::History);
                Task::none()
            }
            Message::ExpandRun(execution) => {
                if self.expanded_run.as_deref() == Some(execution.as_str()) {
                    self.expanded_run = None;
                    self.history = None;
                    return Task::none();
                }
                self.expanded_run = Some(execution.clone());
                self.history = None;
                self.load_history(execution, 0)
            }
            Message::HistoryOffset(offset) => match self.expanded_run.clone() {
                Some(execution) => self.load_history(execution, offset),
                None => Task::none(),
            },
            Message::HistoryPage(result) => {
                match result {
                    Ok(page) if self.expanded_run.as_deref() == Some(page.execution.as_str()) => {
                        self.history = Some(page);
                    }
                    Ok(_) => {}
                    Err(e) => self.fail(&e),
                }
                Task::none()
            }
            Message::Theme(choice) => {
                self.prefs.theme = choice;
                self.save_prefs();
                self.apply_theme();
                Task::none()
            }
            Message::CloseBehavior(action) => {
                self.prefs.close = action;
                self.save_prefs();
                Task::none()
            }
            Message::Compact(compact) => {
                let Some(app) = &self.backend else {
                    return Task::none();
                };
                self.ws.preferences.compact = compact;
                let mut preferences = self.ws.preferences.clone();
                preferences.compact = compact;
                Task::perform(
                    backend::call(app, move |a| a.preferences_save(preferences)),
                    Message::PreferencesSaved,
                )
            }
            Message::PreferencesSaved(result) => {
                if let Err(e) = result {
                    self.fail(&e);
                    return self.refresh();
                }
                Task::none()
            }
            Message::StartupLoaded(result) => {
                self.startup = result.ok();
                Task::none()
            }
            Message::Startup(enabled) => Task::perform(
                backend::blocking(move || native::startup_save(enabled)),
                move |r| Message::StartupSaved(enabled, r),
            ),
            Message::StartupSaved(enabled, result) => {
                match result {
                    Ok(()) => self.startup = Some(enabled),
                    Err(e) => self.fail(&e),
                }
                Task::none()
            }
            Message::CloseDialog => {
                self.close_dialog();
                Task::none()
            }
            Message::ShowShortcuts => {
                self.open_dialog(Dialog::Shortcuts);
                Task::none()
            }
            Message::DismissToast(id) => {
                self.toasts.retain(|t| t.id != id);
                Task::none()
            }
            Message::CopyText(value) => {
                self.toast(ToastKind::Info, "Copié", model::ellipsize(&value, 60));
                iced::clipboard::write(value)
            }
            Message::Noop => Task::none(),
        }
    }
}

impl App {
    /// Restores and focuses the main window (icon click, pending quit,
    /// followed scheduled run), including from the notification area.
    pub(crate) fn show_window(&self) -> Task<Message> {
        match self.window_id {
            Some(id) => Task::batch([
                window::set_mode(id, window::Mode::Windowed),
                window::minimize(id, false),
                window::gain_focus(id),
            ]),
            None => Task::none(),
        }
    }

    /// Minimizing hides the window from the taskbar: it lives in the
    /// notification area until restored from its icon. Without an icon the
    /// window is only minimized, so that it can always be found again.
    pub(crate) fn minimize_window(&self, id: window::Id) -> Task<Message> {
        if self.tray.is_ok() {
            window::set_mode(id, window::Mode::Hidden)
        } else {
            window::minimize(id, true)
        }
    }

    /// Leaves the application. A running task is cancelled first and the
    /// window closes only once its state is durable (see `Refreshed`).
    fn quit(&mut self) -> Task<Message> {
        let Some(id) = self.window_id else {
            return iced::exit();
        };
        let Some(job) = &mut self.job else {
            return window::close(id);
        };
        if self.close_after.is_some() {
            return self.show_window();
        }
        job.cancel_requested = true;
        self.close_after = Some(id);
        if let Some(app) = &self.backend
            && let Err(e) = app.cancel()
        {
            let f = Failure::from(e);
            self.fail(&f);
        }
        self.toast(
            ToastKind::Warning,
            "Fermeture après annulation",
            "La tâche en cours est annulée proprement ; SyncEngine se fermera dès que l’état est enregistré.",
        );
        self.show_window()
    }
}
