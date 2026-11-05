//! Window layout: navigation rail, profile sidebar (groups, drag and drop),
//! content area and the overlay layers (assistant, dialogs, menus, toasts).

use crate::app::{App, JobKind, Message, Nav};
use crate::charts::{self, Mark};
use crate::icons::{self, Glyph};
use crate::model::{self, Outcome};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, pin, row, scrollable, stack};
use iced::{Alignment, Color, Length, Padding};
use sync_plan::SyncMode;

pub(crate) fn mode_glyph(mode: SyncMode) -> Glyph {
    match mode {
        SyncMode::Backup => Glyph::Backup,
        SyncMode::Mirror => Glyph::Mirror,
        _ => Glyph::TwoWay,
    }
}

/// Text-safe mode color (labels, icons on a surface).
pub(crate) fn mode_color(p: &Palette, mode: SyncMode) -> Color {
    match mode {
        SyncMode::Backup => p.backup_ink,
        SyncMode::Mirror => p.mirror_ink,
        _ => p.info,
    }
}

/// Vivid mode color for fills (tiles, bars, dots).
pub(crate) fn mode_fill(p: &Palette, mode: SyncMode) -> Color {
    match mode {
        SyncMode::Backup => p.backup,
        SyncMode::Mirror => p.mirror,
        _ => p.violet,
    }
}

pub(crate) fn outcome_color(p: &Palette, o: Outcome) -> Color {
    match o {
        Outcome::Success => p.success,
        Outcome::Warning => p.warning,
        Outcome::Failure => p.danger,
        Outcome::Interrupted => p.danger,
    }
}

