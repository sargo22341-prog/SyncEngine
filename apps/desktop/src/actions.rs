//! Side effects behind messages: facade calls, drag and drop resolution,
//! dialogs submission and keyboard shortcuts.

use crate::app::{
    AGENT_STALE_SECS, App, Dialog, Job, JobKind, Message, Nav, SEARCH_ID, Sample, ToastKind,
};
use crate::backend::{self, Failure, ScheduleView, Workspace};
use crate::draft;
use crate::model;
use crate::speed;
use crate::tree::{self, ListMode};
use crate::ui_prefs::ThemeChoice;
use crate::wizard::WizardMsg;
use iced::Task;
use iced::keyboard::{Key, Modifiers, key::Named};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};
use sync_core::ProfileId;
use sync_orchestrator::{Application, DeletionCeiling, JobPhase, JobSnapshot};
use sync_profiles::Profile;

impl App {
    pub(crate) fn apply_theme(&mut self) {
        self.theme_t.go(self.theme_target(), self.now);
    }

    pub(crate) fn apply_workspace(&mut self, ws: Workspace) {
        let known: HashSet<String> = ws.profiles.iter().map(|p| p.group.clone()).collect();
        self.prefs.groups.retain(|g| !known.contains(g));
        if let Some(id) = &self.selected
            && !ws.profiles.iter().any(|p| &p.id == id)
        {
            self.selected = None;
        }
        self.ws = ws;
    }

    pub(crate) fn close_dialog(&mut self) {
        // A deletion in progress keeps its dialog until its report.
        if matches!(&self.dialog, Some(Dialog::Purge(d)) if d.running) {
            return;
        }
        // Likewise an update being installed.
        if matches!(self.dialog, Some(Dialog::Update)) && self.update_installing() {
            return;
        }
        self.dialog_t.go(0.0, self.now);
    }

    /// Advances animations, retires toasts, polls the job snapshot.
    pub(crate) fn tick(&mut self) {
        let now = self.now;
        self.toasts
            .retain(|t| now.saturating_duration_since(t.born) < t.life);
        if self.dialog.is_some() && self.dialog_t.target() == 0.0 && !self.dialog_t.running(now) {
            self.dialog = None;
            self.update_dialog_closed();
        }
        if let Some(w) = &mut self.wizard
            && w.closing
            && !w.animating(now)
        {
            self.wizard = None;
        }
        self.lift.retain(|_, t| t.running(now) || t.target() > 0.0);
        if now.saturating_duration_since(self.last_poll) >= Duration::from_millis(200) {
            self.last_poll = now;
            self.poll();
        }
    }

    pub(crate) fn poll(&mut self) {
        let (Some(job), Some(app)) = (&mut self.job, &self.backend) else {
            return;
        };
        if let Ok(snapshot) = app.snapshot() {
            job.samples.push_back(Sample {
                // Time of the read itself, not of the frame that triggered
                // it: a late frame would otherwise skew the windowed rate.
                at: Instant::now(),
                bytes: snapshot.bytes_transferred,
                operations: snapshot.entries_done,
                // Listing and content reading both count as analysis work.
                scanned: snapshot
                    .entries_scanned
                    .saturating_add(snapshot.entries_hashed),
                quick: snapshot.quick_transfers,
            });
            // Keep the drawn span plus one averaging window, never less than
            // two samples.
            while job.samples.len() > 2 && job.samples[1].at + speed::RETAIN <= self.now {
                job.samples.pop_front();
            }
            job.snapshot = snapshot;
        }
    }

    /// Reads the agent's published snapshot off the UI thread.
    pub(crate) fn poll_agent(&mut self) -> Task<Message> {
        let Some(app) = &self.backend else {
            return Task::none();
        };
        if self.agent_polling || self.purging() {
            return Task::none();
        }
        self.agent_polling = true;
        Task::perform(
            backend::call(app, |app| {
                Ok((
                    app.agent_live()?,
                    sync_orchestrator::resident_status(app.state_directory())?,
                    app.agent_cancel_pending()
                        || sync_orchestrator::agent_stop_pending(app.state_directory()),
                    sync_orchestrator::LiveQueue::read(app.state_directory())?,
                ))
            }),
            Message::AgentLive,
        )
    }

