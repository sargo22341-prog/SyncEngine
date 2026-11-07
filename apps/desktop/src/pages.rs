//! Overview dashboard, activity feed, history and settings pages.

use crate::app::{App, JobKind, MenuTarget, Message, Nav, OverviewFilter, SEARCH_ID, SettingsTab};
use crate::charts;
use crate::draft;
use crate::icons::{self, Glyph};
use crate::model::{self, Outcome};
use crate::shell::{mode_fill, outcome_color};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use crate::ui_prefs::{CloseAction, ThemeChoice};
use iced::widget::{
    Space, button, column, container, mouse_area, row, scrollable, text_input, toggler,
};
use iced::{Alignment, Color, Element, Length, Padding};
use sync_profiles::Profile;

/// « Aujourd’hui », « Hier » or the date: section title of a run list.
fn day_heading(secs: i64, now: i64) -> String {
    let day = |t: i64| t.div_euclid(86_400);
    match day(now) - day(secs) {
        0 => "Aujourd’hui".to_owned(),
        1 => "Hier".to_owned(),
        _ => {
            let (year, month, day, _, _) = model::utc(secs);
            format!("{day:02}/{month:02}/{year}")
        }
    }
}

/// Counters of a run as small badges: additions, problems, volume.
pub(crate) fn run_badges<'a>(p: &Palette, run: &model::RunView) -> El<'a, Message> {
    let total = |key: &str| {
        run.totals
            .iter()
            .find(|(k, _)| k == key)
            .map_or(0, |(_, v)| *v)
    };
    let mut badges = row![].spacing(6);
    let done = total("done");
    if done > 0 {
        badges = badges.push(ui::badge(
            theme::alpha(p.backup, if p.is_light() { 0.22 } else { 0.16 }),
            p.backup_ink,
            format!("+{}", model::count(done)),
        ));
    }
    let failed = total("failed");
    if failed > 0 {
        badges = badges.push(ui::badge(
            theme::alpha(p.danger, 0.14),
            p.danger,
            format!("{} échec(s)", model::count(failed)),
        ));
    }
    let refused = total("refused");
    if refused > 0 {
        badges = badges.push(ui::badge(
            theme::alpha(p.warning, 0.22),
            p.warning,
            format!("{} refusé(s)", model::count(refused)),
        ));
    }
    let bytes = total("bytes");
    if bytes > 0 {
        badges = badges.push(ui::badge(p.surface_hi, p.muted, model::bytes(bytes)));
    }
    badges.into()
}

/// One run: outcome dot, profile and result, mode chip, counters, age.
/// Opens the history of its profile.
fn run_row<'a>(
    p: &Palette,
    profile: &'a Profile,
    run: &'a model::RunView,
    now: i64,
) -> El<'a, Message> {
    let c = outcome_color(p, run.outcome);
    let dot = match run.outcome {
        Outcome::Success => p.backup,
        Outcome::Warning => p.warning,
        _ => p.danger,
    };
    let _ = c;
    button(
        row![
            ui::dot(dot, 10.0),
            column![
                ui::strong(p.text, model::ellipsize(&profile.name, 40), 14.0),
                ui::colored(p.muted, model::ellipsize(&run.label, 60), 12.0),
            ]
            .spacing(1)
            .width(Length::Fill),
            container(ui::mode_chip(p, profile.mode)).width(Length::Fixed(150.0)),
            container(run_badges(p, run)).width(Length::Fixed(220.0)),
            container(ui::mono(
                p.muted,
                run.started.map_or("—".into(), |s| model::relative(s, now)),
                12.0
            ))
            .width(Length::Fixed(90.0))
            .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([10, 12]))
    .width(Length::Fill)
    .style(theme::row_btn(*p, false))
    .on_press(Message::ShowHistory(profile.id.clone()))
    .into()
}

/// Runs grouped under « Aujourd’hui » / « Hier » / date headings.
fn run_groups<'a>(
    p: &Palette,
    feed: &[(&'a Profile, &'a model::RunView)],
    now: i64,
) -> El<'a, Message> {
    let mut col = column![].spacing(2);
    let mut last: Option<String> = None;
    for (profile, run) in feed {
        let heading = run
            .started
            .map_or_else(|| "Date inconnue".to_owned(), |t| day_heading(t, now));
        if last.as_ref() != Some(&heading) {
            col = col.push(container(ui::eyebrow(p.muted, &heading)).padding(Padding {
                top: if last.is_some() { 14.0 } else { 6.0 },
                bottom: 6.0,
                left: 12.0,
                right: 12.0,
            }));
            last = Some(heading);
        }
        col = col.push(run_row(p, profile, run, now));
    }
    col.into()
}

fn page<'a>(p: &Palette, content: impl Into<El<'a, Message>>, width: f32) -> El<'a, Message> {
    scrollable(
        container(
            container(content)
                .padding(Padding::from([26, 32]))
                .max_width(width),
        )
        .width(Length::Fill)
        .center_x(Length::Fill),
    )
    .style(theme::scroll(*p))
    .height(Length::Fill)
    .into()
}

impl App {
    /// Upcoming timed runs, soonest first (presentation only).
    pub(crate) fn upcoming(&self) -> Vec<(i64, String)> {
        let now = model::now_secs();
        let mut list: Vec<(i64, String)> = self
            .ws
            .schedules
            .iter()
            .filter(|s| self.schedule_valid(&s.schedule.approved.id) == Some(true))
            .filter_map(|s| {
                draft::next_occurrence(&s.schedule.trigger, s.schedule.created_at, now).map(|at| {
                    let id = &s.schedule.approved.id;
                    (
                        at,
                        self.profile(id)
                            .map_or_else(|| id.to_string(), |pr| pr.name.clone()),
                    )
                })
            })
            .collect();
        list.sort();
        list
    }