impl App {
    /// Agent-published queue; no schedule decisions are reconstructed in the UI.
    pub(crate) fn agent_queue_panel(&self, p: &Palette) -> Option<El<'_, Message>> {
        let queue = self.agent_queue.as_ref()?;
        let job = self.agent_job.as_ref()?;
        let current = queue
            .current
            .as_ref()
            .filter(|current| current.id == job.profile)?;
        if !self.ws.resident.available {
            return None;
        }
        let mut rows = column![
            row![
                icons::spinning(
                    Glyph::Sync,
                    20.0,
                    p.accent,
                    self.now
                        .saturating_duration_since(self.started)
                        .as_secs_f32()
                        * 0.6
                ),
                container(ui::strong(p.text, current.name.clone(), 15.0)).width(Length::Fill),
                ui::pill(p.accent, "En cours"),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(12);
        for (index, profile) in queue.pending.iter().enumerate() {
            rows = rows.push(
                row![
                    ui::colored(p.muted, format!("{}", index + 1), 13.0),
                    container(ui::muted(p, profile.name.clone())).width(Length::Fill),
                    ui::caption(p, "En attente"),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        }
        if queue.pending.is_empty() {
            rows = rows.push(ui::caption(p, "Aucun autre profil en attente."));
        }
        Some(container(column![
            ui::strong(p.text, format!("File d’attente · {} profil(s)", queue.pending.len() + 1), 16.0),
            container(scrollable(rows).height(Length::Shrink).style(theme::scroll(*p))).max_height(220.0),
            ui::caption(p, "Un seul profil est exécuté à la fois. Les suivants démarrent après sa fin ; toute modification reste revérifiée avant exécution."),
        ].spacing(14)).padding(20).width(Length::Fill).style(theme::card(*p, 0.0, false)).into())
    }
    pub(crate) fn view(&self) -> El<'_, Message> {
        let p = self.palette();
        let body = match &self.backend {
            Some(_) => self.workspace(&p),
            None => self.splash(&p),
        };
        self.framed(&p, body)
    }

    /// Everything below the title bar once the local state is open.
    fn workspace(&self, p: &Palette) -> El<'_, Message> {
        let p = *p;
        let compact = self.ws.preferences.compact;
        let content = self.content(&p);
        let base = container(row![self.rail(&p), content])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::panel(p.bg));
        let mut layers: Vec<El<'_, Message>> = vec![base.into()];
        if let Some(w) = &self.wizard {
            layers.push(w.view(&p, self.now, &self.ws.providers, compact));
        }
        if let Some(d) = self.dialog_layer(&p) {
            layers.push(d);
        }
        if let Some(m) = self.menu_layer(&p) {
            layers.push(m);
        }
        if let Some(g) = self.ghost(&p) {
            layers.push(g);
        }
        if let Some(t) = self.toasts_layer(&p) {
            layers.push(t);
        }
        stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn splash(&self, p: &Palette) -> El<'_, Message> {
        let turn = self
            .now
            .saturating_duration_since(self.started)
            .as_secs_f32()
            * 0.6;
        if let Some(wait) = &self.boot_wait {
            let live = wait.live.as_ref().filter(|live| {
                wait.resident.available
                    && live.is_live(model::now_secs(), crate::app::AGENT_STALE_SECS)
            });
            let large = self.mini_large;
            let mut body = column![
                row![
                    charts::mark(Mark { from: p.backup, to: p.violet, turn }, if large { 40.0 } else { 30.0 }),
                    ui::title(p, "SyncEngine", if large { 22.0 } else { 18.0 }),
                    ui::fill(),
                    ui::status(
                        p,
                        if wait.resident.available { p.backup } else { p.faint },
                        if wait.resident.available { "Agent actif" } else { "Agent arrêté" },
                    ),
                    ui::action(
                        p,
                        Tone::Secondary,
                        Some(if large { Glyph::ChevronUp } else { Glyph::ChevronDown }),
                        if large { "Réduire" } else { "Agrandir" },
                        Some(Message::MiniToggle),
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                ui::caption(p, "Synchronisation planifiée en cours. L’interface complète s’ouvrira à la fin ; pause et annulation restent disponibles."),
            ].spacing(16);
            if live.is_some()
                && let Some(job) = &self.agent_job
            {
                body = body.push(self.live_panel(p, job, large));
            } else {
                body = body.push(ui::muted(
                    p,
                    "En attente de l'opération en cours — aucune progression récente disponible.",
                ));
                if wait.resident.available {
                    body = body.push(ui::action(
                        p,
                        Tone::Secondary,
                        Some(Glyph::Power),
                        "Arrêter l’agent",
                        (!self.ws.agent_stopped).then_some(Message::StopAgent),
                    ));
                }
            }
            if let Some(error) = &self.boot_control_error {
                body = body.push(ui::callout(
                    p,
                    p.danger,
                    Glyph::Alert,
                    "Commande non appliquée",
                    error.to_string(),
                ));
            }
            if let Some(queue) = self.agent_queue_panel(p) {
                body = body.push(queue);
            }
            if live.is_some() && wait.resident.available {
                body = body.push(ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Power),
                    "Arrêter l’agent",
                    (!self.ws.agent_stopped).then_some(Message::StopAgent),
                ));
            }
            return scrollable(
                container(container(body).max_width(if large { 1040.0 } else { 420.0 }))
                    .padding(if large { 24 } else { 16 })
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            )
            .style(theme::scroll(*p))
            .height(Length::Fill)
            .into();
        }
        let body: El<'_, Message> = match &self.boot_error {
            None => column![
                charts::mark(Mark { from: p.accent, to: theme::mix(p.accent, p.info, 0.6), turn }, 84.0),
                ui::title(p, "SyncEngine", 28.0),
                ui::muted(p, "Ouverture de l’état local…"),
            ]
            .spacing(14)
            .align_x(Alignment::Center)
            .into(),
            Some(e) => column![
                charts::mark(Mark { from: p.danger, to: theme::mix(p.danger, p.warning, 0.5), turn: 0.0 }, 72.0),
                ui::title(p, e.title(), 22.0),
                container(ui::muted(p, e.message.clone())).max_width(560.0),
                ui::caption(p, "Aucune donnée n’a été modifiée. Vérifiez qu’une autre fenêtre SyncEngine n’est pas déjà ouverte."),
                ui::action(p, Tone::Primary, Some(Glyph::Refresh), "Réessayer", Some(Message::Retry)),
            ]
            .spacing(12)
            .align_x(Alignment::Center)
            .into(),
        };
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .center(Length::Fill)
            .style(theme::panel(p.bg))
            .into()
    }

    /// Earliest upcoming timed schedule (presentation only: the agent decides).
    pub(crate) fn next_job(&self) -> Option<(i64, String)> {
        let now = model::now_secs();
        self.ws
            .schedules
            .iter()
            .filter(|s| self.schedule_valid(&s.schedule.approved.id) == Some(true))
            .filter_map(|s| {
                crate::draft::next_occurrence(&s.schedule.trigger, s.schedule.created_at, now)
                    .map(|at| (at, s.schedule.approved.id.clone()))
            })
            .min_by_key(|(at, _)| *at)
            .map(|(at, id)| {
                let name = self
                    .profile(&id)
                    .map_or_else(|| id.to_string(), |pr| pr.name.clone());
                (at, name)
            })
    }

    /// « HH:MM » (UTC, like every schedule time of the application).
    pub(crate) fn clock_label(secs: i64) -> String {
        let (_, _, _, hour, minute) = model::utc(secs);
        format!("{hour:02}:{minute:02}")
    }

    /// Labeled navigation column: brand, sections with counters, live job and
    /// the agent card.
    fn rail(&self, p: &Palette) -> El<'_, Message> {
        let invalid = self
            .ws
            .schedules
            .iter()
            .filter(|s| {
                self.schedule_valid(&s.schedule.approved.id) != Some(true)
                    || s.running_slot.is_some()
            })
            .count();
        let live = usize::from(self.job.is_some()) + usize::from(self.agent_job.is_some());
        let items = [
            (
                Nav::Profiles,
                "Profils",
                (!self.ws.profiles.is_empty()).then(|| self.ws.profiles.len().to_string()),
            ),
            (
                Nav::Activity,
                "Activité",
                (live > 0).then(|| live.to_string()),
            ),
            (Nav::History, "Historique", None),
            (
                Nav::Schedules,
                "Planification",
                (invalid > 0).then(|| invalid.to_string()),
            ),
            (Nav::Trash, "Corbeille", None),
            (Nav::Settings, "Réglages", None),
        ];
        let nav_item = |nav: Nav, label: &'static str, count: Option<String>| {
            let active = self.nav == nav;
            let fg = if active { p.bg } else { p.muted };
            let mut line = row![ui::strong(fg, label, 14.0), ui::fill()]
                .spacing(12)
                .align_y(Alignment::Center);
            if let Some(c) = count {
                line = line.push(ui::mono(fg, c, 11.0));
            }
            button(line)
                .padding(Padding::from([9, 14]))
                .width(Length::Fill)
                .style(theme::nav_btn(*p, active))
                .on_press(Message::Nav(nav))
        };
        let mut col = column![
            row![
                charts::mark(
                    Mark {
                        from: p.backup,
                        to: p.violet,
                        turn: 0.0
                    },
                    24.0
                ),
                ui::title(p, "SyncEngine", 18.0)
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .padding(Padding {
                top: 2.0,
                bottom: 18.0,
                left: 10.0,
                right: 10.0
            }),
        ]
        .spacing(4);
        for (nav, label, count) in items {
            col = col.push(nav_item(nav, label, count));
        }
        let light = p.is_light();
        let agent_on = !self.ws.agent_stopped;
        let detail = match (agent_on, self.next_job()) {
            (false, _) => "Les planifications ne se lancent pas".to_owned(),
            (true, Some((at, _))) => format!("Prochain job à {} UTC", Self::clock_label(at)),
            (true, None) => match self.ws.schedules.len() {
                0 => "Aucune planification".to_owned(),
                1 => "1 planification".to_owned(),
                n => format!("{n} planifications"),
            },
        };
        let agent = container(
            row![
                column![
                    row![
                        ui::dot(if agent_on { p.backup } else { p.faint }, 8.0),
                        ui::strong(
                            p.text,
                            if agent_on {
                                "Agent actif"
                            } else {
                                "Agent arrêté"
                            },
                            13.0
                        )
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    ui::caption(p, detail),
                ]
                .spacing(2)
                .width(Length::Fill),
                ui::icon_button(
                    p,
                    if light { Glyph::Moon } else { Glyph::Sun },
                    if light {
                        "Thème sombre  ·  Ctrl+T"
                    } else {
                        "Thème clair  ·  Ctrl+T"
                    },
                    Some(Message::Theme(if light {
                        crate::ui_prefs::ThemeChoice::Dark
                    } else {
                        crate::ui_prefs::ThemeChoice::Light
                    }))
                ),
            ]
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10, 12]))
        .width(Length::Fill)
        .style(theme::outline(*p, p.border, 0.0));
        col = col
            .push(Space::new().height(Length::Fill))
            .push(self.job_card(p))
            .push(agent);
        row![
            container(col.height(Length::Fill))
                .width(Length::Fixed(232.0))
                .height(Length::Fill)
                .padding(Padding::from([18, 14]))
                .style(theme::panel(p.rail)),
            container(Space::new())
                .width(Length::Fixed(1.0))
                .height(Length::Fill)
                .style(theme::panel(p.border)),
        ]
        .into()
    }

    /// Profiles waiting behind the live job: the agent's queue for a
    /// scheduled run, the chain of a group run otherwise.
    fn waiting_names(&self, job: &crate::app::Job) -> Vec<String> {
        if job.agent {
            self.agent_queue.as_ref().map_or_else(Vec::new, |queue| {
                queue.pending.iter().map(|q| q.name.clone()).collect()
            })
        } else {
            self.group_waiting()
        }
    }

    /// Tracker at the bottom of the sidebar: label and percentage, curve of
    /// the throughput, progress bar with pace and time left, controls, and
    /// the profiles waiting behind (at most three shown).
    fn job_card(&self, p: &Palette) -> El<'_, Message> {
        const SHOWN: usize = 3;
        let Some(job) = self.live_job() else {
            return Space::new().into();
        };
        let (label, color) = match (job.agent, job.kind) {
            (true, JobKind::Preview) => ("Planifiée : analyse", p.accent),
            (true, JobKind::Execute) => ("Planifiée : écritures", p.success),
            (false, JobKind::Preview) => ("Simulation en cours", p.accent),
            (false, JobKind::Execute) => ("Exécution réelle", p.success),
        };
        let s = &job.snapshot;
        let rates = self.job_rates(job);
        let progress = job.progress();
        let total = rates.forward.last().copied().unwrap_or(0.0)
            + rates
                .reverse
                .as_ref()
                .and_then(|r| r.last())
                .copied()
                .unwrap_or(0.0);
        let left = job
            .remaining_label(self.now)
            .map(|l| l.trim_start_matches("reste ").to_owned());
        let detail = match job.kind {
            JobKind::Preview if s.hash_target > 0 => format!(
                "{} / {} empreintes",
                model::count(s.entries_hashed),
                model::count(s.hash_target)
            ),
            JobKind::Preview => format!("{} entrées analysées", model::count(s.entries_scanned)),
            JobKind::Execute => {
                let (value, unit) = crate::profile::rate_parts(total);
                match left {
                    Some(left) => format!("{value} {unit} · {left}"),
                    None => format!("{value} {unit}"),
                }
            }
        };
        let curve = charts::spark(
            rates.forward.clone(),
            color,
            rates.reverse.clone().map(|r| (r, p.violet)),
            Color::TRANSPARENT,
            30.0,
        );
        let mut card = column![
            row![
                ui::strong(color, label, 12.0),
                ui::fill(),
                ui::mono(
                    p.text,
                    progress.map_or_else(|| "…".to_owned(), model::percent),
                    12.0
                ),
            ]
            .align_y(Alignment::Center),
            ui::caption(p, model::ellipsize(&job.name, 28)),
            curve,
            ui::progress(p, progress.unwrap_or(0.0), color, 4.0),
            row![
                container(ui::caption(p, detail)).width(Length::Fill),
                crate::profile::pause_icon(p, job),
                ui::icon_button(
                    p,
                    Glyph::Stop,
                    if job.cancel_requested {
                        "Annulation demandée…"
                    } else {
                        "Annuler proprement"
                    },
                    (!job.cancel_requested).then_some(crate::profile::cancel_message(job))
                ),
            ]
            .spacing(2)
            .align_y(Alignment::Center),
        ]
        .spacing(6);
        let waiting = self.waiting_names(job);
        if !waiting.is_empty() {
            let mut list = column![ui::eyebrow(p.muted, "Ensuite")].spacing(3);
            for name in waiting.iter().take(SHOWN) {
                list = list.push(ui::colored(p.text, model::ellipsize(name, 30), 12.0));
            }
            if waiting.len() > SHOWN {
                list = list.push(ui::caption(
                    p,
                    format!("+ {} autre(s) en attente", waiting.len() - SHOWN),
                ));
            }
            card = card.push(list);
        }
        container(card)
            .padding(Padding::from([10, 12]))
            .width(Length::Fill)
            .style(theme::panel(theme::mix(p.sidebar, p.rail, 0.6)))
            .into()
    }

    fn content(&self, p: &Palette) -> El<'_, Message> {
        let t = self.page_t.at(self.now);
        let faded = p.faded(t);
        let page: El<'_, Message> = match self.nav {
            Nav::Profiles => match self.selected.as_ref().and_then(|id| self.profile(id)) {
                Some(profile) => self.profile_page(&faded, profile),
                None => self.overview(&faded),
            },
            Nav::Activity => self.activity_page(&faded),
            Nav::History => self.history_page(&faded),
            Nav::Schedules => self.schedules_page(&faded),
            Nav::Trash => self.trash_page(&faded),
            Nav::Settings => self.settings_page(&faded),
        };
        container(page)
            .padding(Padding {
                top: 14.0 * (1.0 - t),
                ..Padding::ZERO
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn ghost(&self, p: &Palette) -> Option<El<'_, Message>> {
        let drag = self.drag.as_ref().filter(|d| d.active)?;
        let profile = self.profile(&drag.profile)?;
        let target = if self.drop_new {
            "Nouveau groupe".to_owned()
        } else {
            match self.drop_zone.as_deref() {
                Some("") => "Sans groupe".to_owned(),
                Some(g) if g != profile.group => g.to_owned(),
                _ => String::new(),
            }
        };
        let mut content = row![
            icons::icon(mode_glyph(profile.mode), 16.0, mode_color(p, profile.mode)),
            ui::strong(p.text, model::ellipsize(&profile.name, 28), 14.0),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        if !target.is_empty() {
            content = content
                .push(icons::icon(Glyph::ArrowRight, 14.0, p.accent))
                .push(ui::colored(p.accent, target, 13.0));
        }
        let card = container(content)
            .padding(Padding::from([9, 14]))
            .style(theme::card(*p, 1.0, true));
        Some(
            pin(card)
                .x(self.cursor.x + 14.0)
                .y(self.cursor.y + 10.0)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        )
    }
}
