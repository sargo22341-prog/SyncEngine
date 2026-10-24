//! Step-by-step profile assistant. No JSON anywhere: every engine setting is
//! a guided choice, validated before moving on and again by the facade.

use crate::anim::{Ease, Tween};
use crate::app::Message;
use crate::backend::{self, ScheduleChange};
use crate::draft::{Field, FilterList, PRESETS, ProfileDraft, Provider, TriggerKind};
use crate::icons::{self, Glyph};
use crate::model;
use crate::native;
use crate::schedules::{self, ScheduleMsg};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{
    Space, button, checkbox, column, container, mouse_area, opaque, row, scrollable, stack,
    text_input, toggler,
};
use iced::{Alignment, Length, Padding, Task};
use std::path::PathBuf;
use std::time::Instant;
use sync_plan::SyncMode;
use sync_profiles::{CompareMethod, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Mode,
    Source,
    Destination,
    Filters,
    Safety,
    Schedule,
    Summary,
}

impl Step {
    pub(crate) const ALL: [Self; 7] = [
        Self::Mode,
        Self::Source,
        Self::Destination,
        Self::Filters,
        Self::Safety,
        Self::Schedule,
        Self::Summary,
    ];
    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    fn title(self) -> &'static str {
        match self {
            Self::Mode => "Que voulez-vous faire ?",
            Self::Source => "D’où viennent les fichiers ?",
            Self::Destination => "Où doivent-ils aller ?",
            Self::Filters => "Que faut-il ignorer ?",
            Self::Safety => "Protections et suppressions",
            Self::Schedule => "Quand synchroniser ?",
            Self::Summary => "Dernière vérification",
        }
    }
    fn short(self) -> &'static str {
        match self {
            Self::Mode => "Mode",
            Self::Source => "Source",
            Self::Destination => "Destination",
            Self::Filters => "Filtres",
            Self::Safety => "Sécurité",
            Self::Schedule => "Planification",
            Self::Summary => "Récapitulatif",
        }
    }
    fn subtitle(self) -> &'static str {
        match self {
            Self::Mode => {
                "Chaque mode a sa propre stratégie, toujours prévisualisée avant écriture."
            }
            Self::Source => {
                "Les fichiers de référence. Ils ne sont jamais modifiés par une sauvegarde ou un miroir."
            }
            Self::Destination => {
                "La copie à maintenir. Elle doit être distincte de la source et non imbriquée."
            }
            Self::Filters => {
                "Les mêmes règles s’appliquent aux deux côtés, en simulation comme en exécution."
            }
            Self::Safety => "Les garde-fous restent actifs quel que soit votre choix.",
            Self::Schedule => "Facultatif : l’agent peut lancer ce profil sans ouvrir la fenêtre.",
            Self::Summary => "Nommez le profil, rangez-le dans un groupe, puis enregistrez.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Source,
    Destination,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flag {
    Plaintext,
    Exclusive,
}

#[derive(Debug, Clone)]
pub(crate) enum WizardMsg {
    Mode(SyncMode),
    Provider(Side, Provider),
    Field(Side, Field, String),
    Flag(Side, Flag, bool),
    Browse(Side, Field),
    Browsed(Side, Field, Option<PathBuf>),
    Preset(usize),
    PatternInput(FilterList, String),
    AddPattern(FilterList),
    RemovePattern(FilterList, usize),
    CaseInsensitive(bool),
    AllowDelete(bool),
    Compare(CompareMethod),
    Schedule(ScheduleMsg),
    Name(String),
    Group(String),
    Next,
    Back,
    GoTo(Step),
    Close,
    Save(bool),
}

pub(crate) enum Outcome {
    None,
    Task(Task<Message>),
    Save {
        profile: Box<Profile>,
        schedule: ScheduleChange,
        simulate: bool,
    },
}

pub(crate) struct Wizard {
    pub(crate) draft: ProfileDraft,
    pub(crate) step: Step,
    pub(crate) error: Option<String>,
    pub(crate) saving: bool,
    pub(crate) closing: bool,
    visited: usize,
    open_t: Tween,
    slide_t: Tween,
    shake_t: Tween,
    forward: bool,
    inputs: [String; 3],
    groups: Vec<String>,
}

fn list_index(list: FilterList) -> usize {
    match list {
        FilterList::ExcludeFiles => 0,
        FilterList::ExcludeDirectories => 1,
        FilterList::IncludeFiles => 2,
    }
}

impl Wizard {
    pub(crate) fn open(draft: ProfileDraft, groups: Vec<String>, now: Instant) -> Self {
        let editing = draft.existing.is_some();
        let mut open_t = Tween::new(0.0, 320, Ease::OutCubic);
        open_t.go(1.0, now);
        let mut slide_t = Tween::new(0.0, 340, Ease::OutCubic);
        slide_t.go(1.0, now);
        Self {
            draft,
            step: Step::Mode,
            error: None,
            saving: false,
            closing: false,
            visited: if editing { Step::ALL.len() - 1 } else { 0 },
            open_t,
            slide_t,
            shake_t: Tween::new(1.0, 420, Ease::OutCubic),
            forward: true,
            inputs: Default::default(),
            groups: groups.into_iter().filter(|g| !g.is_empty()).collect(),
        }
    }

    pub(crate) fn animating(&self, now: Instant) -> bool {
        self.open_t.running(now) || self.slide_t.running(now) || self.shake_t.running(now)
    }

    pub(crate) fn close(&mut self, now: Instant) {
        if !self.saving {
            self.closing = true;
            self.open_t.go(0.0, now);
        }
    }

    fn side(&mut self, side: Side) -> &mut crate::draft::EndpointDraft {
        match side {
            Side::Source => &mut self.draft.source,
            Side::Destination => &mut self.draft.destination,
        }
    }

    fn step_error(&self, step: Step) -> Option<String> {
        let d = &self.draft;
        match step {
            Step::Mode => d.mode.is_none().then(|| "Choisissez un mode pour continuer".to_owned()),
            Step::Source => d.source.build().err(),
            Step::Destination => {
                if d.destination.provider == Some(Provider::Onedrive) {
                    return Some("OneDrive ne peut être qu’une source".into());
                }
                d.destination.build().err()
            }
            Step::Filters | Step::Safety => None,
            Step::Schedule => match d.schedule.trigger() {
                Err(e) => Some(e),
                Ok(Some(_)) if !d.schedule.approve_writes => {
                    Some("Cochez l’autorisation des écritures automatiques, ou choisissez « Manuellement »".into())
                }
                Ok(_) => None,
            },
            Step::Summary => d.build().err(),
        }
    }

    fn goto(&mut self, step: Step, now: Instant) {
        if step == self.step {
            return;
        }
        self.forward = step.index() > self.step.index();
        self.step = step;
        self.visited = self.visited.max(step.index());
        self.error = None;
        self.slide_t.replay(0.0, 1.0, now);
        if step == Step::Summary && self.draft.name.trim().is_empty() {
            self.draft.name = self.draft.suggested_name();
        }
    }

    fn bidirectional(&self) -> bool {
        self.draft.mode == Some(SyncMode::Bidirectional)
    }

    pub(crate) fn update(&mut self, msg: WizardMsg, now: Instant) -> Outcome {
        if !matches!(msg, WizardMsg::Next | WizardMsg::Save(_)) {
            self.error = None;
        }
        match msg {
            WizardMsg::Mode(mode) => {
                self.draft.mode = Some(mode);
                if mode == SyncMode::Backup {
                    self.draft.allow_delete = false;
                }
                if mode == SyncMode::Bidirectional {
                    for side in [&mut self.draft.source, &mut self.draft.destination] {
                        if side.provider.is_some_and(|p| p != Provider::Local) {
                            side.select(Provider::Local);
                        }
                    }
                }
            }
            WizardMsg::Provider(side, provider) => self.side(side).select(provider),
            WizardMsg::Field(side, field, value) => self.side(side).set(field, value),
            WizardMsg::Flag(side, flag, value) => match flag {
                Flag::Plaintext => self.side(side).acknowledge_plaintext = value,
                Flag::Exclusive => self.side(side).exclusive_writers = value,
            },
            WizardMsg::Browse(side, field) => {
                let start = PathBuf::from(self.side(side).path_value());
                let task = backend::blocking(move || {
                    Ok(match field {
                        Field::KnownHosts => native::pick_file("Choisir le fichier known_hosts"),
                        Field::RootCertificate => {
                            native::pick_file("Choisir le certificat de l’autorité")
                        }
                        _ => native::pick_folder("Choisir un dossier SyncEngine", Some(&start)),
                    })
                });
                return Outcome::Task(Task::perform(task, move |r| {
                    Message::Wizard(WizardMsg::Browsed(side, field, r.ok().flatten()))
                }));
            }
            WizardMsg::Browsed(side, field, path) => {
                if let Some(path) = path.and_then(|p| p.to_str().map(str::to_owned)) {
                    self.side(side).set(field, path);
                }
            }
            WizardMsg::Preset(i) => {
                if let Some(preset) = PRESETS.get(i) {
                    self.draft.filters.toggle_preset(preset);
                }
            }
            WizardMsg::PatternInput(list, v) => self.inputs[list_index(list)] = v,
            WizardMsg::AddPattern(list) => {
                let value = std::mem::take(&mut self.inputs[list_index(list)]);
                self.draft.filters.add(list, &value);
            }
            WizardMsg::RemovePattern(list, i) => self.draft.filters.remove(list, i),
            WizardMsg::CaseInsensitive(v) => self.draft.filters.case_insensitive = v,
            WizardMsg::AllowDelete(v) => {
                self.draft.allow_delete = v && self.draft.deletions_possible();
                if !self.draft.allow_delete {
                    self.draft.schedule.approve_deletions = false;
                }
            }
            WizardMsg::Compare(method) => self.draft.compare = method,
            WizardMsg::Schedule(m) => schedules::apply(&mut self.draft.schedule, m),
            WizardMsg::Name(v) => self.draft.name = v.chars().take(200).collect(),
            WizardMsg::Group(v) => self.draft.group = v.chars().take(200).collect(),
            WizardMsg::Next => {
                if let Some(e) = self.step_error(self.step) {
                    self.error = Some(e);
                    self.shake_t.replay(0.0, 1.0, now);
                } else if let Some(next) = Step::ALL.get(self.step.index() + 1) {
                    self.goto(*next, now);
                }
            }
            WizardMsg::Back => {
                if let Some(prev) = self
                    .step
                    .index()
                    .checked_sub(1)
                    .and_then(|i| Step::ALL.get(i))
                {
                    self.goto(*prev, now);
                }
            }
            WizardMsg::GoTo(step) => {
                // Free navigation backward; forward only through valid steps.
                let blocked = Step::ALL[..step.index()]
                    .iter()
                    .find_map(|s| self.step_error(*s).map(|e| (*s, e)));
                match blocked {
                    Some((s, e)) if step.index() > self.step.index() => {
                        self.goto(s, now);
                        self.error = Some(e);
                        self.shake_t.replay(0.0, 1.0, now);
                    }
                    _ if step.index() <= self.visited => self.goto(step, now),
                    _ => {}
                }
            }
            WizardMsg::Close => self.close(now),
            WizardMsg::Save(simulate) => {
                if let Some((s, e)) = Step::ALL
                    .iter()
                    .find_map(|s| self.step_error(*s).map(|e| (*s, e)))
                {
                    self.goto(s, now);
                    self.error = Some(e);
                    self.shake_t.replay(0.0, 1.0, now);
                    return Outcome::None;
                }
                let profile = match self.draft.build() {
                    Ok(p) => p,
                    Err(e) => {
                        self.error = Some(e);
                        return Outcome::None;
                    }
                };
                let s = &self.draft.schedule;
                let schedule = match s.trigger() {
                    Ok(Some(trigger)) => ScheduleChange::Set {
                        trigger,
                        missed: s.missed(),
                        approve_writes: s.approve_writes,
                        approve_deletions: s.approve_deletions && profile.allow_delete,
                        show_window: s.show_window,
                    },
                    Ok(None) if self.draft.had_schedule => ScheduleChange::Remove,
                    _ => ScheduleChange::Keep,
                };
                self.saving = true;
                return Outcome::Save {
                    profile: Box::new(profile),
                    schedule,
                    simulate,
                };
            }
        }
        Outcome::None
    }

    pub(crate) fn view<'a>(
        &'a self,
        p: &Palette,
        now: Instant,
        providers: &[&'static str],
        compact: bool,
    ) -> El<'a, Message> {
        let open = self.open_t.at(now);
        let panel_p = p.faded(open);
        let slide = self.slide_t.at(now);
        let content_p = panel_p.faded(slide);
        let shift = 36.0 * (1.0 - slide);
        let body = container(self.step_view(&content_p, providers, compact)).padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: if self.forward { shift } else { 0.0 },
            right: if self.forward { 0.0 } else { shift },
        });
        let step_no = self.step.index() + 1;
        let header = column![
            ui::eyebrow(
                content_p.accent,
                &format!("Étape {step_no} sur {}", Step::ALL.len())
            ),
            ui::title(&content_p, self.step.title(), 26.0),
            ui::muted(&content_p, self.step.subtitle()),
        ]
        .spacing(6);
        let right = column![
            container(header).padding(Padding {
                top: 30.0,
                bottom: 18.0,
                left: 36.0 + shift * 0.4,
                right: 36.0
            }),
            scrollable(
                container(body)
                    .padding(Padding::from([4, 36]))
                    .width(Length::Fill)
            )
            .style(theme::scroll(panel_p))
            .height(Length::Fill),
            self.footer(&panel_p, now),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        let panel = container(row![self.stepper(&panel_p), right])
            .width(Length::Fill)
            .height(Length::Fill)
            .max_width(1120.0)
            .max_height(780.0)
            .style(theme::dialog(panel_p))
            .clip(true);
        let lift = 28.0 * (1.0 - open);
        let layer = container(panel)
            .padding(Padding {
                top: 32.0 + lift,
                bottom: (32.0 - lift).max(0.0),
                left: 40.0,
                right: 40.0,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill);
        stack![
            opaque(
                mouse_area(
                    container(Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(theme::scrim(*p, open))
                )
                .on_press(Message::Noop)
            ),
            opaque(layer),
        ]
        .into()
    }

    fn stepper<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let mut col = column![
            row![
                icons::icon(Glyph::Logo, 22.0, p.accent),
                ui::strong(
                    p.text,
                    if self.draft.existing.is_some() {
                        "Modifier le profil"
                    } else {
                        "Nouveau profil"
                    },
                    15.0
                )
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            ui::vspace(26.0),
        ]
        .spacing(0);
        let current = self.step.index();
        for (i, step) in Step::ALL.iter().enumerate() {
            let done = i < current
                || (i <= self.visited && i != current && self.step_error(*step).is_none());
            let active = i == current;
            let reachable = i <= self.visited;
            let color = if active {
                p.accent
            } else if done {
                p.success
            } else {
                p.faint
            };
            let marker: El<'a, Message> = container(if done && !active {
                icons::icon(Glyph::Check, 13.0, p.on_accent)
            } else {
                ui::strong(
                    if active { p.on_accent } else { p.muted },
                    (i + 1).to_string(),
                    12.0,
                )
            })
            .width(Length::Fixed(26.0))
            .height(Length::Fixed(26.0))
            .center_x(Length::Fixed(26.0))
            .center_y(Length::Fixed(26.0))
            .style(theme::pill(
                if active || done {
                    color
                } else {
                    theme::alpha(p.text, 0.08)
                },
                p.on_accent,
            ))
            .into();
            col = col.push(
                button(
                    row![
                        marker,
                        ui::colored(
                            if active {
                                p.text
                            } else if reachable {
                                p.muted
                            } else {
                                p.faint
                            },
                            step.short(),
                            14.0
                        )
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([6, 8]))
                .width(Length::Fill)
                .style(theme::row_btn(*p, active))
                .on_press_maybe(
                    (reachable || i == current + 1)
                        .then_some(Message::Wizard(WizardMsg::GoTo(*step))),
                ),
            );
            if i + 1 < Step::ALL.len() {
                col = col.push(
                    container(
                        container(Space::new())
                            .width(Length::Fixed(2.0))
                            .height(Length::Fixed(14.0))
                            .style(theme::bar(if i < current {
                                p.success
                            } else {
                                theme::alpha(p.text, 0.1)
                            })),
                    )
                    .padding(Padding {
                        left: 20.0,
                        ..Padding::ZERO
                    }),
                );
            }
        }
        col = col.push(Space::new().height(Length::Fill)).push(
            row![ui::kbd(p, "Échap"), ui::caption(p, "pour fermer")]
                .spacing(8)
                .align_y(Alignment::Center),
        );
        container(col)
            .padding(Padding::from([28, 20]))
            .width(Length::Fixed(240.0))
            .height(Length::Fill)
            .style(theme::panel(p.sidebar))
            .into()
    }

    fn footer<'a>(&'a self, p: &Palette, now: Instant) -> El<'a, Message> {
        let t = self.shake_t.at(now);
        let shake = (t * std::f32::consts::PI * 6.0).sin() * (1.0 - t) * 10.0;
        let error: El<'a, Message> = match &self.error {
            Some(e) => container(
                row![
                    icons::icon(Glyph::Alert, 16.0, p.danger),
                    ui::colored(p.danger, e.clone(), 13.0)
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding(Padding {
                left: 12.0 + shake.max(0.0),
                right: 12.0 + (-shake).max(0.0),
                top: 0.0,
                bottom: 0.0,
            })
            .width(Length::Fill)
            .into(),
            None => ui::fill(),
        };
        let back = (self.step.index() > 0).then(|| {
            ui::action(
                p,
                Tone::Ghost,
                Some(Glyph::ChevronLeft),
                "Retour",
                (!self.saving).then_some(Message::Wizard(WizardMsg::Back)),
            )
        });
        let forward: El<'a, Message> = if self.step == Step::Summary {
            row![
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Check),
                    "Enregistrer",
                    (!self.saving).then_some(Message::Wizard(WizardMsg::Save(false)))
                ),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Eye),
                    if self.saving {
                        "Enregistrement…"
                    } else {
                        "Enregistrer et simuler"
                    },
                    (!self.saving).then_some(Message::Wizard(WizardMsg::Save(true)))
                ),
            ]
            .spacing(10)
            .into()
        } else {
            ui::action(
                p,
                Tone::Primary,
                None,
                "Continuer",
                Some(Message::Wizard(WizardMsg::Next)),
            )
        };
        container(
            row![
                ui::action(
                    p,
                    Tone::Ghost,
                    None,
                    "Annuler",
                    (!self.saving).then_some(Message::Wizard(WizardMsg::Close))
                ),
                error,
            ]
            .push(back)
            .push(forward)
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([16, 28]))
        .width(Length::Fill)
        .style(theme::panel(theme::mix(p.surface, p.sidebar, 0.5)))
        .into()
    }

    fn step_view<'a>(
        &'a self,
        p: &Palette,
        providers: &[&'static str],
        compact: bool,
    ) -> El<'a, Message> {
        match self.step {
            Step::Mode => self.mode_view(p),
            Step::Source => self.endpoint_view(p, Side::Source, providers, compact),
            Step::Destination => self.endpoint_view(p, Side::Destination, providers, compact),
            Step::Filters => self.filters_view(p, compact),
            Step::Safety => self.safety_view(p),
            Step::Schedule => {
                schedules::editor(p, &self.draft.schedule, self.draft.allow_delete, compact)
                    .map(|m| Message::Wizard(WizardMsg::Schedule(m)))
            }
            Step::Summary => self.summary_view(p),
        }
    }

    fn mode_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let modes = [
            (
                SyncMode::Backup,
                Glyph::Backup,
                p.success,
                [
                    "Copie les nouveautés et modifications",
                    "Ne supprime jamais rien",
                    "Versions gardées avant remplacement",
                ],
            ),
            (
                SyncMode::Mirror,
                Glyph::Mirror,
                p.accent,
                [
                    "Destination identique à la source",
                    "Suppressions seulement si autorisées",
                    "Bloqué si source vide ou > 50 %",
                ],
            ),
            (
                SyncMode::Bidirectional,
                Glyph::TwoWay,
                p.warning,
                [
                    "Changements propagés des deux côtés",
                    "Conflits conservés, jamais écrasés",
                    "Deux dossiers locaux requis",
                ],
            ),
        ];
        let tiles = modes.into_iter().map(|(mode, glyph, color, facts)| {
            let selected = self.draft.mode == Some(mode);
            let facts = facts.into_iter().fold(column![].spacing(8), |c, f| {
                c.push(
                    row![
                        icons::icon(Glyph::Check, 14.0, color),
                        ui::colored(p.muted, f, 13.0)
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
            });
            button(
                column![
                    ui::glyph_box(glyph, color, 56.0),
                    ui::title(p, model::mode_label(mode), 19.0),
                    ui::muted(p, model::mode_tagline(mode)),
                    ui::divider(p),
                    facts,
                ]
                .spacing(12),
            )
            .padding(20)
            .width(Length::Fill)
            .style(theme::tile(*p, selected, true))
            .on_press(Message::Wizard(WizardMsg::Mode(mode)))
            .into()
        });
        row(tiles).spacing(14).into()
    }

    fn endpoint_view<'a>(
        &'a self,
        p: &Palette,
        side: Side,
        providers: &[&'static str],
        compact: bool,
    ) -> El<'a, Message> {
        let d = match side {
            Side::Source => &self.draft.source,
            Side::Destination => &self.draft.destination,
        };
        let tiles = Provider::ALL.iter().map(|prov| {
            let compiled = providers.contains(&prov.key());
            let forbidden = if self.bidirectional() && *prov != Provider::Local {
                Some("Bidirectionnel : local uniquement")
            } else if side == Side::Destination && *prov == Provider::Onedrive {
                Some("Source uniquement")
            } else if !compiled {
                Some("Non inclus dans cette version")
            } else {
                None
            };
            let enabled = forbidden.is_none();
            let selected = d.provider == Some(*prov);
            let color = if enabled { p.accent } else { p.faint };
            button(
                row![
                    ui::glyph_box(prov.glyph(), color, 36.0),
                    column![
                        ui::strong(if enabled { p.text } else { p.faint }, prov.label(), 14.0),
                        ui::caption(p, forbidden.unwrap_or(prov.hint())),
                    ]
                    .spacing(2)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::tile(*p, selected, enabled))
            .on_press_maybe(enabled.then_some(Message::Wizard(WizardMsg::Provider(side, *prov))))
            .into()
        });
        let grid = iced::widget::grid(tiles)
            .fluid(if compact { 220.0 } else { 260.0 })
            .spacing(10)
            .height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink));
        let form = d.provider.map(|prov| self.endpoint_form(p, side, prov));
        column![grid].push(form).spacing(22).into()
    }

    fn endpoint_form<'a>(&'a self, p: &Palette, side: Side, prov: Provider) -> El<'a, Message> {
        let d = match side {
            Side::Source => &self.draft.source,
            Side::Destination => &self.draft.destination,
        };
        let input = move |field: Field,
                          label: &'static str,
                          placeholder: &'static str,
                          help: Option<&'static str>|
              -> El<'a, Message> {
            ui::field(
                p,
                label,
                placeholder,
                d.value(field),
                move |v| Message::Wizard(WizardMsg::Field(side, field, v)),
                help,
                None,
            )
        };
        let browse = |field: Field, label: &'static str| {
            ui::action(
                p,
                Tone::Secondary,
                Some(Glyph::Folder),
                label,
                Some(Message::Wizard(WizardMsg::Browse(side, field))),
            )
        };
        let path_row = |field: Field,
                        label: &'static str,
                        placeholder: &'static str,
                        help: &'static str,
                        button_label: &'static str|
         -> El<'a, Message> {
            let value = if field == Field::Path {
                d.path_value()
            } else {
                d.value(field)
            };
            column![
                ui::strong(p.muted, label, 12.0),
                row![
                    text_input(placeholder, value)
                        .on_input(move |v| Message::Wizard(WizardMsg::Field(side, field, v)))
                        .padding(Padding::from([9, 12]))
                        .size(14)
                        .style(theme::input(*p, false)),
                    browse(field, button_label),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                ui::caption(p, help),
            ]
            .spacing(6)
            .into()
        };
        let address = || -> El<'a, Message> {
            row![
                input(Field::Ip, "Adresse IP du serveur", "192.168.1.10", None),
                container(input(Field::Port, "Port", "22", None)).width(Length::Fixed(120.0)),
            ]
            .spacing(12)
            .into()
        };
        let secret_help = Some(
            "Nom d’une variable SYNCENGINE_SECRET_… lue à l’exécution. Aucun secret n’est enregistré.",
        );
        let flag = |flag: Flag, value: bool, label: &'static str| -> El<'a, Message> {
            checkbox(value)
                .label(label)
                .on_toggle(move |v| Message::Wizard(WizardMsg::Flag(side, flag, v)))
                .text_size(13)
                .style(theme::check(*p))
                .into()
        };
        let content: El<'a, Message> = match prov {
            Provider::Local => path_row(
                Field::Path,
                "Dossier",
                if cfg!(windows) { "C:\\Users\\vous\\Documents" } else { "/home/vous/Documents" },
                "Le dossier doit exister. Source et destination doivent être disjointes.",
                "Parcourir…",
            ),
            Provider::Smb => column![
                path_row(Field::Path, "Partage réseau", "\\\\nas\\sauvegardes", "Authentification gérée par le système. Linux : lecture seule.", "Parcourir…"),
            ]
            .into(),
            Provider::Sftp => column![
                address(),
                row![
                    input(Field::HostName, "Nom d’hôte vérifié", "nas.example", Some("Doit figurer dans known_hosts")),
                    input(Field::Username, "Utilisateur SSH", "backup", None),
                ]
                .spacing(12),
                path_row(Field::KnownHosts, "Fichier known_hosts", "C:\\Users\\vous\\.ssh\\known_hosts", "Clé de l’hôte déjà approuvée ; jamais écrite automatiquement.", "Choisir…"),
                input(Field::Root, "Dossier distant", "/sauvegardes", Some("Authentification par l’agent SSH du système")),
            ]
            .spacing(14)
            .into(),
            Provider::Webdav => column![
                input(Field::Url, "URL de la collection", "https://nas.example/dav/sauvegardes/", Some("HTTPS obligatoire, sans identifiants dans l’URL")),
                input(Field::CredentialRef, "Référence du jeton", "DAV", secret_help),
            ]
            .spacing(14)
            .into(),
            Provider::Ftp => column![
                ui::callout(p, p.danger, Glyph::Alert, "Protocole non chiffré", "Identifiants et fichiers circulent en clair sur le réseau. Préférez FTPS ou SFTP."),
                address(),
                row![
                    input(Field::Username, "Utilisateur", "backup", None),
                    input(Field::Root, "Dossier distant", "/sauvegardes", None),
                ]
                .spacing(12),
                input(Field::CredentialRef, "Référence du mot de passe", "FTP", secret_help),
                flag(Flag::Plaintext, d.acknowledge_plaintext, "J’accepte la transmission non chiffrée"),
                flag(Flag::Exclusive, d.exclusive_writers, "Aucun autre logiciel ne modifie ce dossier pendant les transferts"),
            ]
            .spacing(14)
            .into(),
            Provider::Ftps => column![
                address(),
                row![
                    input(Field::TlsDomain, "Nom du certificat TLS", "nas.example", None),
                    input(Field::Username, "Utilisateur", "backup", None),
                ]
                .spacing(12),
                input(Field::Root, "Dossier distant", "/sauvegardes", None),
                input(Field::CredentialRef, "Référence du mot de passe", "FTPS", secret_help),
                path_row(Field::RootCertificate, "Autorité privée (facultatif)", "", "Fichier PEM ; la confiance du système n’est pas modifiée.", "Choisir…"),
                flag(Flag::Exclusive, d.exclusive_writers, "Aucun autre logiciel ne modifie ce dossier pendant les transferts"),
            ]
            .spacing(14)
            .into(),
            Provider::Onedrive => column![
                ui::callout(p, p.warning, Glyph::Info, "Source expérimentale", "Lecture seule via Microsoft Graph, jeton obtenu hors de SyncEngine, sans garantie d’instantané."),
                input(Field::DriveId, "Identifiant du lecteur", "b!…", None),
                input(Field::DownloadHosts, "Hôtes de téléchargement approuvés", "public.dm.files.1drv.com", Some("Séparés par des virgules ; aucun joker")),
                input(Field::CredentialRef, "Référence du jeton", "ONEDRIVE", secret_help),
            ]
            .spacing(14)
            .into(),
        };
        container(
            column![
                row![
                    ui::glyph_box(prov.glyph(), p.accent, 30.0),
                    ui::heading(p, prov.label())
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                content
            ]
            .spacing(16),
        )
        .padding(20)
        .width(Length::Fill)
        .style(theme::card(*p, 0.0, false))
        .into()
    }

    fn filters_view<'a>(&'a self, p: &Palette, compact: bool) -> El<'a, Message> {
        let f = &self.draft.filters;
        let presets = PRESETS.iter().enumerate().map(|(i, preset)| {
            let on = f.has_preset(preset);
            button(
                row![
                    container(if on {
                        icons::icon(Glyph::Check, 14.0, p.on_accent)
                    } else {
                        ui::hspace(14.0)
                    })
                    .width(Length::Fixed(22.0))
                    .height(Length::Fixed(22.0))
                    .center(Length::Fixed(22.0))
                    .style(theme::outline(
                        *p,
                        if on { p.accent } else { p.faint },
                        if on { 8.0 } else { 0.0 }
                    )),
                    column![
                        ui::strong(p.text, preset.label, 14.0),
                        ui::caption(p, preset.hint)
                    ]
                    .spacing(2)
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::tile(*p, on, true))
            .on_press(Message::Wizard(WizardMsg::Preset(i)))
            .into()
        });
        let grid = iced::widget::grid(presets)
            .fluid(if compact { 260.0 } else { 300.0 })
            .spacing(10)
            .height(iced::widget::grid::Sizing::EvenlyDistribute(Length::Shrink));
        let list =
            |list: FilterList, label: &'static str, help: &'static str, color| -> El<'a, Message> {
                let chips = f.list(list).iter().enumerate().map(|(i, pattern)| {
                    button(
                        row![
                            ui::mono(p.text, pattern.clone(), 12.0),
                            icons::icon(Glyph::Close, 11.0, p.muted)
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding(Padding::from([4, 10]))
                    .style(theme::chip(*p, color, true))
                    .on_press(Message::Wizard(WizardMsg::RemovePattern(list, i)))
                    .into()
                });
                let value = &self.inputs[list_index(list)];
                column![
                    ui::strong(p.text, label, 14.0),
                    ui::caption(p, help),
                    row(chips).spacing(6).wrap(),
                    row![
                        text_input("Ajouter un motif puis Entrée", value)
                            .on_input(move |v| Message::Wizard(WizardMsg::PatternInput(list, v)))
                            .on_submit(Message::Wizard(WizardMsg::AddPattern(list)))
                            .padding(Padding::from([8, 12]))
                            .size(13)
                            .style(theme::input(*p, false)),
                        ui::action(
                            p,
                            Tone::Secondary,
                            Some(Glyph::Plus),
                            "Ajouter",
                            (!value.trim().is_empty())
                                .then_some(Message::Wizard(WizardMsg::AddPattern(list)))
                        ),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                ]
                .spacing(8)
                .into()
            };
        column![
            ui::eyebrow(p.muted, "Raccourcis"),
            grid,
            ui::divider(p),
            list(
                FilterList::ExcludeFiles,
                "Exclure des fichiers",
                "Ex. *.tmp exclut les fichiers temporaires ; **/*.iso dans tous les sous-dossiers.",
                p.danger
            ),
            list(
                FilterList::ExcludeDirectories,
                "Exclure des dossiers",
                "Ex. cache exclut tout dossier nommé cache (non parcouru).",
                p.warning
            ),
            list(
                FilterList::IncludeFiles,
                "Inclure uniquement",
                "Vide = tous les fichiers. Ex. **/*.jpg pour ne garder que les photos.",
                p.success
            ),
            toggler(f.case_insensitive)
                .label("Ignorer la casse des motifs")
                .on_toggle(|v| Message::Wizard(WizardMsg::CaseInsensitive(v)))
                .text_size(14)
                .style(theme::switch(*p, false)),
        ]
        .spacing(16)
        .into()
    }

    /// How one-way previews recognize identical files.
    fn compare_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        if self.bidirectional() {
            return ui::callout(
                p,
                p.info,
                Glyph::TwoWay,
                "Comparaison par contenu",
                "Le bidirectionnel lit l’empreinte de chaque fichier : l’état de référence doit prouver qu’un fichier n’a pas changé.",
            );
        }
        let current = self.draft.compare;
        let tile = |method: CompareMethod,
                    glyph: Glyph,
                    title: &'static str,
                    body: &'static str|
         -> El<'a, Message> {
            let on = current == method;
            button(
                row![
                    ui::glyph_box(glyph, if on { p.accent } else { p.muted }, 36.0),
                    column![ui::strong(p.text, title, 14.0), ui::caption(p, body)]
                        .spacing(3)
                        .width(Length::Fill),
                    icons::icon(
                        if on {
                            Glyph::Check
                        } else {
                            Glyph::ChevronRight
                        },
                        16.0,
                        if on { p.accent } else { p.faint }
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding(14)
            .width(Length::Fill)
            .style(theme::tile(*p, on, true))
            .on_press(Message::Wizard(WizardMsg::Compare(method)))
            .into()
        };
        column![
            ui::strong(p.muted, "Comparaison des fichiers", 12.0),
            tile(
                CompareMethod::SizeAndDate,
                Glyph::Bolt,
                "Taille et date — rapide",
                "Seuls les dossiers sont lus. Une modification qui garde la même taille et la même date passe inaperçue.",
            ),
            tile(
                CompareMethod::Content,
                Glyph::Shield,
                "Contenu — empreintes BLAKE3",
                "Relit les fichiers de même taille des deux côtés : plus lent, détecte tout changement de contenu.",
            ),
        ]
        .spacing(8)
        .into()
    }

    fn safety_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let guard = |title: &'static str, body: &'static str| -> El<'a, Message> {
            row![
                icons::icon(Glyph::Shield, 18.0, p.success),
                column![ui::strong(p.text, title, 14.0), ui::caption(p, body)].spacing(2)
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        };
        let guards = column![
            guard("Simulation obligatoire", "Le plan exact affiché est le seul exécutable ; toute modification exige une nouvelle simulation."),
            guard("Copie vérifiée", "Empreinte BLAKE3 calculée pendant la copie et contenu relu avant publication."),
            guard("Versions conservées", "Avant chaque remplacement ou suppression."),
            guard("Jamais de suppression déduite d’une panne", "Source inaccessible, inventaire incomplet ou source vide : suppressions refusées."),
        ]
        .spacing(14);
        let deletion: El<'a, Message> = if self.draft.deletions_possible() {
            let on = self.draft.allow_delete;
            container(
                column![
                    row![
                        ui::glyph_box(Glyph::Trash, if on { p.danger } else { p.muted }, 40.0),
                        column![
                            ui::strong(p.text, "Autoriser les suppressions protégées", 15.0),
                            ui::caption(p, if self.draft.mode == Some(SyncMode::Mirror) {
                                "Réplique les retraits de la source. Sans cela, le miroir garde les fichiers en trop et le signale."
                            } else {
                                "Propage les suppressions détectées grâce à l’état précédent fiable."
                            }),
                        ]
                        .spacing(3)
                        .width(Length::Fill),
                        toggler(on)
                            .on_toggle(|v| Message::Wizard(WizardMsg::AllowDelete(v)))
                            .size(24)
                            .style(theme::switch(*p, true)),
                    ]
                    .spacing(14)
                    .align_y(Alignment::Center),
                    ui::caption(p, "Source vide ou illisible toujours bloquée. Au-delà de 50 % de la destination (dès 10 suppressions), la simulation demande votre accord explicite. Une confirmation distincte est demandée à chaque exécution."),
                ]
                .spacing(10),
            )
            .padding(18)
            .style(theme::outline(*p, if on { p.danger } else { p.border }, if on { 1.0 } else { 0.0 }))
            .into()
        } else {
            ui::callout(
                p,
                p.success,
                Glyph::Backup,
                "Une sauvegarde ne supprime jamais",
                "Les fichiers retirés de la source restent à destination.",
            )
        };
        column![
            deletion,
            self.compare_view(p),
            container(guards)
                .padding(18)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false))
        ]
        .spacing(16)
        .into()
    }

    fn summary_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let d = &self.draft;
        let groups = self.groups.iter().map(|g| {
            button(ui::colored(p.text, g.clone(), 13.0))
                .padding(Padding::from([4, 12]))
                .style(theme::chip(*p, p.accent, d.group.trim() == g))
                .on_press(Message::Wizard(WizardMsg::Group(g.clone())))
                .into()
        });
        let side = |label: &'static str, e: &crate::draft::EndpointDraft| -> El<'a, Message> {
            let (local, remote) = e.build().unwrap_or_default();
            let prov = e.provider.unwrap_or(Provider::Local);
            row![
                ui::glyph_box(prov.glyph(), p.accent, 34.0),
                column![
                    ui::eyebrow(p.faint, label),
                    ui::mono(
                        p.text,
                        model::ellipsize(&model::endpoint_location(&local, remote.as_ref()), 70),
                        13.0
                    ),
                    ui::caption(p, model::provider_label(remote.as_ref())),
                ]
                .spacing(2)
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        };
        let schedule = match d.schedule.trigger() {
            Ok(Some(t)) => crate::draft::trigger_label(&t),
            _ if d.schedule.kind == TriggerKind::Manual => "Lancement manuel".into(),
            _ => "Planification invalide".into(),
        };
        let item = |g: Glyph, label: String, step: Step| -> El<'a, Message> {
            button(
                row![
                    icons::icon(g, 16.0, p.muted),
                    ui::colored(p.text, label, 13.0),
                    ui::fill(),
                    icons::icon(Glyph::Pencil, 13.0, p.faint)
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([8, 10]))
            .width(Length::Fill)
            .style(theme::row_btn(*p, false))
            .on_press(Message::Wizard(WizardMsg::GoTo(step)))
            .into()
        };
        let mode = d.mode.map_or("—", model::mode_label);
        column![
            row![
                ui::field(
                    p,
                    "Nom du profil",
                    "Ex. Documents vers NAS",
                    &d.name,
                    |v| Message::Wizard(WizardMsg::Name(v)),
                    None,
                    None
                ),
                ui::field(
                    p,
                    "Groupe",
                    "Sans groupe",
                    &d.group,
                    |v| Message::Wizard(WizardMsg::Group(v)),
                    None,
                    None
                ),
            ]
            .spacing(14),
            row(groups).spacing(6).wrap(),
            container(
                column![
                    side("Source", &d.source),
                    container(icons::icon(
                        if d.mode == Some(SyncMode::Bidirectional) {
                            Glyph::TwoWay
                        } else {
                            Glyph::ArrowRight
                        },
                        18.0,
                        p.accent
                    ))
                    .padding(Padding {
                        left: 8.0,
                        ..Padding::ZERO
                    }),
                    side("Destination", &d.destination),
                    ui::divider(p),
                    item(Glyph::Layers, format!("Mode : {mode}"), Step::Mode),
                    item(
                        Glyph::Filter,
                        format!(
                            "{} motif(s) de filtre{}",
                            d.filters.count(),
                            if d.filters.case_insensitive {
                                ", casse ignorée"
                            } else {
                                ""
                            }
                        ),
                        Step::Filters
                    ),
                    item(
                        Glyph::Eye,
                        format!(
                            "Comparaison : {}",
                            model::compare_label(d.effective_compare())
                        ),
                        Step::Safety
                    ),
                    item(
                        Glyph::Shield,
                        if d.allow_delete {
                            "Suppressions protégées autorisées".into()
                        } else {
                            "Aucune suppression".into()
                        },
                        Step::Safety
                    ),
                    item(Glyph::Clock, schedule, Step::Schedule),
                ]
                .spacing(10),
            )
            .padding(18)
            .style(theme::card(*p, 0.0, false)),
            ui::caption(
                p,
                "« Enregistrer et simuler » calcule le plan sans modifier aucun fichier."
            ),
        ]
        .spacing(16)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_gated_and_name_is_suggested() {
        let now = Instant::now();
        let mut w = Wizard::open(ProfileDraft::new(""), vec![], now);
        assert!(matches!(w.update(WizardMsg::Next, now), Outcome::None));
        assert_eq!(w.step, Step::Mode);
        assert!(w.error.is_some());
        w.update(WizardMsg::Mode(SyncMode::Bidirectional), now);
        w.update(WizardMsg::GoTo(Step::Summary), now);
        assert_eq!(
            w.step,
            Step::Source,
            "forward jumps stop at the first invalid step"
        );
        w.update(WizardMsg::Provider(Side::Source, Provider::Sftp), now);
        assert_eq!(w.draft.source.provider, Some(Provider::Sftp));
        w.update(WizardMsg::Mode(SyncMode::Bidirectional), now);
        assert_eq!(
            w.draft.source.provider,
            Some(Provider::Local),
            "two-way forces local"
        );
        let dir = if cfg!(windows) {
            "C:\\data\\Photos"
        } else {
            "/data/Photos"
        };
        w.update(WizardMsg::Field(Side::Source, Field::Path, dir.into()), now);
        w.update(WizardMsg::Next, now);
        assert_eq!(w.step, Step::Destination);
        let dst = if cfg!(windows) {
            "C:\\data\\Copie"
        } else {
            "/data/Copie"
        };
        w.update(WizardMsg::Provider(Side::Destination, Provider::Local), now);
        w.update(
            WizardMsg::Field(Side::Destination, Field::Path, dst.into()),
            now,
        );
        for _ in 0..3 {
            w.update(WizardMsg::Next, now);
        }
        assert_eq!(w.step, Step::Schedule);
        w.update(
            WizardMsg::Schedule(ScheduleMsg::Kind(TriggerKind::Interval)),
            now,
        );
        w.update(WizardMsg::Next, now);
        assert_eq!(w.step, Step::Schedule, "consent required");
        w.update(WizardMsg::Schedule(ScheduleMsg::Writes(true)), now);
        w.update(WizardMsg::Next, now);
        assert_eq!(w.step, Step::Summary);
        assert_eq!(w.draft.name, "Photos vers Copie");
        match w.update(WizardMsg::Save(true), now) {
            Outcome::Save {
                profile,
                schedule,
                simulate,
            } => {
                assert!(simulate);
                assert_eq!(profile.mode, SyncMode::Bidirectional);
                assert!(matches!(
                    schedule,
                    ScheduleChange::Set {
                        approve_writes: true,
                        approve_deletions: false,
                        ..
                    }
                ));
            }
            _ => panic!("save expected"),
        }
        assert!(w.saving);
        w.close(now);
        assert!(!w.closing, "cannot close while saving");
    }
}