    /// Follows a scheduled run from the agent's snapshot. An unreadable read
    /// (file being replaced) keeps the current view; a missing, finished or
    /// stale snapshot ends the follow-up with an explicit outcome.
    pub(crate) fn agent_observed(
        &mut self,
        result: Result<backend::AgentObservation, Failure>,
    ) -> Task<Message> {
        self.agent_polling = false;
        let (live, resident, paused, queue) = match result {
            Ok(value) => value,
            Err(error) => {
                self.ws.resident.phase = sync_orchestrator::ResidentPhase::Error;
                self.ws.supervision_error = Some(error);
                return Task::none();
            }
        };
        self.ws.resident = resident;
        self.agent_queue = queue;
        self.ws.agent_stopped = paused;
        let Some(live) = live
            .clone()
            .filter(|l| l.is_live(model::now_secs(), AGENT_STALE_SECS))
        else {
            return self.agent_ended(live.map(|l| l.snapshot));
        };
        let s = live.snapshot;
        let Some(profile) = s.profile.clone() else {
            return Task::none();
        };
        let kind = if s.phase == JobPhase::Executing {
            JobKind::Execute
        } else {
            JobKind::Preview
        };
        let sample = Sample {
            at: self.now,
            bytes: s.bytes_transferred,
            operations: s.entries_done,
            scanned: s.entries_scanned.saturating_add(s.entries_hashed),
            quick: s.quick_transfers,
        };
        let name = self
            .agent_queue
            .as_ref()
            .and_then(|queue| queue.current.as_ref())
            .filter(|current| current.id == profile)
            .map(|current| current.name.clone())
            .or_else(|| self.profile(&profile).map(|p| p.name.clone()))
            .unwrap_or_else(|| "Profil planifié".to_owned());
        let mut task = Task::none();
        match &mut self.agent_job {
            Some(job) if job.profile == profile => {
                job.name = name.clone();
                if job.kind != kind {
                    // Analysis then writes: rates restart with the new unit.
                    job.kind = kind;
                    job.samples.clear();
                    job.started = self.now;
                }
                job.samples.push_back(sample);
                while job.samples.len() > 2 && job.samples[1].at + speed::RETAIN <= self.now {
                    job.samples.pop_front();
                }
            }
            _ => {
                if self
                    .schedule_of(&profile)
                    .is_some_and(|s| s.schedule.show_window)
                {
                    self.go(Nav::Activity);
                    task = self.show_window();
                }
                self.agent_job = Some(Job {
                    kind,
                    profile,
                    name,
                    started: self.now,
                    snapshot: JobSnapshot::default(),
                    samples: VecDeque::from([sample]),
                    cancel_requested: false,
                    agent: true,
                    planned: None,
                    planned_bytes: None,
                });
            }
        }
        if let Some(job) = &mut self.agent_job {
            if kind == JobKind::Execute {
                job.planned = Some(s.planned_operations);
                job.planned_bytes = Some(s.planned_bytes);
            }
            job.snapshot = s;
        }
        task
    }

    /// End of a followed scheduled run: says how it ended (from the final
    /// snapshot when there is one), lifts this window's stop request and
    /// reads durable history back.
    fn agent_ended(&mut self, last: Option<JobSnapshot>) -> Task<Message> {
        let Some(job) = self.agent_job.take() else {
            return Task::none();
        };
        let name = job.name.clone();
        let last = last.filter(|s| !s.active && s.profile.as_ref() == Some(&job.profile));
        match last {
            Some(s) if s.phase == JobPhase::Finished && s.operation_errors == 0 => self.toast(
                ToastKind::Success,
                "Synchronisation planifiée terminée",
                format!("{name} : résultat détaillé dans l’historique."),
            ),
            Some(s) if s.phase == JobPhase::Finished => self.toast(
                ToastKind::Warning,
                "Synchronisation planifiée terminée avec des erreurs",
                format!(
                    "{name} : {} refus ou échecs, détaillés dans l’historique.",
                    model::count(s.operation_errors)
                ),
            ),
            Some(_) if job.cancel_requested => self.toast(
                ToastKind::Warning,
                "Synchronisation planifiée annulée",
                format!("{name} : l’état partiel est conservé et journalisé."),
            ),
            Some(_) => self.toast(
                ToastKind::Error,
                "Synchronisation planifiée en échec",
                format!("{name} : cause dans l’historique et la page Planification."),
            ),
            None => self.toast(
                ToastKind::Warning,
                "Suivi interrompu",
                format!(
                    "{name} : l’agent ne donne plus de nouvelles. Vérifiez la page Planification."
                ),
            ),
        }
        let mut tasks = vec![self.refresh()];
        if job.cancel_requested
            && let Some(app) = &self.backend
        {
            tasks.push(Task::perform(
                backend::call(app, Application::agent_cancel_clear),
                Message::AgentResumed,
            ));
        }
        Task::batch(tasks)
    }

