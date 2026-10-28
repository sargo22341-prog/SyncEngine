//! Overview dashboard, activity feed, history and settings pages.

use crate::app::{App, MenuTarget, Message, Nav};
use crate::draft;
use crate::icons::{self, Glyph};
use crate::model::{self, Outcome};
use crate::shell::{mode_color, mode_glyph, outcome_color};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use crate::ui_prefs::{CloseAction, ThemeChoice};
use iced::widget::{
    Space, button, column, container, grid, mouse_area, row, scrollable, text_input, toggler,
};
use iced::{Alignment, Color, Length, Padding};
use sync_profiles::Profile;

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

fn stat<'a>(
    p: &Palette,
    glyph: Glyph,
    color: Color,
    value: String,
    label: &'static str,
) -> El<'a, Message> {
    container(
        row![
            ui::glyph_box(glyph, color, 42.0),
            column![ui::title(p, value, 24.0), ui::caption(p, label)].spacing(0),
        ]
        .spacing(14)
        .align_y(Alignment::Center),
    )
    .padding(16)
    .width(Length::Fill)
    .style(theme::card(*p, 0.0, false))
    .into()
}

impl App {
    pub(crate) fn overview<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let total = self.ws.profiles.len();
        let scheduled = self.ws.schedules.len();
        let lasts: Vec<Outcome> = self
            .ws
            .profiles
            .iter()
            .filter_map(|pr| self.last_run(&pr.id).map(|r| r.outcome))
            .collect();
        let ok = lasts.iter().filter(|o| **o == Outcome::Success).count();
        let attention = lasts.iter().filter(|o| **o != Outcome::Success).count()
            + self
                .ws
                .schedules
                .iter()
                .filter(|s| self.schedule_valid(&s.schedule.approved.id) != Some(true))
                .count();
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
        let mut col = column![
            row![
                column![
                    ui::eyebrow(p.accent, "Vue d’ensemble"),
                    ui::title(p, "Vos synchronisations", 28.0),
                    ui::muted(p, summary),
                ]
                .spacing(6)
                .width(Length::Fill),
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Layers),
                    "Nouveau groupe",
                    Some(Message::NewGroup(None))
                ),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Plus),
                    "Nouveau profil",
                    Some(Message::New(None))
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            row![
                stat(p, Glyph::Layers, p.accent, total.to_string(), "profils"),
                stat(p, Glyph::Clock, p.info, scheduled.to_string(), "planifiés"),
                stat(
                    p,
                    Glyph::Check,
                    p.success,
                    ok.to_string(),
                    "derniers résultats réussis"
                ),
                stat(
                    p,
                    Glyph::Alert,
                    if attention > 0 { p.warning } else { p.faint },
                    attention.to_string(),
                    "à surveiller"
                ),
            ]
            .spacing(12),
        ]
        .spacing(24);
        if total == 0 {
            col = col.push(onboarding(p));
            return page(p, col, 1240.0);
        }
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        for group in self.groups() {
            let members = self.members(&group);
            let label = if group.is_empty() {
                "Sans groupe".to_owned()
            } else {
                group.clone()
            };
            let cards = members
                .iter()
                .map(|pr| self.profile_card(p, pr))
                .chain(std::iter::once(add_card(p, &group)));
            let hot = self.drop_zone.as_deref() == Some(group.as_str());
            let section = column![
                mouse_area(
                    row![
                        ui::eyebrow(p.muted, &label),
                        ui::badge(
                            theme::alpha(p.text, 0.08),
                            p.muted,
                            members.len().to_string()
                        ),
                        ui::fill(),
                        ui::icon_button(
                            p,
                            Glyph::Clock,
                            "Planifier le groupe",
                            Some(Message::ScheduleGroup(group.clone()))
                        ),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                )
                .on_right_press(Message::OpenMenu(MenuTarget::Group(group.clone()))),
                grid(cards)
                    .fluid(420.0)
                    .spacing(12)
                    .height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink)),
            ]
            .spacing(10);
            col = col.push(
                mouse_area(container(section).padding(8).style(theme::drop_zone(
                    *p,
                    dragging,
                    if hot { 1.0 } else { 0.0 },
                )))
                .on_enter(Message::EnterZone(group.clone()))
                .on_exit(Message::LeaveZone(group)),
            );
        }
        page(p, col, 1240.0)
    }

    fn profile_card<'a>(&'a self, p: &Palette, profile: &'a Profile) -> El<'a, Message> {
        let lift = self.lift.get(&profile.id).map_or(0.0, |t| t.at(self.now));
        let selected = self.selected.as_ref() == Some(&profile.id);
        let color = mode_color(p, profile.mode);
        let now = model::now_secs();
        let running = self.running_on(&profile.id);
        let status: El<'a, Message> = if running {
            ui::pill(p.accent, "En cours")
        } else {
            match self.last_run(&profile.id) {
                Some(run) => ui::pill(
                    outcome_color(p, run.outcome),
                    match run.outcome {
                        Outcome::Success => "Réussi",
                        Outcome::Warning => "À vérifier",
                        _ => "Échec",
                    },
                ),
                None => ui::pill(p.faint, "Jamais lancé"),
            }
        };
        let schedule = match (
            self.schedule_of(&profile.id),
            self.schedule_valid(&profile.id),
        ) {
            (Some(s), Some(true)) => (p.info, draft::trigger_label(&s.schedule.trigger)),
            (Some(_), _) => (p.danger, "Planification à renouveler".to_owned()),
            _ => (p.faint, "Manuel".to_owned()),
        };
        let last = self
            .last_run(&profile.id)
            .and_then(|r| r.started)
            .map_or("jamais lancé".to_owned(), |t| model::relative(t, now));
        let body = column![
            row![
                ui::glyph_box(mode_glyph(profile.mode), color, 40.0),
                column![
                    ui::strong(p.text, model::ellipsize(&profile.name, 30), 15.0),
                    ui::caption(p, model::mode_label(profile.mode))
                ]
                .spacing(1)
                .width(Length::Fill),
                status,
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            column![
                ui::mono(
                    p.muted,
                    model::ellipsize(&model::source_of(profile), 40),
                    12.0
                ),
                row![
                    icons::icon(
                        if profile.mode == sync_plan::SyncMode::Bidirectional {
                            Glyph::TwoWay
                        } else {
                            Glyph::ArrowRight
                        },
                        12.0,
                        color
                    ),
                    ui::mono(
                        p.muted,
                        model::ellipsize(&model::destination_of(profile), 38),
                        12.0
                    )
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            ]
            .spacing(4),
            ui::divider(p),
            row![
                icons::icon(Glyph::Clock, 13.0, schedule.0),
                container(ui::colored(schedule.0, schedule.1, 12.0)).width(Length::Fill),
                ui::caption(p, last),
                ui::icon_button(
                    p,
                    Glyph::Eye,
                    "Simuler",
                    (!self.busy()).then(|| Message::Simulate(profile.id.clone()))
                ),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        ]
        .spacing(12);
        let lifted = container(body)
            .padding(Padding {
                top: 16.0 - 2.0 * lift,
                bottom: 12.0 + 2.0 * lift,
                left: 16.0,
                right: 12.0,
            })
            .width(Length::Fill)
            .style(theme::card(*p, lift, selected));
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
        let mut col = column![
            column![ui::title(p, "Activité", 26.0), ui::muted(p, "Suivi en direct des tâches de cette fenêtre et derniers résultats de tous les profils.")].spacing(4),
        ]
        .spacing(20);
        let live: Vec<_> = [&self.job, &self.agent_job].into_iter().flatten().collect();
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
        let mut list = column![
            row![
                ui::heading(p, "Derniers résultats"),
                ui::fill(),
                ui::action(
                    p,
                    Tone::Ghost,
                    Some(Glyph::Refresh),
                    "Actualiser",
                    Some(Message::Refresh)
                )
            ]
            .align_y(Alignment::Center)
        ]
        .spacing(4);
        for (profile, run) in feed.iter().take(15) {
            let c = outcome_color(p, run.outcome);
            list = list.push(
                button(
                    row![
                        ui::glyph_box(
                            if run.outcome == Outcome::Success {
                                Glyph::Check
                            } else {
                                Glyph::Alert
                            },
                            c,
                            32.0
                        ),
                        column![
                            ui::strong(p.text, profile.name.clone(), 14.0),
                            ui::colored(c, run.label.clone(), 12.0)
                        ]
                        .spacing(1)
                        .width(Length::Fill),
                        ui::caption(
                            p,
                            run.started.map_or("—".into(), |s| model::relative(s, now))
                        ),
                        icons::icon(Glyph::ChevronRight, 14.0, p.faint),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([8, 10]))
                .width(Length::Fill)
                .style(theme::row_btn(*p, false))
                .on_press(Message::ShowHistory(profile.id.clone())),
            );
        }
        if feed.is_empty() {
            list = list.push(ui::muted(p, "Aucun résultat enregistré."));
        }
        col = col.push(
            container(list)
                .padding(14)
                .style(theme::card(*p, 0.0, false)),
        );
        page(p, col, 1100.0)
    }

    pub(crate) fn history_page<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let Some(profile) = self.selected.as_ref().and_then(|id| self.profile(id)) else {
            let picks = self.ws.profiles.iter().map(|pr| {
                button(
                    row![
                        icons::icon(mode_glyph(pr.mode), 16.0, mode_color(p, pr.mode)),
                        ui::colored(p.text, pr.name.clone(), 14.0)
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::tile(*p, false, true))
                .on_press(Message::ShowHistory(pr.id.clone()))
                .into()
            });
            return page(
                p,
                column![
                    ui::title(p, "Historique", 26.0),
                    ui::muted(p, "Choisissez un profil pour consulter ses exécutions, y compris les états partiels et interruptions."),
                    grid(picks).fluid(260.0).spacing(10).height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink)),
                ]
                .spacing(16),
                1100.0,
            );
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
            filters,
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
        let Some(page) = &self.history else {
            return container(
                column![controls, ui::muted(p, "Chargement des opérations…")].spacing(10),
            )
            .padding(12)
            .into();
        };
        let mut col = column![].spacing(2);
        for r in &page.rows {
            let (status_color, glyph) = match r.status.as_str() {
                "done" => (p.success, Glyph::Check),
                "no_action" => (p.faint, Glyph::Check),
                "refused" | "cancelled" | "not_started" => (p.warning, Glyph::Alert),
                _ => (p.danger, Glyph::Alert),
            };
            let (path, action) = r.op.as_ref().map_or(("?".into(), "—"), |o| {
                (o.path.clone(), model::action_label(o.action))
            });
            let mut detail = r.detail.clone();
            if let Some(v) = &r.version {
                detail = format!("{detail}  · version préservée {v}");
            }
            col = col.push(
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
                ]
                .spacing(10)
                .padding(Padding::from([5, 10]))
                .align_y(Alignment::Center),
            );
        }
        if page.rows.is_empty() {
            col = col.push(ui::muted(
                p,
                if self.history_filter == sync_orchestrator::HistoryFilter::All
                    && self.history_query.is_empty()
                {
                    "Aucun changement enregistré pour cette exécution."
                } else {
                    "Aucune opération ne correspond à ces filtres."
                },
            ));
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
                          a: Color,
                          b: Color,
                          glyph: Glyph|
         -> El<'a, Message> {
            let selected = self.prefs.theme == choice;
            let swatch = row![
                container(Space::new())
                    .width(Length::Fixed(18.0))
                    .height(Length::Fixed(54.0))
                    .style(theme::panel(a)),
                container(
                    column![
                        container(Space::new())
                            .width(Length::Fixed(60.0))
                            .height(Length::Fixed(8.0))
                            .style(theme::bar(theme::mix(b, a, 0.35))),
                        container(Space::new())
                            .width(Length::Fixed(40.0))
                            .height(Length::Fixed(8.0))
                            .style(theme::bar(theme::mix(b, a, 0.55))),
                        container(Space::new())
                            .width(Length::Fixed(28.0))
                            .height(Length::Fixed(8.0))
                            .style(theme::bar(p.accent)),
                    ]
                    .spacing(5)
                )
                .padding(8)
                .width(Length::Fill)
                .height(Length::Fixed(54.0))
                .style(theme::panel(b)),
            ];
            button(
                column![
                    container(swatch)
                        .style(move |_| iced::widget::container::Style {
                            border: iced::Border {
                                color: theme::alpha(Color::BLACK, 0.15),
                                width: 1.0,
                                radius: 6.0.into()
                            },
                            ..iced::widget::container::Style::default()
                        })
                        .clip(true),
                    row![
                        icons::icon(glyph, 15.0, if selected { p.accent } else { p.muted }),
                        ui::strong(p.text, label, 14.0)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                ]
                .spacing(10),
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::tile(*p, selected, true))
            .on_press(Message::Theme(choice))
            .into()
        };
        let d = theme::Palette::DARK;
        let l = theme::Palette::LIGHT;
        let section = |title: &'static str, body: El<'a, Message>| -> El<'a, Message> {
            column![
                ui::eyebrow(p.muted, title),
                container(body)
                    .padding(18)
                    .width(Length::Fill)
                    .style(theme::card(*p, 0.0, false))
            ]
            .spacing(10)
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
        let state = self
            .backend
            .as_ref()
            .map(|a| model::pretty_path(&a.state_directory().display().to_string()))
            .unwrap_or_default();
        let col = column![
            column![
                ui::title(p, "Réglages", 26.0),
                ui::muted(
                    p,
                    "Apparence, fenêtre, démarrage et informations sur cette version."
                )
            ]
            .spacing(4),
            section(
                "Apparence",
                column![
                    row![
                        theme_tile(
                            ThemeChoice::System,
                            "Système",
                            if self.system_dark { d.rail } else { l.rail },
                            if self.system_dark { d.bg } else { l.bg },
                            Glyph::Monitor
                        ),
                        theme_tile(ThemeChoice::Dark, "Sombre", d.rail, d.bg, Glyph::Moon),
                        theme_tile(ThemeChoice::Light, "Clair", l.rail, l.bg, Glyph::Sun),
                    ]
                    .spacing(12),
                    row![
                        column![
                            ui::strong(p.text, "Affichage compact", 14.0),
                            ui::caption(p, "Lignes plus serrées pour les longues listes.")
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        toggler(self.ws.preferences.compact)
                            .on_toggle(Message::Compact)
                            .size(22)
                            .style(theme::switch(*p, false)),
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(18)
                .into()
            ),
            section("Fenêtre", window_section),
            section("Démarrage", startup),
            section(
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
                    ui::action(
                        p,
                        Tone::Secondary,
                        None,
                        "Afficher",
                        Some(Message::ShowShortcuts)
                    ),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .into()
            ),
            section("Garanties d’exécution", guarantees.into()),
            section(
                "Données",
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
                    ui::action(
                        p,
                        Tone::Danger,
                        Some(Glyph::Trash),
                        "Supprimer…",
                        (!self.busy() && self.agent_job.is_none())
                            .then_some(Message::Purge(crate::purge_view::PurgeMsg::Ask))
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .into()
            ),
            section(
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
                    row![
                        ui::caption(p, format!("État local : {}", model::ellipsize(&state, 70))),
                        ui::icon_button(
                            p,
                            Glyph::Folder,
                            "Ouvrir le dossier des données",
                            Some(Message::OpenStateFolder)
                        ),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                ]
                .spacing(10)
                .into()
            ),
            ui::action(
                p,
                Tone::Ghost,
                Some(Glyph::ChevronLeft),
                "Retour aux profils",
                Some(Message::Nav(Nav::Profiles))
            ),
        ]
        .spacing(22);
        page(p, col, 900.0)
    }
}

fn add_card<'a>(p: &Palette, group: &str) -> El<'a, Message> {
    button(
        container(
            column![
                icons::icon(Glyph::Plus, 22.0, p.faint),
                ui::colored(p.muted, "Nouveau profil", 13.0)
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .center_x(Length::Fill)
        .center_y(Length::Fixed(150.0)),
    )
    .padding(0)
    .width(Length::Fill)
    .style(theme::tile(*p, false, true))
    .on_press(Message::New(Some(group.to_owned())))
    .into()
}

fn onboarding<'a>(p: &Palette) -> El<'a, Message> {
    let step = |n: &'static str,
                glyph: Glyph,
                title: &'static str,
                body: &'static str,
                color: Color|
     -> El<'a, Message> {
        column![
            row![
                ui::badge(color, p.on_accent, n),
                ui::glyph_box(glyph, color, 44.0)
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            ui::strong(p.text, title, 15.0),
            ui::muted(p, body),
        ]
        .spacing(10)
        .width(Length::Fill)
        .into()
    };
    container(
        column![
            ui::title(p, "Créez votre premier profil", 22.0),
            ui::muted(
                p,
                "Un assistant vous guide pas à pas : aucun fichier de configuration à éditer."
            ),
            row![
                step(
                    "1",
                    Glyph::Layers,
                    "Choisir",
                    "Un mode, une source et une destination, locales ou réseau.",
                    p.accent
                ),
                step(
                    "2",
                    Glyph::Eye,
                    "Simuler",
                    "Chaque opération prévue est affichée avant toute écriture.",
                    p.info
                ),
                step(
                    "3",
                    Glyph::Bolt,
                    "Exécuter",
                    "Vous confirmez le plan exact ; l’historique garde la trace.",
                    p.success
                ),
            ]
            .spacing(24),
            ui::action(
                p,
                Tone::Primary,
                Some(Glyph::Plus),
                "Démarrer l’assistant",
                Some(Message::New(None))
            ),
        ]
        .spacing(20),
    )
    .padding(28)
    .width(Length::Fill)
    .style(theme::outline(*p, p.accent, 0.5))
    .into()
}
