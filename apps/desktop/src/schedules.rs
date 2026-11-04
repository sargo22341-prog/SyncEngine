//! Planning: shared trigger editor (wizard step and dialog) and the
//! « Planification » page listing every authorization held by the agent.

use crate::app::{App, Message};
use crate::draft::{self, ScheduleDraft, TriggerKind};
use crate::icons::{self, Glyph};
use crate::model;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{Space, button, checkbox, column, container, row, scrollable, text_input};
use iced::{Alignment, Length, Padding};

/// Edits of a schedule draft.
#[derive(Debug, Clone)]
pub(crate) enum ScheduleMsg {
    Kind(TriggerKind),
    Interval(String),
    IntervalPreset(u32),
    Hour(i32),
    Minute(i32),
    Delay(String),
    Skip(bool),
    Writes(bool),
    Deletions(bool),
    Window(bool),
}

/// Explanations behind the « i » of each option, for non-specialists.
const SKIP_INFO: &str = "Si l’ordinateur est éteint, en veille ou si votre session Windows est fermée à l’heure prévue, la synchronisation de cette heure-là est manquée.

Case cochée : elle est simplement oubliée. Rien ne se lance au rallumage ; on attend la prochaine heure prévue.

Case décochée (conseillé pour une sauvegarde) : une seule synchronisation de rattrapage se lance dès que l’ordinateur est rallumé et la session ouverte, même si plusieurs heures ont été manquées.";
const WRITES_INFO: &str = "Obligatoire pour planifier. À l’heure prévue, SyncEngine compare les deux dossiers puis copie et remplace aussitôt les fichiers nécessaires, sans vous demander de confirmer, même fenêtre fermée.

C’est la vraie synchronisation, pas une simulation. Avant de remplacer un fichier, SyncEngine garde une copie de l’ancienne version, et chaque opération est notée dans l’historique.

Cette case seule n’autorise aucune suppression.";
const DELETIONS_INFO: &str = "Autorise aussi la synchronisation planifiée à effacer les fichiers qui ont disparu de l’autre côté (modes miroir et bidirectionnel).

Les garde-fous restent actifs : aucune suppression si la source est vide ou illisible, si l’analyse est incomplète, ou si trop de fichiers devraient disparaître d’un coup (plus de la moitié de la destination, à partir de 10 suppressions).

Sans cette case, une synchronisation qui devrait supprimer des fichiers ne fait rien du tout et s’affiche en échec dans la page Planification : vous décidez vous-même.";
const WINDOW_INFO: &str = "À chaque synchronisation planifiée, SyncEngine ouvre sa fenêtre sur la page Activité : vous voyez en direct le pourcentage, le débit et les éventuelles erreurs, et vous pouvez l’annuler.

Si la fenêtre est déjà ouverte ou réduite dans la zone de notification, elle revient simplement au premier plan.

Sans cette case, la synchronisation se fait en arrière-plan, sans fenêtre ; son résultat apparaît ensuite dans l’historique. Nécessite une session Windows ouverte.";

pub(crate) fn apply(d: &mut ScheduleDraft, msg: ScheduleMsg) {
    match msg {
        ScheduleMsg::Kind(k) if k.available() => {
            if d.kind != k {
                // A new kind of trigger is a new decision: consents reset.
                d.approve_writes = false;
                d.approve_deletions = false;
            }
            d.kind = k;
        }
        ScheduleMsg::Kind(_) => {}
        ScheduleMsg::Interval(v) => {
            d.interval_minutes = v.chars().filter(char::is_ascii_digit).take(6).collect()
        }
        ScheduleMsg::IntervalPreset(m) => d.interval_minutes = m.to_string(),
        ScheduleMsg::Hour(delta) => d.hour = (i32::from(d.hour) + delta).rem_euclid(24) as u8,
        ScheduleMsg::Minute(delta) => d.minute = (i32::from(d.minute) + delta).rem_euclid(60) as u8,
        ScheduleMsg::Delay(v) => {
            d.delay_minutes = v.chars().filter(char::is_ascii_digit).take(4).collect()
        }
        ScheduleMsg::Skip(v) => d.skip_missed = v,
        ScheduleMsg::Writes(v) => {
            d.approve_writes = v;
            if !v {
                d.approve_deletions = false;
            }
        }
        ScheduleMsg::Deletions(v) => d.approve_deletions = v && d.approve_writes,
        ScheduleMsg::Window(v) => d.show_window = v && draft::window_supported(),
    }
}