    /// « Live » summary card: big throughput, curve and progress, or a calm
    /// last-run summary when nothing runs.
    fn live_summary<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let Some(job) = self.live_job() else {
            let now = model::now_secs();
            let latest = self
                .ws
                .runs
                .values()
                .filter_map(|r| r.first().and_then(|r| r.started))
                .max();
            let summary = match latest {
                Some(t) => format!("Dernière exécution {}.", model::relative(t, now)),
                None => "Aucune exécution pour l’instant.".into(),
            };
            return container(
                column![
                    ui::eyebrow(p.muted, "En direct"),
                    ui::metric(p, "—", "", 44.0),
                    ui::muted(p, "Rien en cours"),
                    ui::caption(p, summary),
                ]
                .spacing(8),
            )
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fixed(192.0))
            .style(theme::card(*p, 0.0, false))
            .into();
        };
        let rates = self.job_rates(job);
        let total = rates.forward.last().copied().unwrap_or(0.0)
            + rates
                .reverse
                .as_ref()
                .and_then(|r| r.last())
                .copied()
                .unwrap_or(0.0);
        let rate = match (job.kind, rates.forward.last()) {
            (_, None) => ("…".to_owned(), String::new()),
            (JobKind::Preview, Some(r)) => (model::count(*r as u64), "éléments/s".to_owned()),
            (_, Some(_)) => crate::profile::rate_parts(total),
        };
        let color = self
            .job_mode(job)
            .map_or(p.violet, |mode| mode_fill(p, mode));
        let progress = job.progress();
        // Percentage and estimated time left, under the bar.
        let caption = match progress {
            Some(f) => {
                let done = model::percent(f);
                match job.remaining_label(self.now) {
                    Some(left) => format!("{done} · {left}"),
                    None => match job.kind {
                        JobKind::Execute => format!("{done} du volume"),
                        JobKind::Preview => format!("{done} des empreintes"),
                    },
                }
            }
            None => "Analyse en cours…".to_owned(),
        };
        button(
            container(
                column![
                    row![
                        ui::eyebrow(p.info, "En direct"),
                        ui::fill(),
                        ui::status(p, p.violet, model::ellipsize(&job.name, 26)),
                    ]
                    .align_y(Alignment::Center),
                    ui::metric(p, rate.0, rate.1, 38.0),
                    charts::spark(
                        rates.forward.clone(),
                        p.violet,
                        rates.reverse.clone().map(|r| (r, p.backup)),
                        Color::TRANSPARENT,
                        32.0,
                    ),
                    ui::progress(p, progress.unwrap_or(0.0), color, 10.0),
                    ui::caption(p, caption),
                ]
                .spacing(6),
            )
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fixed(192.0))
            .style(theme::tint(
                p.violet,
                if p.is_light() { 0.12 } else { 0.17 },
                theme::RADIUS,
            )),
        )
        .padding(0)
        .width(Length::Fill)
        .style(theme::bare_btn())
        .on_press(Message::Nav(Nav::Activity))
        .into()
    }

    /// « À traiter » card: what needs a decision, with the shortcut to it.
    fn attention_summary<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let problems = self
            .ws
            .profiles
            .iter()
            .filter(|pr| {
                self.last_run(&pr.id)
                    .is_some_and(|r| r.outcome != Outcome::Success)
            })
            .count();
        let renew = self
            .ws
            .schedules
            .iter()
            .filter(|s| self.schedule_valid(&s.schedule.approved.id) != Some(true))
            .count();
        let line = |value: usize,
                    label: &'static str,
                    button_label: &'static str,
                    primary: bool,
                    to: Message|
         -> El<'a, Message> {
            row![
                ui::metric(p, value.to_string(), "", 24.0),
                container(ui::colored(p.text, label, 14.0)).width(Length::Fill),
                ui::action_sm(
                    p,
                    if primary {
                        Tone::Primary
                    } else {
                        Tone::Secondary
                    },
                    None,
                    button_label,
                    Some(to),
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
        };
        if problems + renew == 0 {
            return container(
                column![
                    ui::eyebrow(p.backup_ink, "À traiter"),
                    ui::metric(p, "0", "", 24.0),
                    ui::colored(p.text, "Tout est en ordre", 14.0),
                    ui::caption(p, "Aucun échec récent, aucune autorisation à renouveler."),
                ]
                .spacing(8),
            )
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fixed(192.0))
            .style(theme::tint(
                p.backup,
                if p.is_light() { 0.2 } else { 0.14 },
                theme::RADIUS,
            ))
            .into();
        }
        let mut body = column![ui::eyebrow(p.warning, "À traiter")].spacing(14);
        if problems > 0 {
            body = body.push(line(
                problems,
                if problems == 1 {
                    "exécution à vérifier"
                } else {
                    "exécutions à vérifier"
                },
                "Examiner",
                true,
                Message::Review(crate::review::ReviewMsg::Open),
            ));
        }
        if renew > 0 {
            body = body.push(line(
                renew,
                if renew == 1 {
                    "autorisation à renouveler"
                } else {
                    "autorisations à renouveler"
                },
                "Renouveler",
                problems == 0,
                Message::Renew(crate::review::RenewMsg::Open),
            ));
        }
        container(body)
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fixed(192.0))
            .style(theme::tint(
                p.warning,
                if p.is_light() { 0.22 } else { 0.15 },
                theme::RADIUS,
            ))
            .into()
    }

    fn next_summary<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let upcoming = self.upcoming();
        let body: El<'a, Message> = match upcoming.first() {
            Some((at, name)) => {
                let then: Vec<String> = upcoming
                    .iter()
                    .skip(1)
                    .take(2)
                    .map(|(at, name)| format!("{name} à {}", Self::clock_label(*at)))
                    .collect();
                let mut col = column![
                    ui::eyebrow(p.muted, "Prochaine planification"),
                    ui::metric(p, Self::clock_label(*at), "UTC", 24.0),
                    ui::strong(p.text, model::ellipsize(name, 34), 14.0),
                ]
                .spacing(8);
                if !then.is_empty() {
                    col = col.push(ui::caption(
                        p,
                        model::ellipsize(&format!("puis {}", then.join(" · ")), 60),
                    ));
                }
                col.into()
            }
            None if self.ws.schedules.is_empty() => column![
                ui::eyebrow(p.muted, "Prochaine planification"),
                ui::metric(p, "—", "", 24.0),
                ui::muted(p, "Aucun profil planifié."),
            ]
            .spacing(8)
            .into(),
            None => column![
                ui::eyebrow(p.muted, "Planification"),
                ui::metric(p, self.ws.schedules.len().to_string(), "planifiés", 24.0),
                ui::strong(p.text, "Déclenchés par la session", 14.0),
            ]
            .spacing(8)
            .into(),
        };
        container(
            column![
                body,
                Space::new().height(Length::Fill),
                ui::action_sm(
                    p,
                    Tone::Secondary,
                    None,
                    "Voir le calendrier",
                    Some(Message::Nav(Nav::Schedules))
                ),
            ]
            .spacing(8),
        )
        .padding(20)
        .width(Length::Fill)
        .height(Length::Fixed(192.0))
        .style(theme::card(*p, 0.0, false))
        .into()
    }

    pub(crate) fn overview<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let total = self.ws.profiles.len();
        let named = self.groups().iter().filter(|g| !g.is_empty()).count();
        let plural = |n: usize| if n > 1 { "s" } else { "" };
        let eyebrow = if named == 0 {
            format!("{total} profil{}", plural(total))
        } else {
            format!(
                "{total} profil{} · {named} groupe{}",
                plural(total),
                plural(named)
            )
        };
        let mut col = column![
            mouse_area(
                row![
                    column![
                        ui::eyebrow(p.muted, &eyebrow),
                        ui::title(p, "Vos synchronisations", 32.0),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                    container(
                        row![
                            text_input("Rechercher…", &self.search)
                                .id(SEARCH_ID)
                                .on_input(Message::Search)
                                .padding(Padding::from([9, 0]))
                                .size(14)
                                .style(theme::search(*p)),
                            ui::kbd(p, "Ctrl+F"),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center)
                    )
                    .padding(Padding::from([0, 16]))
                    .width(Length::Fixed(240.0))
                    .style(theme::field_box(*p, 999.0)),
                    ui::action(
                        p,
                        Tone::Secondary,
                        None,
                        "Nouveau groupe",
                        Some(Message::NewGroup(None))
                    ),
                    ui::action(
                        p,
                        Tone::Primary,
                        None,
                        "+ Nouveau profil",
                        Some(Message::New(None))
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
            )
            .on_right_press(Message::OpenMenu(MenuTarget::Background)),
            row![
                self.live_summary(p),
                self.attention_summary(p),
                self.next_summary(p)
            ]
            .spacing(12),
        ]
        .spacing(20);
        if total == 0 {
            col = col.push(onboarding(p));
            return page(p, col, 1240.0);
        }
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        if dragging {
            col = col.push(
                mouse_area(
                    container(
                        row![
                            icons::icon(Glyph::Plus, 16.0, p.accent),
                            ui::colored(p.accent, "Déposer ici pour créer un groupe", 13.0)
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .padding(14)
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .style(theme::drop_zone(
                        *p,
                        true,
                        if self.drop_new { 1.0 } else { 0.0 },
                    )),
                )
                .on_enter(Message::EnterNewZone)
                .on_exit(Message::LeaveZone(String::new())),
            );
        }
        let mut filter = Some(ui::seg(
            p,
            &[
                (OverviewFilter::All, "Tous"),
                (OverviewFilter::Running, "En cours"),
                (OverviewFilter::Review, "À vérifier"),
            ],
            self.overview_filter,
            Message::OverviewFilter,
        ));
        let mut shown = 0;
        for group in self.groups() {
            let members: Vec<&Profile> = self
                .members(&group)
                .into_iter()
                .filter(|pr| self.passes_filter(pr))
                .collect();
            // Empty groups stay visible (with their filter bar): only the
            // profiles are filtered out.
            shown += members.len();
            let label = if group.is_empty() {
                "Sans groupe".to_owned()
            } else {
                group.clone()
            };
            let count = members.len();
            let collapsed = self.prefs.collapsed.contains(&group);
            let rows = members
                .iter()
                .filter(|_| !collapsed)
                .map(|pr| self.profile_card(p, pr))
                .chain(
                    (!collapsed && self.overview_filter == OverviewFilter::All)
                        .then(|| add_card(p, &group)),
                );
            let hot = self.drop_zone.as_deref() == Some(group.as_str());
            // The heading opens and closes the group (also in its context menu).
            let toggle = button(
                row![
                    icons::icon(
                        if collapsed {
                            Glyph::ChevronRight
                        } else {
                            Glyph::ChevronDown
                        },
                        14.0,
                        p.muted
                    ),
                    ui::eyebrow(p.muted, &label),
                    ui::badge(p.surface_hi, p.muted, count.to_string()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([4, 6]))
            .style(theme::row_btn(*p, false))
            .on_press(Message::ToggleGroup(group.clone()));
            let mut head = row![toggle, ui::fill()]
                .spacing(8)
                .align_y(Alignment::Center);
            if let Some(f) = filter.take() {
                head = head.push(f);
            }
            head = head.push(ui::icon_button(
                p,
                Glyph::Clock,
                "Planifier le groupe",
                Some(Message::ScheduleGroup(group.clone())),
            ));
            let section = column![
                mouse_area(head)
                    .on_right_press(Message::OpenMenu(MenuTarget::Group(group.clone()))),
                column(rows).spacing(8),
            ]
            .spacing(10);
            col = col.push(
                mouse_area(
                    container(section)
                        .padding(Padding::from([4, 0]))
                        .style(theme::drop_zone(*p, dragging, if hot { 1.0 } else { 0.0 })),
                )
                .on_enter(Message::EnterZone(group.clone()))
                .on_exit(Message::LeaveZone(group)),
            );
        }
        if shown == 0 {
            col = col.push(ui::callout(
                p,
                p.info,
                Glyph::Search,
                "Aucun profil ne correspond",
                "Changez le filtre ou la recherche pour retrouver vos profils.",
            ));
        }
        page(p, col, 1240.0)
    }

    /// Whether the overview filter keeps this profile.
    fn passes_filter(&self, profile: &Profile) -> bool {
        match self.overview_filter {
            OverviewFilter::All => true,
            OverviewFilter::Running => self.running_on(&profile.id),
            OverviewFilter::Review => {
                self.last_run(&profile.id)
                    .is_some_and(|r| r.outcome != Outcome::Success)
                    || self.schedule_valid(&profile.id) == Some(false)
            }
        }
    }

    /// Trash overview: one line per profile with its policy; the content of a
    /// profile trash opens in the existing dialog (sizes load there, not here).
    pub(crate) fn trash_page<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let mut list = column![].spacing(8);
        for profile in &self.ws.profiles {
            let (policy, dot) = if profile.trash.enabled {
                (
                    format!("Activée · {}", model::bytes(profile.trash.max_bytes)),
                    p.backup,
                )
            } else {
                ("Désactivée".to_owned(), p.faint)
            };
            list = list.push(
                container(
                    row![
                        ui::mode_tile(p, profile.mode, 36.0),
                        container(ui::strong(
                            p.text,
                            model::ellipsize(&profile.name, 48),
                            14.0
                        ))
                        .width(Length::Fill),
                        container(ui::mode_chip(p, profile.mode)).width(Length::Fixed(150.0)),
                        container(ui::status(p, dot, policy)).width(Length::Fixed(170.0)),
                        ui::action_sm(
                            p,
                            Tone::Secondary,
                            None,
                            "Voir la corbeille",
                            Some(Message::Trash(crate::trash::TrashMsg::Open(
                                profile.id.clone()
                            ))),
                        ),
                    ]
                    .spacing(16)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([12, 16]))
                .width(Length::Fill)
                .style(theme::row_card(*p, 0.0, false)),
            );
        }
        if self.ws.profiles.is_empty() {
            list = list.push(ui::callout(
                p,
                p.info,
                Glyph::Trash,
                "Aucun profil",
                "Chaque profil a sa propre corbeille, réglée dans l’étape « Sécurité » de l’assistant.",
            ));
        }
        page(
            p,
            column![
                column![
                    ui::eyebrow(p.muted, "Anciennes versions des fichiers remplacés ou supprimés"),
                    ui::title(p, "Corbeille", 32.0),
                ]
                .spacing(4),
                ui::callout(
                    p,
                    p.info,
                    Glyph::Info,
                    "Pas de restauration en place",
                    "Un miroir supprimerait de nouveau le fichier. Utilisez « Enregistrer sous… » dans la corbeille d’un profil.",
                ),
                list,
            ]
            .spacing(18),
            1100.0,
        )
    }

    fn profile_card<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        let lift = self.lift.get(&profile.id).map_or(0.0, |t| t.at(self.now));
        let selected = self.selected.as_ref() == Some(&profile.id);
        let now = model::now_secs();
        let live = [&self.job, &self.agent_job]
            .into_iter()
            .flatten()
            .find(|j| j.profile == profile.id);
        let (dot, status) = if live.is_some() {
            (p.violet, "Synchronisation")
        } else {
            match self.last_run(&profile.id) {
                Some(run) => match run.outcome {
                    Outcome::Success => (p.backup, "À jour"),
                    Outcome::Warning => (p.warning, "À vérifier"),
                    Outcome::Failure => (p.danger, "Échec"),
                    Outcome::Interrupted => (p.danger, "Interrompu"),
                },
                None => (p.faint, "Jamais lancé"),
            }
        };
        let when = match live {
            Some(job) => job
                .progress()
                .map_or_else(|| "analyse".to_owned(), model::percent),
            None => self
                .last_run(&profile.id)
                .and_then(|r| r.started)
                .map_or("jamais".to_owned(), |t| model::relative(t, now)),
        };
        let arrow = match profile.mode {
            sync_plan::SyncMode::Backup => "→",
            sync_plan::SyncMode::Mirror => "⇉",
            _ => "⇄",
        };
        let mut text_col = column![
            ui::strong(p.text, model::ellipsize(&profile.name, 44), 15.0),
            container(ui::mono(
                p.muted,
                format!(
                    "{} {arrow} {}",
                    model::source_of(profile),
                    model::destination_of(profile)
                ),
                12.0
            ))
            .width(Length::Fill)
            .clip(true),
        ]
        .spacing(2);
        if let Some(job) = live {
            text_col = text_col.push(ui::vspace(4.0)).push(ui::progress(
                p,
                job.progress().unwrap_or(0.0),
                mode_fill(p, profile.mode),
                6.0,
            ));
        }
        let action: El<'a, Message> = if live.is_some() {
            ui::action_sm(
                p,
                Tone::Secondary,
                None,
                "Suivre",
                Some(Message::Nav(Nav::Activity)),
            )
        } else {
            ui::action_sm(
                p,
                Tone::Primary,
                None,
                "Simuler",
                (!self.busy()).then(|| Message::Simulate(profile.id.clone())),
            )
        };
        let body = row![
            ui::mode_tile(p, profile.mode, 44.0),
            container(text_col).width(Length::Fill).clip(true),
            container(ui::mode_chip(p, profile.mode)).width(Length::Fixed(150.0)),
            container(ui::status(p, dot, status)).width(Length::Fixed(130.0)),
            container(ui::colored(p.muted, when, 12.0))
                .width(Length::Fixed(110.0))
                .align_x(iced::alignment::Horizontal::Right),
            container(action)
                .width(Length::Fixed(90.0))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .spacing(16)
        .align_y(Alignment::Center);
        let lifted = container(body)
            .padding(Padding::from([12, 16]))
            .width(Length::Fill)
            .style(theme::row_card(*p, lift, selected));
        mouse_area(lifted)
            .on_press(Message::Press(profile.id.clone()))
            .on_double_click(Message::Edit(profile.id.clone()))
            .on_right_press(Message::OpenMenu(MenuTarget::Profile(profile.id.clone())))
            .on_enter(Message::Hover(Some(profile.id.clone())))
            .on_exit(Message::Hover(None))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    }

    pub(crate) fn activity_page<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let live: Vec<_> = [&self.job, &self.agent_job].into_iter().flatten().collect();
        let mut col = column![].spacing(18);
        if live.is_empty() {
            col = col.push(
                column![
                    ui::eyebrow(p.muted, "Suivi en direct et derniers résultats"),
                    ui::title(p, "Activité", 32.0),
                ]
                .spacing(4),
            );
        }
        for job in &live {
            col = col.push(self.live_panel(p, job, true));
        }
        if let Some(queue) = self.agent_queue_panel(p) {
            col = col.push(queue);
        }
        if !live.is_empty() {
            // Live panels above; the last result returns once they end.
        } else if let Some(last) = &self.last_run {
            col = col.push(self.result_card(p, last));
        } else {
            col = col.push(ui::callout(
                p,
                p.info,
                Glyph::Pulse,
                "Aucune tâche en cours dans cette fenêtre",
                "Lancez une simulation depuis un profil (F5). Les synchronisations planifiées de l’agent s’affichent ici en direct.",
            ));
        }
        let now = model::now_secs();
        let mut feed: Vec<(&Profile, &model::RunView)> = self
            .ws
            .profiles
            .iter()
            .flat_map(|pr| {
                self.ws
                    .runs
                    .get(&pr.id)
                    .into_iter()
                    .flatten()
                    .map(move |r| (pr, r))
            })
            .collect();
        feed.sort_by_key(|(_, r)| std::cmp::Reverse(r.started.unwrap_or(0)));
        feed.truncate(15);
        let list = column![
            row![
                ui::heading(p, "Derniers résultats"),
                ui::fill(),
                ui::action_sm(
                    p,
                    Tone::Secondary,
                    None,
                    "Actualiser",
                    Some(Message::Refresh)
                )
            ]
            .padding(Padding::from([6, 12]))
            .align_y(Alignment::Center),
            if feed.is_empty() {
                ui::muted(p, "Aucun résultat enregistré.")
            } else {
                run_groups(p, &feed, now)
            },
        ]
        .spacing(4);
        col = col.push(
            container(list)
                .padding(12)
                .style(theme::card(*p, 0.0, false)),
        );
        page(p, col, 1240.0)
    }

    pub(crate) fn history_page<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let Some(profile) = self.selected.as_ref().and_then(|id| self.profile(id)) else {
            let now = model::now_secs();
            let mut feed: Vec<(&Profile, &model::RunView)> = self
                .ws
                .profiles
                .iter()
                .flat_map(|pr| {
                    self.ws
                        .runs
                        .get(&pr.id)
                        .into_iter()
                        .flatten()
                        .map(move |r| (pr, r))
                })
                .collect();
            feed.sort_by_key(|(_, r)| std::cmp::Reverse(r.started.unwrap_or(0)));
            let total = feed.len();
            let shown = self.history_limit.min(total);
            let list: El<'a, Message> = if total == 0 {
                ui::muted(p, "Aucune exécution enregistrée.")
            } else {
                run_groups(p, &feed[..shown], now)
            };
            let mut col = column![
                column![
                    ui::eyebrow(p.muted, "Dernières exécutions de tous les profils"),
                    ui::title(p, "Historique", 32.0),
                ]
                .spacing(4),
                container(list)
                    .padding(12)
                    .style(theme::card(*p, 0.0, false)),
            ]
            .spacing(16);
            if total > 0 {
                let mut footer = row![ui::caption(p, format!("{shown} sur {total} exécutions"))]
                    .spacing(14)
                    .align_y(Alignment::Center);
                if shown < total {
                    footer = footer.push(ui::action(
                        p,
                        Tone::Secondary,
                        None,
                        "Afficher plus",
                        Some(Message::HistoryMore),
                    ));
                }
                col = col.push(footer);
            }
            return page(p, col, 1240.0);
        };
        let now = model::now_secs();
        let runs = self
            .ws
            .runs
            .get(&profile.id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut list = column![].spacing(8);
        for run in runs {
            let c = outcome_color(p, run.outcome);
            let open = self.expanded_run.as_deref() == Some(run.execution.as_str());
            let totals = run.totals.iter().filter(|(_, v)| *v > 0).map(|(k, v)| {
                let value = if k == "bytes" {
                    model::bytes(*v)
                } else {
                    model::count(*v)
                };
                ui::badge(
                    theme::alpha(p.text, 0.08),
                    p.muted,
                    format!("{value} {}", model::total_label(k)),
                )
            });
            let header = button(
                row![
                    container(Space::new())
                        .width(Length::Fixed(4.0))
                        .height(Length::Fixed(40.0))
                        .style(theme::bar(c)),
                    column![
                        ui::strong(p.text, run.label.clone(), 14.0),
                        ui::caption(
                            p,
                            run.started.map_or("Date inconnue".into(), |s| format!(
                                "{} · {}",
                                model::utc_label(s),
                                model::relative(s, now)
                            ))
                        ),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    row(totals).spacing(6),
                    icons::icon(
                        if open {
                            Glyph::ChevronDown
                        } else {
                            Glyph::ChevronRight
                        },
                        14.0,
                        p.faint
                    ),
                ]
                .spacing(14)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([10, 12]))
            .width(Length::Fill)
            .style(theme::row_btn(*p, open))
            .on_press(Message::ExpandRun(run.execution.clone()));
            let mut card = column![header].spacing(8);
            if open {
                if let Some(m) = &run.message {
                    card = card.push(ui::callout(p, c, Glyph::Info, "Diagnostic", m.clone()));
                }
                card = card.push(self.history_rows(p));
            }
            list = list.push(container(card).padding(6).style(theme::card(*p, 0.0, open)));
        }
        if runs.is_empty() {
            list = list.push(ui::callout(p, p.info, Glyph::History, "Aucune exécution enregistrée", "Simulez puis exécutez ce profil : chaque tentative, même interrompue, sera conservée ici."));
        }
        page(
            p,
            column![
                row![
                    column![ui::eyebrow(p.accent, "Historique"), ui::title(p, profile.name.clone(), 26.0)].spacing(4).width(Length::Fill),
                    ui::action(p, Tone::Secondary, Some(Glyph::Refresh), "Actualiser", Some(Message::Refresh)),
                ]
                .align_y(Alignment::Center),
                ui::muted(p, "Un état « en cours » après redémarrage signale une interruption à diagnostiquer, jamais un succès."),
                list,
            ]
            .spacing(16),
            1100.0,
        )
    }

    fn history_rows<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        use sync_orchestrator::HistoryFilter;
        // Rows can be excluded from the profile being shown.
        let excludable = self.selected.is_some();
        let filters = row([
            (HistoryFilter::All, "Toutes"),
            (HistoryFilter::Problems, "Problèmes"),
            (HistoryFilter::Failed, "Échecs"),
            (HistoryFilter::Refused, "Refusés"),
            (HistoryFilter::Unfinished, "Annulées / non commencées"),
            (HistoryFilter::Skipped, "Ignorées / conflits"),
        ]
        .into_iter()
        .map(|(filter, label)| {
            button(ui::caption(p, label))
                .padding([6, 10])
                .style(theme::row_btn(*p, self.history_filter == filter))
                .on_press(Message::HistoryFilter(filter))
                .into()
        }))
        .spacing(6)
        .wrap();
        let controls = column![
            row![filters, ui::fill(), crate::profile::mode_switch(p, self.list_mode)]
                .spacing(10)
                .align_y(Alignment::Center),
            text_input(
                "Rechercher un chemin ou un diagnostic… (Entrée pour appliquer)",
                &self.history_query
            )
            .on_input(Message::HistoryQuery)
            .on_submit(Message::HistorySearch)
            .padding(10)
            .style(theme::input(*p, false)),
            ui::caption(
                p,
                "Seuls les changements, erreurs et avertissements sont conservés : les éléments identiques ne sont pas enregistrés."
            )
        ]
        .spacing(8);
        let filtered = self.history_filter != sync_orchestrator::HistoryFilter::All
            || !self.history_query.is_empty();
        let empty = if filtered {
            "Aucune opération ne correspond à ces filtres."
        } else {
            "Aucun changement enregistré pour cette exécution."
        };
        if self.list_mode == crate::tree::ListMode::Tree {
            let pal = *p;
            let tree = crate::tree::view(
                p,
                &self.history_tree,
                Message::HistoryTree,
                &|r, indent| history_row(&pal, r, Some(indent), excludable),
                &|folder| crate::tree::action_summary(&pal, folder),
                &|_| None,
                empty,
            );
            return column![controls, tree]
                .spacing(8)
                .padding(Padding::from([0, 8]))
                .into();
        }
        let Some(page) = &self.history else {
            return container(
                column![controls, ui::muted(p, "Chargement des opérations…")].spacing(10),
            )
            .padding(12)
            .into();
        };
        let mut col = column![].spacing(2);
        for r in &page.rows {
            col = col.push(history_row(p, r, None, excludable));
        }
        if page.rows.is_empty() {
            col = col.push(ui::muted(p, empty));
        }
        let end = (page.offset + page.rows.len()).min(page.total);
        let pager = row![
            ui::caption(
                p,
                format!(
                    "{}–{} sur {}",
                    if page.total == 0 { 0 } else { page.offset + 1 },
                    end,
                    page.total
                )
            ),
            ui::fill(),
            ui::action(
                p,
                Tone::Ghost,
                Some(Glyph::ChevronLeft),
                "Précédent",
                (page.offset > 0).then(|| Message::HistoryOffset(page.offset.saturating_sub(100)))
            ),
            ui::action(
                p,
                Tone::Ghost,
                Some(Glyph::ChevronRight),
                "Suivant",
                (end < page.total).then_some(Message::HistoryOffset(end))
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        column![controls, col, pager]
            .spacing(8)
            .padding(Padding::from([0, 8]))
            .into()
    }

    pub(crate) fn settings_page<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let theme_tile = |choice: ThemeChoice,
                          label: &'static str,
                          left: Color,
                          right: Color|
         -> El<'a, Message> {
            let selected = self.prefs.theme == choice;
            let swatch = row![
                container(Space::new())
                    .width(Length::FillPortion(1))
                    .height(Length::Fixed(54.0))
                    .style(theme::panel(left)),
                container(Space::new())
                    .width(Length::FillPortion(1))
                    .height(Length::Fixed(54.0))
                    .style(theme::panel(right)),
            ];
            button(
                column![
                    container(swatch)
                        .style(move |_| iced::widget::container::Style {
                            border: iced::Border {
                                color: theme::alpha(Color::BLACK, 0.18),
                                width: 1.0,
                                radius: 10.0.into()
                            },
                            ..iced::widget::container::Style::default()
                        })
                        .clip(true),
                    ui::strong(p.text, label, 14.0),
                ]
                .spacing(10),
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::option(*p, p.violet, selected))
            .on_press(Message::Theme(choice))
            .into()
        };
        let d = theme::Palette::DARK;
        let l = theme::Palette::LIGHT;
        let section = |title: &'static str, body: El<'a, Message>| -> El<'a, Message> {
            container(column![ui::title(p, title, 18.0), body].spacing(16))
                .padding(20)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false))
                .into()
        };
        let startup: El<'a, Message> = if cfg!(windows) {
            row![
                column![
                    ui::strong(p.text, "Ouvrir SyncEngine à l’ouverture de session", 14.0),
                    ui::caption(p, "Indépendant de l’agent : les tâches planifiées fonctionnent même fenêtre fermée."),
                ]
                .spacing(2)
                .width(Length::Fill),
                toggler(self.startup.unwrap_or(false))
                    .on_toggle_maybe(self.startup.is_some().then_some(Message::Startup))
                    .size(22)
                    .style(theme::switch(*p, false)),
            ]
            .align_y(Alignment::Center)
            .into()
        } else {
            ui::muted(
                p,
                "Démarrage automatique de l’interface disponible sous Windows ; sous Linux, utilisez l’autostart de votre bureau.",
            )
        };
        let minimize = self.prefs.close == CloseAction::Minimize;
        let tray_line: El<'a, Message> = match &self.tray {
            Ok(_) => ui::caption(
                p,
                "Icône présente dans la zone de notification : clic pour afficher la fenêtre, clic droit › « Quitter SyncEngine ».",
            ),
            Err(e) => row![
                icons::icon(Glyph::Alert, 14.0, p.warning),
                ui::colored(
                    p.muted,
                    format!(
                        "Icône de notification indisponible ({}). Quittez avec Ctrl+Q ou le bouton ci-contre.",
                        model::ellipsize(e, 120)
                    ),
                    12.0
                )
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        };
        let window_section: El<'a, Message> = column![
            row![
                column![
                    ui::strong(p.text, "Le bouton × réduit SyncEngine", 14.0),
                    ui::caption(
                        p,
                        if minimize {
                            "La fenêtre reste dans la barre des tâches ; les tâches en cours continuent."
                        } else {
                            "Le bouton × quitte l’application ; une tâche en cours est d’abord annulée proprement."
                        }
                    ),
                ]
                .spacing(2)
                .width(Length::Fill),
                toggler(minimize)
                    .on_toggle(|on| Message::CloseBehavior(if on {
                        CloseAction::Minimize
                    } else {
                        CloseAction::Quit
                    }))
                    .size(22)
                    .style(theme::switch(*p, false)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                container(tray_line).width(Length::Fill),
                ui::kbd(p, "Ctrl+Q"),
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Power),
                    "Quitter SyncEngine",
                    Some(Message::Quit)
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(16)
        .into();
        let providers = self.ws.providers.iter().map(|k| {
            let label = draft::Provider::ALL
                .iter()
                .find(|pr| pr.key() == *k)
                .map_or(*k, |pr| pr.label());
            ui::badge(theme::alpha(p.accent, 0.14), p.accent, label)
        });
        let guarantees = [
            "Plan simulé identique au plan exécuté ; toute modification exige une nouvelle simulation.",
            "Copie vérifiée par empreinte BLAKE3 ; comparaison par taille et date ou par contenu, au choix de chaque profil.",
            "Corbeille par profil (facultative) : anciennes versions gardées avant remplacement ou suppression, dans la limite choisie.",
            "Suppressions désactivées par défaut ; source vide ou illisible toujours bloquée ; plus de 50 % de la destination (dès 10 suppressions) bloqué sauf accord explicite dans la simulation.",
            "Un verrou par compte sérialise CLI, interface et agent.",
            "Quitter pendant une tâche demande son annulation ; SyncEngine attend l’état durable.",
        ]
        .into_iter()
        .fold(column![].spacing(8), |c, g| {
            c.push(row![icons::icon(Glyph::Shield, 15.0, p.success), ui::colored(p.text, g, 13.0)].spacing(10).align_y(Alignment::Center))
        });
        let perf = self.ws.preferences.performance;
        let performance_section: El<'a, Message> = column![
            row![
                column![
                    ui::strong(p.text, "Transferts simultanés", 14.0),
                    ui::caption(p, "Fichiers copiés en même temps. Chaque emplacement borne ce nombre à ce qu’il supporte ; le bidirectionnel reste ordonné."),
                ]
                .spacing(2)
                .width(Length::Fill),
                ui::seg(p, &[(1_u8, "1"), (2, "2"), (4, "4"), (8, "8"), (16, "16")], perf.copy_workers, Message::PerfWorkers),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            row![
                column![
                    ui::strong(p.text, "Gros fichiers simultanés", 14.0),
                    ui::caption(p, "À partir de 8 Mio. Plusieurs gros flux sur un même disque dur le ralentissent : 1 est le plus sûr."),
                ]
                .spacing(2)
                .width(Length::Fill),
                ui::seg(p, &[(1_u8, "1"), (2, "2"), (3, "3"), (4, "4")], perf.large_transfers, Message::PerfLarge),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            column![
                row![
                    column![
                        ui::strong(p.text, "Limiter le débit", 14.0),
                        ui::caption(p, "Débit total des copies (tous les fichiers confondus). Les relectures de vérification et l’archivage en corbeille ne sont pas limités. S’applique à la prochaine exécution."),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    toggler(perf.rate_limit_mib_s.is_some())
                        .on_toggle(|on| Message::PerfRate(on.then_some(20)))
                        .size(22)
                        .style(theme::switch(*p, false)),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
                if let Some(current) = perf.rate_limit_mib_s {
                    Element::from(row![
                        ui::seg(p, &[(5_u32, "5 Mio/s"), (10, "10 Mio/s"), (20, "20 Mio/s"), (50, "50 Mio/s"), (100, "100 Mio/s")], current, |n| Message::PerfRate(Some(n))),
                        ui::fill(),
                        container(
                            text_input(&format!("{current}"), &self.rate_input)
                                .on_input(Message::RateInput)
                                .on_submit(Message::RateApply)
                                .font(ui::mono_font())
                                .padding(Padding::from([8, 0]))
                                .size(13)
                                .style(theme::search(*p)),
                        )
                        .padding(Padding::from([0, 12]))
                        .style(theme::field_box(*p, 12.0))
                        .width(Length::Fixed(110.0)),
                        ui::caption(p, "Mio/s · Entrée"),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center))
                } else {
                    Element::from(Space::new())
                },
            ]
            .spacing(10),
        ]
        .spacing(18)
        .into();
        let state = self
            .backend
            .as_ref()
            .map(|a| model::pretty_path(&a.state_directory().display().to_string()))
            .unwrap_or_default();
        let tab = self.settings_tab;
        let appearance: El<'a, Message> = section(
            "Apparence",
            column![
                row![
                    theme_tile(ThemeChoice::Light, "Clair", l.bg, l.bg),
                    theme_tile(ThemeChoice::Dark, "Sombre", d.bg, d.bg),
                    theme_tile(ThemeChoice::System, "Système", l.bg, d.bg,),
                ]
                .spacing(12),
                ui::toggle_row(
                    p,
                    self.ws.preferences.compact,
                    "Affichage compact",
                    Some("Lignes plus serrées pour les longues listes."),
                    false,
                    Message::Compact,
                    None,
                ),
            ]
            .spacing(14)
            .into(),
        );
        let body: El<'a, Message> = match tab {
            SettingsTab::Appearance => appearance,
            SettingsTab::Window => column![
                section("Fenêtre", window_section),
                section("Démarrage", startup),
            ]
            .spacing(14)
            .into(),
            SettingsTab::Performance => section("Performances", performance_section),
            SettingsTab::Keyboard => section(
                "Clavier",
                row![
                    column![
                        ui::strong(p.text, "Raccourcis clavier", 14.0),
                        ui::caption(
                            p,
                            "Toutes les actions principales sont accessibles au clavier."
                        )
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    ui::kbd(p, "F1"),
                    ui::action_sm(
                        p,
                        Tone::Secondary,
                        None,
                        "Afficher",
                        Some(Message::ShowShortcuts)
                    ),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .into(),
            ),
            SettingsTab::Guarantees => section("Garanties d’exécution", guarantees.into()),
            SettingsTab::Data => section(
                "Données",
                column![
                    row![
                        column![
                            ui::strong(p.text, "Dossier de données", 14.0),
                            ui::mono(p.muted, model::ellipsize(&state, 70), 12.0),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        ui::action_sm(
                            p,
                            Tone::Secondary,
                            None,
                            "Ouvrir le dossier",
                            Some(Message::OpenStateFolder)
                        ),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                    row![
                        column![
                            ui::strong(p.text, "Supprimer toutes les données de SyncEngine", 14.0),
                            ui::caption(
                                p,
                                "Profils, historique, corbeilles, planifications et démarrage automatique : tout ce que SyncEngine a laissé sur ce système. Vos fichiers synchronisés ne sont pas touchés."
                            )
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        ui::action_sm(
                            p,
                            Tone::Danger,
                            None,
                            "Supprimer toutes les données…",
                            (!self.busy() && self.agent_job.is_none())
                                .then_some(Message::Purge(crate::purge_view::PurgeMsg::Ask))
                        ),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                ]
                .spacing(16)
                .into(),
            ),
            SettingsTab::Updates => section("Mises à jour", self.updates_section(p)),
            SettingsTab::About => section(
                "Cette version",
                column![
                    row![ui::caption(p, "Connecteurs inclus :")]
                        .extend(providers)
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .wrap(),
                    ui::caption(
                        p,
                        format!(
                            "SyncEngine {} · interface native Rust (iced) · {}",
                            env!("CARGO_PKG_VERSION"),
                            std::env::consts::OS
                        )
                    ),
                ]
                .spacing(10)
                .into(),
            ),
        };
        let menu = column(SettingsTab::ALL.iter().map(|t| {
            let on = tab == *t;
            let fg = if on { p.bg } else { p.muted };
            button(ui::strong(fg, t.label(), 14.0))
                .padding(Padding::from([8, 14]))
                .width(Length::Fill)
                .style(theme::nav_btn(*p, on))
                .on_press(Message::SettingsTab(*t))
                .into()
        }))
        .spacing(2)
        .width(Length::Fixed(220.0));
        let col = column![
            row![
                ui::title(p, "Réglages", 32.0),
                ui::fill(),
                ui::mono(
                    p.muted,
                    format!(
                        "SyncEngine {} · {}",
                        env!("CARGO_PKG_VERSION"),
                        std::env::consts::OS
                    ),
                    12.0
                ),
            ]
            .align_y(Alignment::Center),
            row![menu, container(body).width(Length::Fill)]
                .spacing(24)
                .align_y(Alignment::Start),
        ]
        .spacing(20);
        page(p, col, 1240.0)
    }
}

fn add_card<'a>(p: &Palette, group: &str) -> El<'a, Message> {
    button(
        container(
            row![
                icons::icon(Glyph::Plus, 16.0, p.muted),
                ui::strong(p.muted, "Nouveau profil dans ce groupe", 13.0)
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(40.0)),
    )
    .padding(0)
    .width(Length::Fill)
    .style(theme::ghost_row(*p))
    .on_press(Message::New(Some(group.to_owned())))
    .into()
}

fn onboarding<'a>(p: &Palette) -> El<'a, Message> {
    let mode = |m: sync_plan::SyncMode, body: &'static str| -> El<'a, Message> {
        container(
            column![
                ui::mode_tile(p, m, 44.0),
                ui::title(p, model::mode_label(m), 18.0),
                ui::colored(p.muted, body, 12.0),
            ]
            .spacing(10),
        )
        .padding(20)
        .width(Length::Fill)
        .style(theme::tint(
            mode_fill(p, m),
            if p.is_light() { 0.16 } else { 0.15 },
            theme::RADIUS,
        ))
        .into()
    };
    column![
        column![
            ui::eyebrow(p.muted, "Bienvenue"),
            ui::title(p, "Vos dossiers, toujours d’accord.", 52.0),
            ui::colored(
                p.muted,
                "Choisissez un mode, vérifiez le plan, puis confirmez. Rien n’est écrit sans que vous l’ayez vu.",
                16.0
            ),
        ]
        .spacing(10),
        row![
            mode(
                sync_plan::SyncMode::Backup,
                "Copier sans jamais supprimer. Idéal pour photos et archives."
            ),
            mode(
                sync_plan::SyncMode::Mirror,
                "Destination identique à la source, suppressions confirmées."
            ),
            mode(
                sync_plan::SyncMode::Bidirectional,
                "Deux dossiers qui évoluent ensemble, conflits arbitrés."
            ),
        ]
        .spacing(12),
        row![
            button(ui::strong(p.bg, "Créer mon premier profil", 15.0))
                .padding(Padding::from([14, 28]))
                .style(theme::btn(*p, Tone::Primary))
                .on_press(Message::New(None)),
            ui::colored(p.muted, "ou", 13.0),
            ui::kbd(p, "Ctrl+N"),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ]
    .spacing(32)
    .padding(Padding::from([40, 0]))
    .into()
}

/// One historical operation and its outcome. In the folder view
/// (`indent`), only its name is shown, below its folder.
fn history_row<'a>(
    p: &Palette,
    r: &'a model::HistoryRow,
    indent: Option<f32>,
    excludable: bool,
) -> El<'a, Message> {
    let (status_color, glyph) = match r.status.as_str() {
        "done" => (p.success, Glyph::Check),
        "no_action" => (p.faint, Glyph::Check),
        "refused" | "cancelled" | "not_started" => (p.warning, Glyph::Alert),
        _ => (p.danger, Glyph::Alert),
    };
    let (path, action) = r.op.as_ref().map_or(("?".into(), "—"), |o| {
        let shown = match indent {
            Some(_) => o.path.rsplit('/').next().unwrap_or(&o.path),
            None => o.path.as_str(),
        };
        (shown.to_owned(), model::action_label(o.action))
    });
    let mut detail = r.detail.clone();
    if let Some(v) = &r.version {
        detail = format!("{detail}  · version préservée {v}");
    }
    // Ignoring what a row reports: only for a real entry of the profile.
    let exclude: El<'a, Message> = match &r.op {
        Some(op) if excludable && !op.path.is_empty() => ui::icon_button(
            p,
            Glyph::Shield,
            "Exclure du profil…",
            Some(Message::Exclusion(crate::exclusion::ExclusionMsg::Ask {
                path: op.path.clone(),
                directory: op.kind == "directory",
            })),
        ),
        _ => Space::new().into(),
    };
    row![
        icons::icon(glyph, 14.0, status_color),
        container(ui::caption(p, action)).width(Length::Fixed(90.0)),
        column![
            ui::mono(p.text, model::ellipsize(&path, 80), 12.0),
            ui::caption(p, detail)
        ]
        .spacing(1)
        .width(Length::Fill),
        ui::colored(
            status_color,
            model::outcome_label(&r.status).to_owned(),
            12.0
        ),
        exclude,
    ]
    .spacing(10)
    .padding(Padding {
        top: 5.0,
        bottom: 5.0,
        left: 10.0 + indent.unwrap_or(0.0),
        right: 10.0,
    })
    .align_y(Alignment::Center)
    .into()
}
