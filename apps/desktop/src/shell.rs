//! Window layout: navigation rail, profile sidebar (groups, drag and drop),
//! content area and the overlay layers (assistant, dialogs, menus, toasts).

use crate::app::{App, JobKind, MenuTarget, Message, Nav, SEARCH_ID};
use crate::charts::{self, Mark};
use crate::icons::{self, Glyph};
use crate::model::{self, Outcome};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{
    Space, button, column, container, mouse_area, pin, row, scrollable, stack, text_input, tooltip,
};
use iced::{Alignment, Color, Length, Padding};
use sync_plan::SyncMode;
use sync_profiles::Profile;

const RAIL_ITEM: f32 = 56.0;

pub(crate) fn mode_glyph(mode: SyncMode) -> Glyph {
    match mode {
        SyncMode::Backup => Glyph::Backup,
        SyncMode::Mirror => Glyph::Mirror,
        _ => Glyph::TwoWay,
    }
}

pub(crate) fn mode_color(p: &Palette, mode: SyncMode) -> Color {
    match mode {
        SyncMode::Backup => p.success,
        SyncMode::Mirror => p.accent,
        _ => p.warning,
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
    pub(crate) fn view(&self) -> El<'_, Message> {
        let p = self.palette();
        let Some(_) = &self.backend else {
            return self.splash(&p);
        };
        let compact = self.ws.preferences.compact;
        let content = self.content(&p);
        let mut base = row![self.rail(&p)];
        if self.nav != Nav::Settings {
            base = base.push(self.sidebar(&p, compact));
        }
        let base = container(base.push(content))
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

    fn rail(&self, p: &Palette) -> El<'_, Message> {
        let items = [
            (Nav::Profiles, Glyph::Grid, "Profils  ·  Ctrl+1"),
            (Nav::Activity, Glyph::Pulse, "Activité  ·  Ctrl+2"),
            (Nav::History, Glyph::History, "Historique  ·  Ctrl+3"),
            (Nav::Schedules, Glyph::Clock, "Planification  ·  Ctrl+4"),
        ];
        let turn = self
            .now
            .saturating_duration_since(self.started)
            .as_secs_f32();
        let mut col = column![ui::vspace(6.0)].align_x(Alignment::Center);
        for (nav, glyph, hint) in items {
            let active = self.nav == nav;
            let mut icon = if nav == Nav::Activity && self.job.is_some() {
                icons::spinning(
                    Glyph::Sync,
                    22.0,
                    if active { p.on_accent } else { p.accent },
                    turn,
                )
            } else {
                icons::icon(glyph, 22.0, if active { p.on_accent } else { p.text })
            };
            if nav == Nav::Schedules {
                let invalid = self
                    .ws
                    .schedules
                    .iter()
                    .filter(|s| {
                        self.schedule_valid(&s.schedule.approved.id) != Some(true)
                            || s.running_slot.is_some()
                    })
                    .count();
                if invalid > 0 {
                    icon = stack![
                        icon,
                        container(ui::dot(p.danger, 9.0))
                            .width(Length::Fixed(22.0))
                            .align_right(Length::Fixed(22.0))
                    ]
                    .into();
                }
            }
            col = col.push(
                container(ui::with_tip(
                    p,
                    button(container(icon).center(Length::Fixed(48.0)))
                        .padding(0)
                        .width(Length::Fixed(48.0))
                        .height(Length::Fixed(48.0))
                        .style(theme::rail_btn(*p, active))
                        .on_press(Message::Nav(nav)),
                    hint,
                    tooltip::Position::Right,
                ))
                .height(Length::Fixed(RAIL_ITEM))
                .center_y(Length::Fixed(RAIL_ITEM)),
            );
        }
        let light = p.is_light();
        col = col
            .push(Space::new().height(Length::Fill))
            .push(ui::with_tip(
                p,
                button(
                    container(icons::icon(
                        if light { Glyph::Moon } else { Glyph::Sun },
                        20.0,
                        p.text,
                    ))
                    .center(Length::Fixed(48.0)),
                )
                .padding(0)
                .style(theme::rail_btn(*p, false))
                .on_press(Message::Theme(if light {
                    crate::ui_prefs::ThemeChoice::Dark
                } else {
                    crate::ui_prefs::ThemeChoice::Light
                })),
                if light {
                    "Thème sombre  ·  Ctrl+T"
                } else {
                    "Thème clair  ·  Ctrl+T"
                },
                tooltip::Position::Right,
            ))
            .push(ui::vspace(8.0))
            .push(ui::with_tip(
                p,
                button(
                    container(icons::icon(
                        Glyph::Sliders,
                        20.0,
                        if self.nav == Nav::Settings {
                            p.on_accent
                        } else {
                            p.text
                        },
                    ))
                    .center(Length::Fixed(48.0)),
                )
                .padding(0)
                .style(theme::rail_btn(*p, self.nav == Nav::Settings))
                .on_press(Message::Nav(Nav::Settings)),
                "Réglages  ·  Ctrl+,",
                tooltip::Position::Right,
            ))
            .push(ui::vspace(14.0));
        // Discord-like selection pill sliding along the rail.
        let index = self.nav_t.at(self.now);
        let pill_y = if self.nav == Nav::Settings {
            -100.0
        } else {
            // 6 px top gap, then the pill centered in its 56 px item.
            6.0 + index.clamp(0.0, 3.5) * RAIL_ITEM + 12.0
        };
        let indicator = pin(container(Space::new())
            .width(Length::Fixed(4.0))
            .height(Length::Fixed(32.0))
            .style(theme::bar(p.text)))
        .x(0.0)
        .y(pill_y);
        container(stack![
            col.width(Length::Fixed(72.0)).height(Length::Fill),
            indicator
        ])
        .width(Length::Fixed(72.0))
        .height(Length::Fill)
        .padding(Padding {
            top: 6.0,
            ..Padding::ZERO
        })
        .style(theme::panel(p.rail))
        .into()
    }

    fn sidebar(&self, p: &Palette, compact: bool) -> El<'_, Message> {
        let search = container(
            container(
                row![
                    icons::icon(Glyph::Search, 14.0, p.faint),
                    text_input("Rechercher un profil", &self.search)
                        .id(SEARCH_ID)
                        .on_input(Message::Search)
                        .padding(Padding::from([7, 4]))
                        .size(13)
                        .style(theme::search(*p)),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding(Padding {
                left: 10.0,
                right: 4.0,
                top: 0.0,
                bottom: 0.0,
            })
            .style(theme::panel(if p.is_light() {
                p.surface_hi
            } else {
                p.rail
            })),
        )
        .padding(Padding::from([12, 10]));
        let overview_selected = self.selected.is_none() && self.nav == Nav::Profiles;
        let overview = button(
            row![
                icons::icon(
                    Glyph::Grid,
                    18.0,
                    if overview_selected { p.text } else { p.muted }
                ),
                ui::colored(
                    if overview_selected { p.text } else { p.muted },
                    "Vue d’ensemble",
                    14.0
                )
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([8, 10]))
        .width(Length::Fill)
        .style(theme::row_btn(*p, overview_selected))
        .on_press(Message::Select(None));
        let dragging = self.drag.as_ref().is_some_and(|d| d.active);
        let mut list = column![overview, ui::vspace(8.0)].spacing(2);
        let groups = self.groups();
        for group in groups {
            list = list.push(self.group_block(p, group, dragging, compact));
        }
        if dragging {
            list = list.push(
                mouse_area(
                    container(
                        row![
                            icons::icon(Glyph::Plus, 16.0, p.accent),
                            ui::colored(p.accent, "Déposer pour créer un groupe", 13.0)
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
        if self.ws.profiles.is_empty() {
            list = list.push(
                container(
                    column![
                        icons::bold(Glyph::Folder, 36.0, p.faint),
                        ui::muted(p, "Aucun profil pour l’instant"),
                    ]
                    .spacing(8)
                    .align_x(Alignment::Center),
                )
                .padding(24)
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        }
        let scroll = mouse_area(
            scrollable(container(list).padding(Padding {
                top: 0.0,
                bottom: 12.0,
                left: 8.0,
                right: 10.0,
            }))
            .style(theme::scroll(*p))
            .height(Length::Fill),
        )
        .on_right_press(Message::OpenMenu(MenuTarget::Background));
        let footer = column![
            self.job_card(p),
            container(
                button(
                    row![
                        icons::icon(Glyph::Plus, 16.0, p.on_accent),
                        ui::strong(p.on_accent, "Nouveau profil", 14.0)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([9, 14]))
                .width(Length::Fill)
                .style(theme::btn(*p, Tone::Primary))
                .on_press(Message::New(None)),
            )
            .padding(Padding::from([10, 10]))
            .width(Length::Fill)
        ];
        container(column![search, ui::divider(p), scroll, footer])
            .width(Length::Fixed(272.0))
            .height(Length::Fill)
            .style(theme::panel(p.sidebar))
            .into()
    }

    fn group_block<'a>(
        &'a self,
        p: &Palette,
        group: String,
        dragging: bool,
        compact: bool,
    ) -> El<'a, Message> {
        let collapsed = self.prefs.collapsed.contains(&group);
        let members = self.members(&group);
        let label = if group.is_empty() {
            "Sans groupe".to_owned()
        } else {
            group.clone()
        };
        let header = mouse_area(
            button(
                row![
                    icons::icon(
                        if collapsed {
                            Glyph::ChevronRight
                        } else {
                            Glyph::ChevronDown
                        },
                        12.0,
                        p.faint
                    ),
                    ui::eyebrow(p.muted, &label),
                    ui::caption(p, members.len().to_string()),
                    ui::fill(),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([6, 6]))
            .width(Length::Fill)
            .style(theme::btn(*p, Tone::Ghost))
            .on_press(Message::ToggleGroup(group.to_owned())),
        )
        .on_right_press(Message::OpenMenu(MenuTarget::Group(group.to_owned())));
        let header = row![
            header,
            ui::icon_button(
                p,
                Glyph::Plus,
                "Nouveau profil dans ce groupe",
                Some(Message::New(Some(group.to_owned())))
            ),
        ]
        .align_y(Alignment::Center);
        let mut col = column![header].spacing(1);
        if !collapsed {
            for profile in members {
                col = col.push(self.profile_row(p, profile, compact));
            }
        }
        let hot = self.drop_zone.as_deref() == Some(group.as_str());
        mouse_area(
            container(col)
                .padding(Padding::from([2, 2]))
                .style(theme::drop_zone(*p, dragging, if hot { 1.0 } else { 0.0 })),
        )
        .on_enter(Message::EnterZone(group.clone()))
        .on_exit(Message::LeaveZone(group))
        .into()
    }

    fn profile_row<'a>(
        &'a self,
        p: &Palette,
        profile: &'a Profile,
        compact: bool,
    ) -> El<'a, Message> {
        let selected = self.selected.as_ref() == Some(&profile.id);
        let lift = self.lift.get(&profile.id).map_or(0.0, |t| t.at(self.now));
        let dragged = self
            .drag
            .as_ref()
            .is_some_and(|d| d.active && d.profile == profile.id);
        let running = self.job.as_ref().is_some_and(|j| j.profile == profile.id);
        let color = mode_color(p, profile.mode);
        let mut right = row![].spacing(6).align_y(Alignment::Center);
        if self.schedule_of(&profile.id).is_some() {
            let valid = self.schedule_valid(&profile.id) == Some(true);
            right = right.push(icons::icon(
                Glyph::Clock,
                13.0,
                if valid { p.faint } else { p.danger },
            ));
        }
        if running {
            let turn = self
                .now
                .saturating_duration_since(self.started)
                .as_secs_f32();
            right = right.push(icons::spinning(Glyph::Sync, 14.0, p.accent, turn));
        } else if let Some(run) = self.last_run(&profile.id) {
            right = right.push(ui::dot(outcome_color(p, run.outcome), 8.0));
        }
        let fg = if selected {
            p.text
        } else {
            theme::mix(p.muted, p.text, lift)
        };
        let bg = if selected {
            theme::alpha(p.accent, if p.is_light() { 0.12 } else { 0.22 })
        } else {
            theme::alpha(p.text, 0.06 * lift)
        };
        let item = container(
            row![
                icons::icon(
                    mode_glyph(profile.mode),
                    16.0,
                    if dragged { p.faint } else { color }
                ),
                ui::colored(
                    if dragged { p.faint } else { fg },
                    model::ellipsize(&profile.name, 26),
                    14.0
                ),
                ui::fill(),
                right,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([if compact { 5 } else { 8 }, 10]))
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(bg.into()),
            border: iced::border::rounded(theme::RADIUS_SM),
            ..container::Style::default()
        });
        mouse_area(item)
            .on_press(Message::Press(profile.id.clone()))
            .on_double_click(Message::Edit(profile.id.clone()))
            .on_right_press(Message::OpenMenu(MenuTarget::Profile(profile.id.clone())))
            .on_enter(Message::Hover(Some(profile.id.clone())))
            .on_exit(Message::Hover(None))
            .interaction(if self.drag.as_ref().is_some_and(|d| d.active) {
                iced::mouse::Interaction::Grabbing
            } else {
                iced::mouse::Interaction::Pointer
            })
            .into()
    }

    /// Discord « voice connected »-style live panel at the bottom of the sidebar.
    fn job_card(&self, p: &Palette) -> El<'_, Message> {
        let Some(job) = &self.job else {
            return Space::new().into();
        };
        let turn = self
            .now
            .saturating_duration_since(job.started)
            .as_secs_f32();
        let (label, color) = match job.kind {
            JobKind::Preview => ("Simulation en cours", p.accent),
            JobKind::Execute => ("Exécution réelle", p.success),
        };
        let s = &job.snapshot;
        let detail = match job.kind {
            JobKind::Preview if s.hash_target > 0 => format!(
                "{} / {} empreintes",
                model::count(s.entries_hashed),
                model::count(s.hash_target)
            ),
            JobKind::Preview => format!("{} entrées analysées", model::count(s.entries_scanned)),
            // Block-level bytes, as the main progress card.
            JobKind::Execute => format!(
                "{} opérations · {} copiés",
                model::count(s.entries_done),
                model::bytes(s.bytes_transferred)
            ),
        };
        let progress = match job.kind {
            JobKind::Preview => (s.hash_target > 0)
                .then(|| (s.entries_hashed as f32 / s.hash_target as f32).min(1.0)),
            JobKind::Execute => job
                .planned_bytes
                .filter(|b| *b > 0)
                .map(|b| (s.bytes_transferred as f32 / b as f32).min(1.0))
                .or_else(|| {
                    job.planned
                        .filter(|n| *n > 0)
                        .map(|n| (s.entries_done as f32 / n as f32).min(1.0))
                }),
        };
        container(
            row![
                charts::ring(
                    charts::Ring {
                        progress,
                        phase: turn * 0.8,
                        color,
                        track: theme::alpha(color, 0.2),
                        width: 3.0
                    },
                    30.0
                ),
                column![
                    ui::strong(color, label, 13.0),
                    ui::caption(p, model::ellipsize(&job.name, 24)),
                    ui::caption(p, detail),
                ]
                .spacing(1)
                .width(Length::Fill),
                ui::icon_button(
                    p,
                    Glyph::Stop,
                    if job.cancel_requested {
                        "Annulation demandée…"
                    } else {
                        "Annuler proprement"
                    },
                    (!job.cancel_requested).then_some(Message::Cancel)
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10, 12]))
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