/// Checkbox followed by its « i » explanation.
fn option<'a>(
    p: &Palette,
    value: bool,
    label: &'a str,
    on: Option<fn(bool) -> ScheduleMsg>,
    info: &str,
) -> El<'a, ScheduleMsg> {
    row![
        checkbox(value)
            .label(label)
            .on_toggle_maybe(on)
            .text_size(13)
            .style(theme::check(*p)),
        ui::info_tip(p, info),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

fn stepper<'a>(
    p: &Palette,
    value: String,
    minus: ScheduleMsg,
    plus: ScheduleMsg,
) -> El<'a, ScheduleMsg> {
    let small = |g: Glyph, m: ScheduleMsg| {
        button(icons::icon(g, 14.0, p.muted))
            .padding(6)
            .style(theme::btn(*p, Tone::Ghost))
            .on_press(m)
    };
    container(
        column![
            small(Glyph::ChevronUp, plus).padding(Padding::from([4, 10])),
            ui::title(p, value, 30.0),
            small(Glyph::ChevronDown, minus).padding(Padding::from([4, 10])),
        ]
        .align_x(Alignment::Center),
    )
    .padding(Padding::from([4, 10]))
    .style(theme::outline(*p, p.border, 0.0))
    .into()
}

/// Trigger editor. `deletions` tells whether a target profile may delete.
pub(crate) fn editor<'a>(
    p: &Palette,
    d: &ScheduleDraft,
    deletions: bool,
    compact: bool,
) -> El<'a, ScheduleMsg> {
    let tiles = TriggerKind::ALL.iter().filter(|k| k.available()).map(|k| {
        let selected = d.kind == *k;
        button(
            row![
                ui::glyph_box(k.glyph(), if selected { p.accent } else { p.muted }, 34.0),
                column![
                    ui::strong(p.text, k.label(), 14.0),
                    ui::caption(p, k.hint())
                ]
                .spacing(2)
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .padding(12)
        .width(Length::Fill)
        .style(theme::tile(*p, selected, true))
        .on_press(ScheduleMsg::Kind(*k))
        .into()
    });
    let grid = iced::widget::grid(tiles)
        .fluid(if compact { 250.0 } else { 320.0 })
        .spacing(10)
        .height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink));
    let mut col = column![grid].spacing(18);
    match d.kind {
        TriggerKind::Interval => {
            let presets = [(15, "15 min"), (60, "1 h"), (360, "6 h"), (1440, "24 h")]
                .into_iter()
                .map(|(m, label)| {
                    button(ui::colored(p.text, label, 13.0))
                        .padding(Padding::from([5, 12]))
                        .style(theme::chip(
                            *p,
                            p.accent,
                            d.interval_minutes == m.to_string(),
                        ))
                        .on_press(ScheduleMsg::IntervalPreset(m))
                        .into()
                });
            col = col.push(
                column![
                    ui::strong(p.muted, "Fréquence", 12.0),
                    row![
                        text_input("60", &d.interval_minutes)
                            .on_input(ScheduleMsg::Interval)
                            .padding(Padding::from([9, 12]))
                            .width(Length::Fixed(110.0))
                            .style(theme::input(*p, false)),
                        ui::muted(p, "minutes"),
                        ui::hspace(8.0),
                    ]
                    .extend(presets)
                    .spacing(8)
                    .align_y(Alignment::Center),
                    ui::caption(p, "Phase fixe à partir de l’enregistrement, indépendante de la durée d’exécution."),
                ]
                .spacing(8),
            );
        }
        TriggerKind::Daily => {
            col = col.push(
                row![
                    stepper(p, format!("{:02}", d.hour), ScheduleMsg::Hour(-1), ScheduleMsg::Hour(1)),
                    ui::title(p, ":", 30.0),
                    stepper(p, format!("{:02}", d.minute), ScheduleMsg::Minute(-5), ScheduleMsg::Minute(5)),
                    column![
                        ui::strong(p.text, "Heure universelle (UTC)", 14.0),
                        ui::caption(p, "Aucune ambiguïté au changement d’heure. Les minutes avancent par pas de 5."),
                    ]
                    .spacing(4)
                    .width(Length::Fill)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        }
        TriggerKind::LogonDelayed => {
            col = col.push(
                row![
                    text_input("2", &d.delay_minutes)
                        .on_input(ScheduleMsg::Delay)
                        .padding(Padding::from([9, 12]))
                        .width(Length::Fixed(110.0))
                        .style(theme::input(*p, false)),
                    ui::muted(p, "minutes après l’ouverture de session (1 à 1440)"),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
        }
        _ => {}
    }
    if d.kind != TriggerKind::Manual {
        if draft::window_supported() {
            col = col.push(option(
                p,
                d.show_window,
                "Afficher la fenêtre de suivi pendant la synchronisation",
                Some(ScheduleMsg::Window),
                WINDOW_INFO,
            ));
        }
        let mut consents = column![
            option(
                p,
                d.skip_missed,
                "Ignorer les échéances manquées (sinon : une seule exécution de rattrapage)",
                Some(ScheduleMsg::Skip),
                SKIP_INFO,
            ),
            option(
                p,
                d.approve_writes,
                "J’autorise les écritures automatiques, sans confirmation dans la fenêtre",
                Some(ScheduleMsg::Writes),
                WRITES_INFO,
            ),
        ]
        .spacing(10);
        if deletions {
            consents = consents.push(option(
                p,
                d.approve_deletions,
                "J’autorise aussi les suppressions protégées (seuils et garde-fous maintenus)",
                d.approve_writes.then_some(ScheduleMsg::Deletions),
                DELETIONS_INFO,
            ));
        }
        col = col.push(
            container(
                column![
                    row![
                        icons::icon(Glyph::Shield, 16.0, p.warning),
                        ui::strong(p.text, "Consentement explicite", 13.0)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                    consents
                ]
                .spacing(12),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::outline(*p, p.warning, 0.6)),
        );
        col = col.push(ui::caption(
            p,
            "L’autorisation porte sur la configuration exacte du profil. Changer le mode, la source ou la destination exige de la renouveler ; les autres modifications enregistrées depuis l’assistant la conservent. Les tâches Windows sont enregistrées automatiquement.",
        ));
    }
    col.into()
}

/// Occurrences bucketed per (day, hour) are bounded: a schedule every minute
/// still fills a cell, not memory.
const WEEK_OCCURRENCES_PER_SCHEDULE: usize = 12_000;
const HOUR_ROW: f32 = 34.0;

/// (day index, hour) → (profile name, mode, occurrences in that hour).
type WeekCells =
    std::collections::BTreeMap<(usize, usize), Vec<(String, sync_plan::SyncMode, usize)>>;

impl App {
    /// Seven days from today (UTC), one row per hour, filled from the
    /// timetable of every authorization. Sessions events have no fixed time
    /// and are listed under the grid.
    fn week_view<'a>(&'a self, p: &Palette, now: i64) -> El<'a, Message> {
        let today = now.div_euclid(86_400) * 86_400;
        let end = today + 7 * 86_400;
        // (day index, hour) → (profile name, mode, occurrences)
        let mut cells = WeekCells::new();
        let mut event_driven: Vec<String> = Vec::new();
        for s in &self.ws.schedules {
            let id = &s.schedule.approved.id;
            let name = self
                .profile(id)
                .map_or(s.schedule.approved.name.clone(), |pr| pr.name.clone());
            let mode = s.schedule.approved.mode;
            let times = s
                .schedule
                .occurrences(today, end, WEEK_OCCURRENCES_PER_SCHEDULE);
            if times.is_empty() && s.schedule.enabled {
                event_driven.push(name);
                continue;
            }
            for at in times {
                let day = ((at - today) / 86_400) as usize;
                let hour = (at.rem_euclid(86_400) / 3600) as usize;
                let entry = cells.entry((day, hour)).or_default();
                match entry.iter_mut().find(|(n, _, _)| *n == name) {
                    Some((_, _, count)) => *count += 1,
                    None => entry.push((name.clone(), mode, 1)),
                }
            }
        }
        let current_hour = (now.rem_euclid(86_400) / 3600) as usize;
        let highlight = theme::alpha(p.text, 0.08);
        let mut header = row![container(Space::new()).width(Length::Fixed(48.0))].spacing(4);
        for day in 0..7_usize {
            let (weekday, date) = model::day_label(today + day as i64 * 86_400);
            header = header.push(
                container(
                    column![
                        ui::strong(if day == 0 { p.text } else { p.muted }, weekday, 12.0),
                        ui::caption(p, date),
                    ]
                    .spacing(1)
                    .align_x(Alignment::Center),
                )
                .width(Length::FillPortion(1))
                .center_x(Length::FillPortion(1)),
            );
        }
        let mut grid = column![header].spacing(2);
        for hour in 0..24_usize {
            let mut line = row![
                container(ui::mono(p.faint, format!("{hour:02}:00"), 11.0))
                    .width(Length::Fixed(48.0))
                    .height(Length::Fixed(HOUR_ROW))
                    .center_y(Length::Fixed(HOUR_ROW))
            ]
            .spacing(4);
            for day in 0..7_usize {
                let now_cell = day == 0 && hour == current_hour;
                let mut content = column![].spacing(2);
                if let Some(entries) = cells.get(&(day, hour)) {
                    for (name, mode, count) in entries.iter().take(2) {
                        let color = crate::shell::mode_color(p, *mode);
                        let label = if *count > 1 {
                            format!("{} ×{count}", model::ellipsize(name, 14))
                        } else {
                            model::ellipsize(name, 18)
                        };
                        content = content.push(
                            container(ui::strong(color, label, 11.0))
                                .padding(Padding::from([0, 6]))
                                .width(Length::Fill)
                                .style(theme::pill(theme::alpha(color, 0.14), color)),
                        );
                    }
                    if entries.len() > 2 {
                        content = content.push(ui::caption(p, format!("+{}", entries.len() - 2)));
                    }
                }
                line = line.push(
                    container(content)
                        .padding(2)
                        .width(Length::FillPortion(1))
                        .height(Length::Fixed(HOUR_ROW))
                        .style(move |_| iced::widget::container::Style {
                            background: now_cell.then(|| highlight.into()),
                            ..iced::widget::container::Style::default()
                        }),
                );
            }
            grid = grid.push(line);
        }
        let mut body = column![
            row![
                ui::heading(p, "Cette semaine"),
                ui::fill(),
                ui::caption(p, "Heures UTC · sept jours à partir d’aujourd’hui"),
            ]
            .align_y(Alignment::Center),
            container(grid)
                .padding(10)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false)),
        ]
        .spacing(12);
        if !event_driven.is_empty() {
            body = body.push(ui::muted(
                p,
                format!(
                    "Sans horaire fixe (déclenchés par la session) : {}.",
                    event_driven.join(", ")
                ),
            ));
        }
        body.into()
    }
}