    /// Leaves a stop request for the followed scheduled run.
    pub(crate) fn cancel_agent(&mut self) -> Task<Message> {
        self.boot_control_error = None;
        let Some(job) = &mut self.agent_job else {
            return Task::none();
        };
        job.cancel_requested = true;
        let Some(app) = &self.backend else {
            let Some(state) = self.state_dir.clone() else {
                return Task::none();
            };
            self.boot_cancel_requested = true;
            return Task::perform(
                backend::blocking(move || {
                    sync_orchestrator::request_agent_cancel(&state)
                        .map_err(|e| Failure::new("unavailable", e.to_string()))
                }),
                Message::AgentCancelSent,
            );
        };
        Task::perform(
            backend::call(app, Application::agent_cancel_request),
            Message::AgentCancelSent,
        )
    }

    /// After a job: read durable state back (a pending close waits for it).
    pub(crate) fn finish_job_tasks(&mut self) -> Task<Message> {
        self.refresh()
    }

    pub(crate) fn start_job(&mut self, kind: JobKind, id: ProfileId, planned: Option<(u64, u64)>) {
        let name = self
            .profile(&id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        self.job = Some(Job {
            kind,
            profile: id,
            name,
            started: self.now,
            snapshot: JobSnapshot::default(),
            // Origin of the rates: nothing is transferred or scanned yet.
            samples: VecDeque::from([Sample {
                at: self.now,
                bytes: 0,
                operations: 0,
                scanned: 0,
                quick: 0,
            }]),
            cancel_requested: false,
            agent: false,
            planned: planned.map(|p| p.0),
            planned_bytes: planned.map(|p| p.1),
        });
        self.last_poll = self.now;
    }

    pub(crate) fn simulate(&mut self, id: ProfileId, ceiling: DeletionCeiling) -> Task<Message> {
        let Some(app) = self.backend.clone() else {
            return Task::none();
        };
        if self.busy() {
            self.toast(
                ToastKind::Info,
                "Patientez",
                "Une tâche est déjà en cours dans cette fenêtre.",
            );
            return Task::none();
        }
        self.preview = None;
        self.selected = Some(id.clone());
        self.go(Nav::Profiles);
        self.start_job(JobKind::Preview, id.clone(), None);
        Task::perform(backend::preview(&app, id.clone(), ceiling), move |r| {
            Message::Previewed(id.clone(), r)
        })
    }

    pub(crate) fn load_plan_page(&mut self, offset: usize) -> Task<Message> {
        let (Some(app), Some(p)) = (&self.backend, &mut self.preview) else {
            return Task::none();
        };
        self.plan_generation += 1;
        p.loading = true;
        let action = p.filter.map(model::action_key).unwrap_or("");
        let generation = self.plan_generation;
        Task::perform(
            backend::plan_page(app, p.view.plan_id.clone(), offset, p.query.clone(), action),
            move |r| Message::PlanPage(generation, r),
        )
    }

    pub(crate) fn execute(&mut self) -> Task<Message> {
        let Some(Dialog::Execute {
            profile,
            confirm_delete,
            confirm_permanent,
        }) = self.dialog.clone()
        else {
            return Task::none();
        };
        let (Some(app), Some(preview)) = (self.backend.clone(), self.preview.take()) else {
            return Task::none();
        };
        let summary = preview.view.summary;
        if (summary.delete > 0 && !confirm_delete) || (summary.trash.overflow && !confirm_permanent)
        {
            self.preview = Some(preview);
            return Task::none();
        }
        self.close_dialog();
        self.start_job(
            JobKind::Execute,
            profile.clone(),
            Some((summary.mutations(), summary.bytes)),
        );
        self.last_run = None;
        Task::perform(
            backend::execute(
                &app,
                profile,
                preview.view.plan_id,
                confirm_delete && summary.delete > 0,
                confirm_permanent && summary.trash.overflow,
            ),
            Message::Executed,
        )
    }

    pub(crate) fn release(&mut self) -> Task<Message> {
        let Some(drag) = self.drag.take() else {
            return Task::none();
        };
        let zone = self.drop_zone.take();
        let new = std::mem::take(&mut self.drop_new);
        if !drag.active {
            return self.update(Message::Select(Some(drag.profile)));
        }
        if new {
            return self.update(Message::NewGroup(Some(drag.profile)));
        }
        match zone {
            Some(group) => self.move_to(vec![drag.profile], &group),
            None => Task::none(),
        }
    }

    pub(crate) fn move_to(&mut self, ids: Vec<ProfileId>, group: &str) -> Task<Message> {
        let Some(app) = self.backend.clone() else {
            return Task::none();
        };
        if self.busy() {
            self.toast(
                ToastKind::Info,
                "Patientez",
                "Les profils ne peuvent pas être modifiés pendant une tâche.",
            );
            return Task::none();
        }
        let moves: Vec<(Profile, Option<ScheduleView>)> = ids
            .iter()
            .filter_map(|id| {
                let p = self.profile(id)?;
                let moved = draft::regrouped(p, group)?;
                let schedule = self
                    .schedule_of(id)
                    .filter(|s| s.schedule.approved == *p)
                    .cloned();
                Some((moved, schedule))
            })
            .collect();
        if moves.is_empty() {
            return Task::none();
        }
        // Optimistic display: the refresh after saving restores the truth.
        // A group emptied by the move stays visible (presentation only).
        for (moved, _) in &moves {
            if let Some(p) = self.ws.profiles.iter_mut().find(|p| p.id == moved.id) {
                let old = std::mem::replace(&mut p.group, moved.group.clone());
                if !old.is_empty()
                    && !self.ws.profiles.iter().any(|p| p.group == old)
                    && !self.prefs.groups.contains(&old)
                {
                    self.prefs.groups.push(old);
                }
            }
        }
        self.save_prefs();
        Task::perform(backend::regroup(&app, moves), Message::Moved)
    }

    pub(crate) fn submit_group(&mut self) -> Task<Message> {
        let Some(Dialog::Group {
            original,
            name,
            pending,
            ..
        }) = self.dialog.clone()
        else {
            return Task::none();
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            if let Some(Dialog::Group { error, .. }) = &mut self.dialog {
                *error = Some("Donnez un nom au groupe".into());
            }
            return Task::none();
        }
        self.close_dialog();
        match original {
            Some(old) if old == name => Task::none(),
            Some(old) => {
                let ids: Vec<ProfileId> = self
                    .ws
                    .profiles
                    .iter()
                    .filter(|p| p.group == old)
                    .map(|p| p.id.clone())
                    .collect();
                if let Some(i) = self.prefs.collapsed.iter().position(|c| *c == old) {
                    self.prefs.collapsed[i].clone_from(&name);
                }
                self.prefs.groups.retain(|g| *g != old);
                if ids.is_empty() {
                    self.prefs.groups.push(name);
                    self.save_prefs();
                    return Task::none();
                }
                self.save_prefs();
                self.move_to(ids, &name)
            }
            None => match pending {
                Some(id) => self.move_to(vec![id], &name),
                None => {
                    if !self.groups().contains(&name) {
                        self.prefs.groups.push(name.clone());
                        self.save_prefs();
                    }
                    self.toast(
                        ToastKind::Info,
                        "Groupe créé",
                        format!("Glissez des profils dans « {name} »."),
                    );
                    Task::none()
                }
            },
        }
    }

    pub(crate) fn submit_schedule(&mut self) -> Task<Message> {
        let Some(app) = self.backend.clone() else {
            return Task::none();
        };
        let Some(Dialog::Schedule {
            targets,
            draft,
            error,
            saving,
            ..
        }) = &mut self.dialog
        else {
            return Task::none();
        };
        let trigger = match draft.trigger() {
            Ok(t) => t,
            Err(e) => {
                *error = Some(e);
                return Task::none();
            }
        };
        if trigger.is_some() && !draft.approve_writes {
            *error = Some("Cochez l’autorisation des écritures automatiques".into());
            return Task::none();
        }
        *saving = true;
        let jobs: Vec<(ProfileId, String, bool, bool)> = targets
            .iter()
            .filter_map(|id| {
                let p = self.ws.profiles.iter().find(|p| &p.id == id)?;
                let had = self
                    .ws
                    .schedules
                    .iter()
                    .any(|s| &s.schedule.approved.id == id);
                Some((id.clone(), p.name.clone(), p.allow_delete, had))
            })
            .collect();
        let (missed, writes, deletions, window) = (
            draft.missed(),
            draft.approve_writes,
            draft.approve_deletions,
            draft.show_window,
        );
        let task = backend::blocking(move || {
            let mut done = Vec::new();
            if trigger.is_some() {
                crate::native::prepare_agent(app.state_directory())?;
            }
            for (id, name, allow_delete, had) in jobs {
                let result = match &trigger {
                    Some(t) => app.schedule_save(
                        id,
                        t.clone(),
                        missed,
                        writes,
                        deletions && allow_delete,
                        window,
                    ),
                    None if had => crate::native::remove_schedule(&app, id),
                    None => Ok(()),
                };
                if let Err(e) = result {
                    return Ok(Err((
                        done,
                        Failure::new(&e.code, format!("« {name} » : {}", e.message)),
                    )));
                }
                done.push(name);
            }
            Ok(Ok(done))
        });
        Task::perform(task, |r: Result<backend::Batch, Failure>| {
            Message::ScheduleSaved(r.unwrap_or_else(|e| Err((Vec::new(), e))))
        })
    }

    /// Reloads the shown simulation list (folders or flat) from its start.
    pub(crate) fn reload_plan(&mut self) -> Task<Message> {
        match self.list_mode {
            ListMode::Flat => self.load_plan_page(0),
            ListMode::Tree => match &mut self.preview {
                Some(p) => {
                    let request = p.tree.reset();
                    self.load_plan_tree(request)
                }
                None => Task::none(),
            },
        }
    }

    pub(crate) fn load_plan_tree(&self, request: tree::Request) -> Task<Message> {
        let (Some(app), Some(p)) = (&self.backend, &self.preview) else {
            return Task::none();
        };
        let action = p.filter.map(model::action_key).unwrap_or("");
        let task = backend::plan_tree(
            app,
            p.view.plan_id.clone(),
            &request,
            p.query.clone(),
            action,
        );
        Task::perform(task, move |r| Message::PlanTreeLevel(request.clone(), r))
    }

    /// Reloads the expanded run (folders or flat page) from its start.
    pub(crate) fn reload_history(&mut self, execution: String) -> Task<Message> {
        match self.list_mode {
            ListMode::Flat => self.load_history(execution, 0),
            ListMode::Tree => {
                let request = self.history_tree.reset();
                self.load_history_tree(execution, request)
            }
        }
    }

    pub(crate) fn load_history_tree(
        &self,
        execution: String,
        request: tree::Request,
    ) -> Task<Message> {
        let (Some(app), Some(id)) = (&self.backend, self.selected.clone()) else {
            return Task::none();
        };
        let task = backend::history_tree(
            app,
            id,
            execution.clone(),
            &request,
            self.history_filter,
            self.history_query.clone(),
        );
        Task::perform(task, move |r| {
            Message::HistoryTreeLevel(execution.clone(), request.clone(), r)
        })
    }

    pub(crate) fn load_history(&mut self, execution: String, offset: usize) -> Task<Message> {
        self.history_generation = self.history_generation.wrapping_add(1);
        let (Some(app), Some(id)) = (&self.backend, self.selected.clone()) else {
            return Task::none();
        };
        Task::perform(
            backend::history_page(
                app,
                id,
                execution,
                offset,
                self.history_filter,
                self.history_query.clone(),
                self.history_generation,
            ),
            Message::HistoryPage,
        )
    }

    pub(crate) fn wizard_update(&mut self, msg: WizardMsg) -> Task<Message> {
        let now = self.now;
        let Some(w) = &mut self.wizard else {
            return Task::none();
        };
        match w.update(msg, now) {
            crate::wizard::Outcome::None => Task::none(),
            crate::wizard::Outcome::Task(t) => t,
            crate::wizard::Outcome::Save {
                profile,
                schedule,
                simulate,
            } => {
                let Some(app) = &self.backend else {
                    return Task::none();
                };
                if self.job.is_some() {
                    w.saving = false;
                    w.error = Some(
                        "Une tâche est en cours : enregistrez quand elle est terminée.".into(),
                    );
                    return Task::none();
                }
                Task::perform(backend::save(app, *profile, schedule), move |r| {
                    Message::Saved(r, simulate)
                })
            }
        }
    }

    pub(crate) fn key(
        &mut self,
        key: Key,
        digit: Option<usize>,
        m: Modifiers,
        captured: bool,
    ) -> Task<Message> {
        if let Key::Named(Named::Escape) = key {
            if self.purged {
                return self.quit();
            }
            if self.menu.is_some() {
                self.menu = None;
            } else if self.dialog.is_some() {
                self.close_dialog();
            } else if let Some(w) = &mut self.wizard {
                w.close(self.now);
            } else if self.drag.is_some() {
                self.drag = None;
            } else if !self.search.is_empty() {
                self.search.clear();
            }
            return Task::none();
        }
        if self.wizard.is_some() || self.dialog.is_some() {
            if !captured
                && let (Key::Named(Named::Enter), Some(Dialog::Group { .. })) = (&key, &self.dialog)
            {
                return self.submit_group();
            }
            return Task::none();
        }
        let ctrl = m.command();
        if let (true, Some(i)) = (ctrl, digit) {
            return self.update(Message::Nav(Nav::ALL[i.min(Nav::ALL.len() - 1)]));
        }
        match key.as_ref() {
            Key::Character(c) if ctrl => match c.to_lowercase().as_str() {
                "n" => self.update(Message::New(None)),
                "f" => iced::widget::operation::focus(SEARCH_ID),
                "e" => match self.selected.clone() {
                    Some(id) => self.update(Message::Edit(id)),
                    None => Task::none(),
                },
                "r" => self.refresh(),
                "q" => self.update(Message::Quit),
                "t" => {
                    let next = if self.wants_light() {
                        ThemeChoice::Dark
                    } else {
                        ThemeChoice::Light
                    };
                    self.update(Message::Theme(next))
                }
                "," => self.update(Message::Nav(Nav::Settings)),
                "/" => self.update(Message::ShowShortcuts),
                _ => Task::none(),
            },
            _ if captured => Task::none(),
            Key::Named(Named::F1) => self.update(Message::ShowShortcuts),
            Key::Named(Named::F5) => match self.selected.clone() {
                Some(id) => self.update(Message::Simulate(id)),
                None => Task::none(),
            },
            Key::Named(Named::Delete) => match self.selected.clone() {
                Some(id) if !self.busy() => self.update(Message::AskRemove(id)),
                _ => Task::none(),
            },
            Key::Named(Named::ArrowDown | Named::ArrowUp) => {
                let order: Vec<ProfileId> = self
                    .groups()
                    .iter()
                    .filter(|g| !self.prefs.collapsed.contains(g))
                    .flat_map(|g| {
                        self.members(g)
                            .into_iter()
                            .map(|p| p.id.clone())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                if order.is_empty() {
                    return Task::none();
                }
                let pos = self
                    .selected
                    .as_ref()
                    .and_then(|s| order.iter().position(|id| id == s));
                let next = match (pos, key == Key::Named(Named::ArrowDown)) {
                    (None, _) => 0,
                    (Some(i), true) => (i + 1).min(order.len() - 1),
                    (Some(i), false) => i.saturating_sub(1),
                };
                self.update(Message::Select(Some(order[next].clone())))
            }
            Key::Named(Named::Enter) if self.preview.is_some() && self.selected.is_some() => {
                self.update(Message::AskExecute)
            }
            _ => Task::none(),
        }
    }
}