impl App {
    pub(crate) fn schedules_page<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let now = model::now_secs();
        let mut list = column![].spacing(10);
        let mut rows: Vec<_> = self.ws.schedules.iter().collect();
        rows.sort_by_key(|s| s.schedule.approved.name.to_lowercase());
        for s in rows {
            let id = s.schedule.approved.id.clone();
            let valid = self.schedule_valid(&id);
            let next = draft::next_occurrence(&s.schedule.trigger, s.schedule.created_at, now);
            let (status_color, status) = match (valid, s.running_slot, &s.result) {
                (_, Some(_), _) => (
                    p.warning,
                    "En cours ou interrompu — diagnostic requis".to_owned(),
                ),
                (Some(false) | None, _, _) => (
                    p.danger,
                    "Profil modifié ou retiré : autorisation à renouveler".to_owned(),
                ),
                (_, _, Some(r)) => (
                    match r.status {
                        sync_scheduler::JobStatus::Succeeded => p.success,
                        sync_scheduler::JobStatus::Failed => p.danger,
                        _ => p.warning,
                    },
                    format!("Dernier passage : {}", schedule_status(r)),
                ),
                _ => (p.muted, "Jamais exécuté".to_owned()),
            };
            let name = self
                .profile(&id)
                .map_or(s.schedule.approved.name.clone(), |pr| pr.name.clone());
            list = list.push(
                container(
                    row![
                        ui::glyph_box(Glyph::Clock, p.accent, 40.0),
                        column![
                            ui::strong(p.text, name, 15.0),
                            ui::muted(p, draft::trigger_label(&s.schedule.trigger)),
                            row![
                                ui::dot(status_color, 8.0),
                                ui::colored(status_color, status, 12.0)
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                        ]
                        .spacing(4)
                        .width(Length::Fill),
                        column![
                            ui::caption(p, "Prochaine échéance"),
                            ui::strong(
                                p.text,
                                next.map_or("Événement de session".to_owned(), |n| {
                                    model::relative(n, now)
                                }),
                                14.0
                            ),
                            ui::caption(
                                p,
                                if s.schedule.allow_deletions {
                                    "Suppressions protégées autorisées"
                                } else {
                                    "Sans suppression"
                                }
                            ),
                        ]
                        .spacing(2)
                        .align_x(Alignment::End),
                        ui::icon_button(
                            p,
                            Glyph::Pencil,
                            "Revoir la planification",
                            Some(Message::OpenSchedule(id.clone()))
                        ),
                        ui::icon_button(
                            p,
                            Glyph::Check,
                            "Diagnostiquer l’interruption",
                            (s.running_slot.is_some() && !self.running_on(&id) && !self.busy())
                                .then(|| Message::AskDiagnose(id.clone()))
                        ),
                        ui::icon_button(
                            p,
                            Glyph::Trash,
                            "Retirer la planification",
                            (s.running_slot.is_none() && !self.busy())
                                .then_some(Message::RemoveSchedule(id))
                        ),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center),
                )
                .padding(16)
                .style(theme::card(*p, 0.0, false)),
            );
        }
        if self.ws.schedules.is_empty() {
            list = list.push(empty(p));
        }
        let mut page = column![
            row![
                column![
                    ui::title(p, "Planification", 26.0),
                    ui::muted(
                        p,
                        "Les horaires sont exécutés par l’agent autonome, même fenêtre fermée."
                    ),
                ]
                .spacing(4)
                .width(Length::Fill),
                row([(false, "Liste"), (true, "Semaine")].map(|(week, label)| {
                    button(ui::colored(
                        if self.schedule_week == week {
                            p.text
                        } else {
                            p.muted
                        },
                        label,
                        13.0,
                    ))
                    .padding(Padding::from([5, 14]))
                    .style(theme::chip(*p, p.violet, self.schedule_week == week))
                    .on_press(Message::ScheduleWeek(week))
                    .into()
                }))
                .spacing(6),
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Refresh),
                    "Actualiser",
                    Some(Message::Refresh)
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(20);
        if cfg!(windows) {
            if let Some(error) = &self.ws.supervision_error {
                page = page.push(ui::callout(
                    p,
                    p.danger,
                    Glyph::Info,
                    "Supervision Windows",
                    error.to_string(),
                ));
            }
            let observed = &self.ws.resident;
            let (color, label) = if self.ws.agent_stopped
                || observed.phase == sync_orchestrator::ResidentPhase::Paused
            {
                (p.warning, "Planifications suspendues")
            } else if observed.phase == sync_orchestrator::ResidentPhase::Error {
                (p.danger, "Erreur de l’agent")
            } else if !observed.available {
                (
                    p.warning,
                    "Agent indisponible — dernière observation périmée",
                )
            } else if observed.phase == sync_orchestrator::ResidentPhase::Starting {
                (p.info, "Agent en cours de démarrage")
            } else if observed.updated_at.abs_diff(now) > 30 && self.agent_job.is_none() {
                (
                    p.warning,
                    "Agent sans observation récente — vérifier son état",
                )
            } else if self.agent_job.is_some() {
                (p.info, "Synchronisation en cours")
            } else if observed.phase == sync_orchestrator::ResidentPhase::Attention {
                (
                    p.warning,
                    "Agent démarré — un profil nécessite votre attention",
                )
            } else {
                (p.success, "Agent disponible")
            };
            page = page.push(ui::colored(color, label, 13.0));
            if let Some(diagnostic) = &observed.diagnostic {
                page = page.push(ui::callout(
                    p,
                    p.danger,
                    Glyph::Info,
                    "Dernière erreur du processus agent",
                    diagnostic,
                ));
            }
        }
        if self.ws.agent_stopped {
            page = page.push(
                container(
                    row![
                        icons::icon(Glyph::Stop, 18.0, p.warning),
                        column![
                            ui::strong(p.text, "Planifications en pause", 14.0),
                            ui::muted(p, "Un arrêt a été demandé à l’agent : aucune synchronisation planifiée ne démarre tant que vous ne reprenez pas."),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        ui::action(
                            p,
                            Tone::Primary,
                            Some(Glyph::Refresh),
                            "Reprendre les planifications",
                            Some(Message::ResumeAgent)
                        ),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding(14)
                .width(Length::Fill)
                .style(theme::outline(*p, p.warning, 0.6)),
            );
        }
        if !self.ws.schedules_fresh && self.busy() {
            page = page.push(ui::callout(
                p,
                p.info,
                Glyph::Info,
                "Liste figée pendant la tâche",
                "Les autorisations seront relues dès la fin de la tâche en cours.",
            ));
        }
        page = page.push(if self.schedule_week && !self.ws.schedules.is_empty() {
            self.week_view(p, now)
        } else {
            list.into()
        }).push(
            container(
                row![
                    ui::glyph_box(Glyph::Monitor, p.success, 44.0),
                    column![
                        ui::strong(p.text, "Agent système", 15.0),
                        ui::muted(p, "Windows : les tâches sont enregistrées automatiquement. Une demande UAC apparaît uniquement si Windows exige des droits supplémentaires. Linux : installer les unités systemd utilisateur selon le guide."),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                ]
                .spacing(14)
                .align_y(Alignment::Center),
            )
            .padding(16)
            .style(theme::outline(*p, p.success, 0.5)),
        );
        scrollable(
            container(
                container(page)
                    .padding(Padding::from([28, 32]))
                    .max_width(1100.0),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        )
        .style(theme::scroll(*p))
        .height(Length::Fill)
        .into()
    }
}

fn empty<'a>(p: &Palette) -> El<'a, Message> {
    container(
        column![
            icons::bold(Glyph::Calendar, 48.0, p.faint),
            ui::heading(p, "Aucune planification"),
            ui::muted(p, "Clic droit sur un profil puis « Planifier… », ou choisissez un horaire à la fin de l’assistant."),
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .padding(40)
    .width(Length::Fill)
    .center_x(Length::Fill)
    .style(theme::outline(*p, p.border, 0.0))
    .into()
}

fn schedule_status(r: &sync_scheduler::Completion) -> String {
    let status = match r.status {
        sync_scheduler::JobStatus::Succeeded => "réussi",
        sync_scheduler::JobStatus::Attention => "à vérifier",
        sync_scheduler::JobStatus::Failed => "échec",
        sync_scheduler::JobStatus::Deferred => "reporté",
    };
    if r.code == "trash_insufficient" {
        "bloqué : corbeille insuffisante, lancez-la manuellement".to_owned()
    } else if r.code.is_empty() {
        status.to_owned()
    } else {
        format!("{status} ({})", r.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steppers_wrap_and_consents_reset_on_kind_change() {
        let mut d = ScheduleDraft::default();
        apply(&mut d, ScheduleMsg::Hour(-13));
        assert_eq!(d.hour, 23);
        apply(&mut d, ScheduleMsg::Minute(-5));
        assert_eq!(d.minute, 55);
        apply(&mut d, ScheduleMsg::Kind(TriggerKind::Interval));
        apply(&mut d, ScheduleMsg::Deletions(true));
        assert!(!d.approve_deletions, "deletions require writes first");
        apply(&mut d, ScheduleMsg::Writes(true));
        apply(&mut d, ScheduleMsg::Deletions(true));
        assert!(d.approve_deletions);
        apply(&mut d, ScheduleMsg::Kind(TriggerKind::Daily));
        assert!(!d.approve_writes && !d.approve_deletions);
        apply(&mut d, ScheduleMsg::Interval("12a3".into()));
        assert_eq!(d.interval_minutes, "123");
    }

    #[test]
    fn window_preference_is_not_a_consent_and_needs_support() {
        let mut d = ScheduleDraft::default();
        assert!(!d.show_window, "never pre-ticked");
        apply(&mut d, ScheduleMsg::Window(true));
        assert_eq!(d.show_window, draft::window_supported());
        apply(&mut d, ScheduleMsg::Kind(TriggerKind::Daily));
        assert_eq!(
            d.show_window,
            draft::window_supported(),
            "a new trigger resets consents, not the display preference"
        );
        apply(&mut d, ScheduleMsg::Window(false));
        assert!(!d.show_window);
    }
}
