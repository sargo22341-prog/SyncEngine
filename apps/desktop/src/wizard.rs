//! Step-by-step profile assistant. No JSON anywhere: every engine setting is
//! a guided choice, validated before moving on and again by the facade.

use crate::anim::{Ease, Tween};
use crate::app::Message;
use crate::backend::{self, ScheduleChange};
use crate::charts;
use crate::draft::{Field, FilterList, PRESETS, ProfileDraft, Provider, TriggerKind};
use crate::icons::{self, Glyph};
use crate::model;
use crate::native;
use crate::schedules::{self, ScheduleMsg};
use crate::shell;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{
    Space, button, checkbox, column, container, opaque, row, scrollable, text, text_input,
};
use iced::{Alignment, Color, Length, Padding, Task};
use std::path::PathBuf;
use std::time::Instant;
use sync_plan::SyncMode;
use sync_profiles::{CompareMethod, Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// Mode, source and destination together: they depend on each other (a
    /// two-way profile forces local folders on both sides).
    Locations,
    Filters,
    Safety,
    Schedule,
    Summary,
}

impl Step {
    pub(crate) const ALL: [Self; 5] = [
        Self::Locations,
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
            Self::Locations => "Que voulez-vous faire, et où ?",
            Self::Filters => "Que faut-il ignorer ?",
            Self::Safety => "Protections et suppressions",
            Self::Schedule => "Quand synchroniser ?",
            Self::Summary => "Dernière vérification",
        }
    }
    fn short(self) -> &'static str {
        match self {
            Self::Locations => "Mode et emplacements",
            Self::Filters => "Filtres et exclusions",
            Self::Safety => "Garde-fous",
            Self::Schedule => "Planification",
            Self::Summary => "Récapitulatif",
        }
    }
    fn subtitle(self) -> &'static str {
        match self {
            Self::Locations => {
                "Chaque mode a sa stratégie, toujours prévisualisée avant écriture. La destination doit être distincte de la source et non imbriquée."
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
    Trash(bool),
    TrashSize(String),
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
            step: Step::Locations,
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
            Step::Locations => {
                if d.mode.is_none() {
                    return Some("Choisissez un mode pour continuer".to_owned());
                }
                if let Err(e) = d.source.build() {
                    return Some(format!("Source — {e}"));
                }
                if d.destination.provider == Some(Provider::Onedrive) {
                    return Some("OneDrive ne peut être qu’une source".into());
                }
                d.destination
                    .build()
                    .err()
                    .map(|e| format!("Destination — {e}"))
                    .or_else(|| d.remote_pair_error())
            }
            Step::Filters => None,
            Step::Safety => d.trash().err(),
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
        // What is synchronized where: changing it is a new decision, so
        // the schedule consents must be given again. Other edits (filters,
        // name, group, trash, comparison) keep them: saving from this
        // assistant, which shows them, renews them for the new configuration.
        let endpoints_edit = matches!(
            &msg,
            WizardMsg::Mode(_)
                | WizardMsg::Provider(..)
                | WizardMsg::Field(..)
                | WizardMsg::Flag(..)
                | WizardMsg::Browsed(..)
        );
        let before = endpoints_edit.then(|| {
            (
                self.draft.mode,
                self.draft.source.clone(),
                self.draft.destination.clone(),
            )
        });
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
            WizardMsg::CaseInsensitive(v) => self.draft.filters.set_case_insensitive(v),
            WizardMsg::AllowDelete(v) => {
                self.draft.allow_delete = v && self.draft.deletions_possible();
                if !self.draft.allow_delete {
                    self.draft.schedule.approve_deletions = false;
                }
            }
            WizardMsg::Trash(on) => self.draft.trash_enabled = on,
            WizardMsg::TrashSize(v) => {
                self.draft.trash_size = v.chars().filter(char::is_ascii_digit).take(6).collect();
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
        if before.is_some_and(|(mode, source, destination)| {
            mode != self.draft.mode
                || source != self.draft.source
                || destination != self.draft.destination
        }) {
            self.draft.schedule.approve_writes = false;
            self.draft.schedule.approve_deletions = false;
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
                content_p.muted,
                &format!("Étape {step_no} sur {}", Step::ALL.len()),
            ),
            ui::title(&content_p, self.step.title(), 32.0),
        ]
        .spacing(4);
        let main = column![
            container(header).padding(Padding {
                top: 30.0,
                bottom: 18.0,
                left: 48.0 + shift * 0.4,
                right: 48.0
            }),
            scrollable(
                container(body)
                    .padding(Padding::from([4, 48]))
                    .width(Length::Fill)
            )
            .style(theme::scroll(panel_p))
            .height(Length::Fill),
            self.footer(&panel_p, now),
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        let layout = row![
            self.sidebar(&panel_p),
            container(Space::new())
                .width(Length::Fixed(1.0))
                .height(Length::Fill)
                .style(theme::panel(panel_p.border)),
            main,
        ];
        opaque(
            container(layout)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::panel(theme::alpha(p.bg, open)))
                .clip(true),
        )
    }

    /// Left column of the assistant: title, the five steps as a vertical
    /// list (done, current, upcoming) and the note of the current step.
    fn sidebar<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let current = self.step.index();
        let mut steps = column![].spacing(2);
        for (i, step) in Step::ALL.iter().enumerate() {
            let done = i < current
                || (i <= self.visited && i != current && self.step_error(*step).is_none());
            let active = i == current;
            let reachable = i <= self.visited || i == current + 1;
            let (fill, edge, ink) = if active {
                (p.text, p.text, p.bg)
            } else if done {
                (p.backup, p.backup, Color::from_rgb(0.043, 0.047, 0.055))
            } else {
                (Color::TRANSPARENT, p.line, p.muted)
            };
            let dot = container(if done && !active {
                icons::icon(Glyph::Check, 13.0, ink)
            } else {
                ui::mono(ink, (i + 1).to_string(), 11.0)
            })
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0))
            .center(Length::Fixed(24.0))
            .style(theme::step_dot(fill, edge));
            steps = steps.push(
                button(
                    row![
                        dot,
                        ui::colored(
                            if active || done { p.text } else { p.muted },
                            step.short(),
                            14.0
                        )
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding(Padding::from([8, 12]))
                .width(Length::Fill)
                .style(theme::step_row(*p, active))
                .on_press_maybe(reachable.then_some(Message::Wizard(WizardMsg::GoTo(*step)))),
            );
        }
        let note = container(
            column![
                ui::colored(p.muted, self.step.subtitle(), 12.0),
                ui::kbd(p, "Échap · quitter"),
            ]
            .spacing(8),
        )
        .padding(12)
        .width(Length::Fill)
        .style(theme::outline(*p, p.border, 0.0));
        container(
            column![
                row![
                    charts::mark(
                        charts::Mark {
                            from: p.backup,
                            to: p.violet,
                            turn: 0.0
                        },
                        24.0
                    ),
                    ui::title(
                        p,
                        if self.draft.existing.is_some() {
                            "Modifier le profil"
                        } else {
                            "Nouveau profil"
                        },
                        18.0
                    ),
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .padding(Padding::from([2, 10])),
                steps,
                Space::new().height(Length::Fill),
                note,
            ]
            .spacing(18),
        )
        .width(Length::Fixed(300.0))
        .height(Length::Fill)
        .padding(Padding::from([18, 14]))
        .style(theme::panel(p.rail))
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
            .align_x(iced::alignment::Horizontal::Right)
            .into(),
            None => ui::fill(),
        };
        let back = (self.step.index() > 0).then(|| {
            ui::action(
                p,
                Tone::Secondary,
                None,
                "← Retour",
                (!self.saving).then_some(Message::Wizard(WizardMsg::Back)),
            )
        });
        let forward: El<'a, Message> = if self.step == Step::Summary {
            row![
                ui::action(
                    p,
                    Tone::Secondary,
                    None,
                    "Enregistrer",
                    (!self.saving).then_some(Message::Wizard(WizardMsg::Save(false)))
                ),
                ui::action(
                    p,
                    Tone::Primary,
                    None,
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
                "Continuer →",
                Some(Message::Wizard(WizardMsg::Next)),
            )
        };
        container(
            row![
                ui::action(
                    p,
                    Tone::Secondary,
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
        .padding(Padding::from([16, 48]))
        .width(Length::Fill)
        .into()
    }

    fn step_view<'a>(
        &'a self,
        p: &Palette,
        providers: &[&'static str],
        compact: bool,
    ) -> El<'a, Message> {
        match self.step {
            Step::Locations => self.locations_view(p, providers),
            Step::Filters => self.filters_view(p, compact),
            Step::Safety => self.safety_view(p),
            Step::Schedule => {
                schedules::editor(p, &self.draft.schedule, self.draft.allow_delete, compact)
                    .map(|m| Message::Wizard(WizardMsg::Schedule(m)))
            }
            Step::Summary => self.summary_view(p),
        }
    }

    /// Mode choice, then source and destination side by side.
    fn locations_view<'a>(&'a self, p: &Palette, providers: &[&'static str]) -> El<'a, Message> {
        let arrow_mode = self.draft.mode;
        let arrow: El<'a, Message> = match arrow_mode {
            Some(mode) => container(ui::mode_tile(p, mode, 44.0)).into(),
            None => container(icons::icon(Glyph::ArrowRight, 20.0, p.muted))
                .width(Length::Fixed(44.0))
                .height(Length::Fixed(44.0))
                .center(Length::Fixed(44.0))
                .style(theme::swatch(p.surface_hi))
                .into(),
        };
        column![
            self.mode_view(p),
            row![
                container(self.endpoint_card(
                    p,
                    Side::Source,
                    "D’où viennent les fichiers ?",
                    providers
                ))
                .width(Length::FillPortion(1)),
                container(arrow)
                    .width(Length::Fixed(70.0))
                    .center_x(Length::Fixed(70.0))
                    .center_y(Length::Fill),
                container(self.endpoint_card(
                    p,
                    Side::Destination,
                    "Où doivent-ils aller ?",
                    providers
                ))
                .width(Length::FillPortion(1)),
            ]
            .height(Length::Shrink),
        ]
        .spacing(20)
        .into()
    }

    fn mode_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let modes = [
            (
                SyncMode::Backup,
                "Copie la source vers la destination. Ne supprime jamais : les fichiers présents seulement à la destination sont conservés.",
            ),
            (
                SyncMode::Mirror,
                "Rend la destination identique à la source. Suppressions possibles, avec confirmation séparée.",
            ),
            (
                SyncMode::Bidirectional,
                "Propage les changements dans les deux sens (deux dossiers locaux). Les conflits vous sont soumis.",
            ),
        ];
        let tiles = modes.into_iter().map(|(mode, body)| {
            let selected = self.draft.mode == Some(mode);
            button(
                column![
                    row![
                        ui::mode_tile(p, mode, 36.0),
                        ui::strong(p.text, model::mode_label(mode), 15.0),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                    ui::colored(p.muted, body, 12.0),
                ]
                .spacing(10),
            )
            .padding(18)
            .width(Length::Fill)
            .style(theme::option(*p, shell::mode_fill(p, mode), selected))
            .on_press(Message::Wizard(WizardMsg::Mode(mode)))
            .into()
        });
        row(tiles).spacing(12).height(Length::Shrink).into()
    }

    /// One side of the profile: provider chips, then the form of the chosen
    /// provider, in a card.
    fn endpoint_card<'a>(
        &'a self,
        p: &Palette,
        side: Side,
        question: &'static str,
        providers: &[&'static str],
    ) -> El<'a, Message> {
        container(
            column![
                ui::eyebrow(p.muted, question),
                self.endpoint_view(p, side, providers),
            ]
            .spacing(14),
        )
        .padding(20)
        .width(Length::Fill)
        .style(theme::card(*p, 0.0, false))
        .into()
    }

    fn endpoint_view<'a>(
        &'a self,
        p: &Palette,
        side: Side,
        providers: &[&'static str],
    ) -> El<'a, Message> {
        let d = match side {
            Side::Source => &self.draft.source,
            Side::Destination => &self.draft.destination,
        };
        let chips = Provider::ALL.iter().map(|prov| {
            let compiled = providers.contains(&prov.key());
            let forbidden = if self.bidirectional() && *prov != Provider::Local {
                Some("Bidirectionnel : dossiers locaux uniquement")
            } else if side == Side::Destination && *prov == Provider::Onedrive {
                Some("OneDrive ne peut être qu’une source")
            } else if !compiled {
                Some("Non inclus dans cette version")
            } else {
                None
            };
            let enabled = forbidden.is_none();
            let selected = d.provider == Some(*prov);
            let chip = button(
                text(prov.label())
                    .size(12)
                    .font(ui::weight(iced::font::Weight::Semibold)),
            )
            .padding(Padding::from([4, 12]))
            .style(theme::pick_chip(*p, selected, enabled))
            .on_press_maybe(enabled.then_some(Message::Wizard(WizardMsg::Provider(side, *prov))));
            match forbidden {
                Some(reason) => ui::with_tip(p, chip, reason, iced::widget::tooltip::Position::Top),
                None => chip.into(),
            }
        });
        let chips: El<'a, Message> = row(chips).spacing(6).wrap().into();
        let form = d.provider.map(|prov| self.endpoint_form(p, side, prov));
        column![chips].push(form).spacing(14).into()
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
                container(
                    row![
                        text_input(placeholder, value)
                            .on_input(move |v| Message::Wizard(WizardMsg::Field(side, field, v)))
                            .font(ui::mono_font())
                            .padding(Padding::from([10, 0]))
                            .size(13)
                            .style(theme::search(*p)),
                        button(ui::strong(p.accent, button_label, 13.0))
                            .padding(Padding::from([4, 6]))
                            .style(theme::btn(*p, Tone::Ghost))
                            .on_press(Message::Wizard(WizardMsg::Browse(side, field))),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                )
                .padding(Padding::from([0, 12]))
                .style(theme::field_box(*p, 12.0)),
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
        content
    }

    fn filters_view<'a>(&'a self, p: &Palette, _compact: bool) -> El<'a, Message> {
        let f = &self.draft.filters;
        let mut presets = column![ui::eyebrow(p.muted, "Catégories prêtes")].spacing(4);
        for (i, preset) in PRESETS.iter().enumerate() {
            presets = presets.push(ui::toggle_row(
                p,
                f.has_preset(preset),
                preset.label,
                Some(preset.hint),
                false,
                move |_| Message::Wizard(WizardMsg::Preset(i)),
                None,
            ));
        }
        let list =
            |list: FilterList, label: &'static str, help: &'static str, color| -> El<'a, Message> {
                let chips = f.list(list).iter().enumerate().map(|(i, pattern)| {
                    button(
                        row![
                            ui::mono(color, pattern.clone(), 12.0),
                            icons::icon(Glyph::Close, 11.0, color)
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .padding(Padding::from([4, 10]))
                    .style(theme::tint_btn(
                        color,
                        if p.is_light() { 0.16 } else { 0.18 },
                    ))
                    .on_press(Message::Wizard(WizardMsg::RemovePattern(list, i)))
                    .into()
                });
                let value = &self.inputs[list_index(list)];
                column![
                    ui::strong(p.text, label, 13.0),
                    ui::caption(p, help),
                    row(chips).spacing(6).wrap(),
                    container(
                        row![
                            text_input("Ajouter un motif puis Entrée", value)
                                .on_input(move |v| Message::Wizard(WizardMsg::PatternInput(
                                    list, v
                                )))
                                .on_submit(Message::Wizard(WizardMsg::AddPattern(list)))
                                .font(ui::mono_font())
                                .padding(Padding::from([8, 0]))
                                .size(13)
                                .style(theme::search(*p)),
                            button(ui::strong(
                                if value.trim().is_empty() {
                                    p.faint
                                } else {
                                    p.accent
                                },
                                "Ajouter",
                                13.0
                            ))
                            .padding(Padding::from([4, 6]))
                            .style(theme::btn(*p, Tone::Ghost))
                            .on_press_maybe(
                                (!value.trim().is_empty())
                                    .then_some(Message::Wizard(WizardMsg::AddPattern(list)))
                            ),
                        ]
                        .align_y(Alignment::Center)
                    )
                    .padding(Padding::from([0, 12]))
                    .style(theme::field_box(*p, 12.0)),
                ]
                .spacing(8)
                .into()
            };
        let mut patterns = column![
            ui::eyebrow(p.muted, "Vos motifs"),
            list(
                FilterList::ExcludeFiles,
                "Exclure des fichiers",
                "Ex. *.tmp exclut les fichiers temporaires ; **/*.iso dans tous les sous-dossiers.",
                p.mirror_ink
            ),
            list(
                FilterList::ExcludeDirectories,
                "Exclure des dossiers",
                "Ex. cache exclut tout dossier nommé cache (non parcouru).",
                p.mirror_ink
            ),
            list(
                FilterList::IncludeFiles,
                "Inclure uniquement",
                "Vide = tous les fichiers. Ex. **/*.jpg pour ne garder que les photos.",
                p.backup_ink
            ),
        ]
        .spacing(14);
        patterns = patterns.push(if crate::draft::case_choice_available() {
            ui::toggle_row(
                p,
                f.case_insensitive,
                "Ignorer la casse des motifs",
                None,
                false,
                |v| Message::Wizard(WizardMsg::CaseInsensitive(v)),
                None,
            )
        } else {
            ui::caption(
                p,
                "Comme les noms de fichiers Windows, les motifs ignorent la casse : Thumbs.db exclut aussi thumbs.db.",
            )
        });
        let card = |content: iced::widget::Column<'a, Message>| -> El<'a, Message> {
            container(content)
                .padding(Padding::from([14, 18]))
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false))
                .into()
        };
        column![
            ui::callout(
                p,
                p.info,
                Glyph::Info,
                "Mêmes règles partout",
                "Les filtres s’appliquent aux deux côtés, en simulation comme en exécution. Vous pourrez aussi exclure un dossier depuis le plan simulé.",
            ),
            row![
                container(card(presets)).width(Length::FillPortion(1)),
                container(card(patterns)).width(Length::FillPortion(1)),
            ]
            .spacing(14),
        ]
        .spacing(14)
        .into()
    }

    /// How one-way previews recognize identical files.
    fn compare_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let card = |content: El<'a, Message>| -> El<'a, Message> {
            container(content)
                .padding(20)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false))
                .into()
        };
        if self.bidirectional() {
            return card(
                column![
                    ui::eyebrow(p.muted, "Comparaison des fichiers"),
                    ui::strong(p.text, "Contenu (BLAKE3)", 14.0),
                    ui::caption(
                        p,
                        "Le bidirectionnel lit l’empreinte de chaque fichier : l’état de référence doit prouver qu’un fichier n’a pas changé."
                    ),
                ]
                .spacing(8)
                .into(),
            );
        }
        let current = self.draft.compare;
        let option = |method: CompareMethod, title: &'static str, body: &'static str| {
            let on = current == method;
            button(
                column![
                    ui::strong(p.text, title, 14.0),
                    ui::colored(p.muted, body, 12.0)
                ]
                .spacing(4)
                .width(Length::Fill),
            )
            .padding(16)
            .width(Length::Fill)
            .style(theme::option(*p, p.violet, on))
            .on_press(Message::Wizard(WizardMsg::Compare(method)))
        };
        card(
            column![
                ui::eyebrow(p.muted, "Comparaison des fichiers"),
                row![
                    option(
                        CompareMethod::SizeAndDate,
                        "Taille et date",
                        "Rapide. Lit seulement les listes de dossiers ; une modification qui garde taille et date passe inaperçue."
                    ),
                    option(
                        CompareMethod::Content,
                        "Contenu (BLAKE3)",
                        "Plus lent. Relit les fichiers de même taille des deux côtés et détecte tout changement."
                    ),
                ]
                .spacing(12)
                .height(Length::Shrink),
            ]
            .spacing(12)
            .into(),
        )
    }

    fn safety_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let on = self.draft.trash_enabled;
        let explanation = if on {
            match self.draft.trash_limit() {
                Ok(limit) => format!(
                    "Les anciennes versions sont gardées jusqu’à {}, sur les {} dernières synchronisations au maximum.",
                    model::bytes(limit),
                    sync_orchestrator::KEPT_TRASH_RUNS
                ),
                Err(e) => e,
            }
        } else {
            "Les fichiers remplacés ou supprimés ne seront pas récupérables.".to_owned()
        };
        let size: Option<El<'a, Message>> = on.then(|| {
            let invalid = self.draft.trash_limit().is_err();
            container(
                row![
                    text_input("5", &self.draft.trash_size)
                        .on_input(|v| Message::Wizard(WizardMsg::TrashSize(v)))
                        .font(ui::mono_font())
                        .padding(Padding::from([8, 0]))
                        .width(Length::Fixed(48.0))
                        .size(13)
                        .style(theme::search(*p)),
                    ui::mono(p.muted, "Go", 12.0),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([0, 12]))
            .style(theme::field_box(
                Palette {
                    line: if invalid { p.danger } else { p.line },
                    ..*p
                },
                12.0,
            ))
            .into()
        });
        let mut retention = column![
            ui::eyebrow(p.muted, "Si un fichier est remplacé ou supprimé"),
            ui::toggle_row(
                p,
                on,
                "Garder l’ancienne version en corbeille",
                Some(&explanation),
                false,
                |v| Message::Wizard(WizardMsg::Trash(v)),
                size,
            ),
        ]
        .spacing(6);
        if self.draft.trash_limit().is_err() && on {
            retention = retention.push(ui::colored(
                p.danger,
                "Un nombre entier de 1 à 100 000 Go",
                12.0,
            ));
        }
        if self.draft.deletions_possible() {
            retention = retention.push(ui::toggle_row(
                p,
                self.draft.allow_delete,
                "Autoriser les suppressions protégées",
                Some(if self.draft.mode == Some(SyncMode::Mirror) {
                    "Miroir : réplique les retraits de la source, avec une confirmation à chaque exécution. Sans cela, les fichiers en trop sont gardés et signalés."
                } else {
                    "Propage les suppressions détectées grâce à l’état précédent fiable, avec une confirmation à chaque exécution."
                }),
                true,
                |v| Message::Wizard(WizardMsg::AllowDelete(v)),
                None,
            ));
        } else {
            retention = retention.push(ui::caption(
                p,
                "Une sauvegarde ne supprime jamais : les fichiers retirés de la source restent à destination.",
            ));
        }
        column![
            self.compare_view(p),
            container(retention)
                .padding(Padding::from([14, 18]))
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false)),
            ui::callout(
                p,
                p.backup_ink,
                Glyph::Shield,
                "Toujours actifs, quel que soit votre choix",
                "Simulation obligatoire avant écriture · copie vérifiée par empreinte · source vide ou illisible refusée · aucune suppression déduite d’une panne · plafond de suppressions (plus de la moitié de la destination, dès 10).",
            ),
        ]
        .spacing(14)
        .into()
    }

    fn summary_view<'a>(&'a self, p: &Palette) -> El<'a, Message> {
        let d = &self.draft;
        let groups = self.groups.iter().map(|g| {
            let on = d.group.trim() == g;
            button(ui::colored(
                if on { p.bg } else { p.muted },
                g.clone(),
                12.0,
            ))
            .padding(Padding::from([4, 12]))
            .style(theme::pick_chip(*p, on, true))
            .on_press(Message::Wizard(WizardMsg::Group(g.clone())))
            .into()
        });
        let schedule = match d.schedule.trigger() {
            Ok(Some(t)) => crate::draft::trigger_label(&t),
            _ if d.schedule.kind == TriggerKind::Manual => "Lancement manuel".into(),
            _ => "Planification invalide".into(),
        };
        // A chip that jumps back to the step it summarizes.
        let item = |label: String, step: Step, color: Color| -> El<'a, Message> {
            button(ui::colored(color, label, 12.0))
                .padding(Padding::from([4, 10]))
                .style(theme::tint_btn(color, 0.12))
                .on_press(Message::Wizard(WizardMsg::GoTo(step)))
                .into()
        };
        let location = |e: &crate::draft::EndpointDraft| -> String {
            let (local, remote) = e.build().unwrap_or_default();
            model::ellipsize(&model::endpoint_location(&local, remote.as_ref()), 44)
        };
        let mode = d.mode.map_or("—", model::mode_label);
        let chips = vec![
            item(format!("Mode : {mode}"), Step::Locations, p.text),
            item(location(&d.source), Step::Locations, p.text),
            item(location(&d.destination), Step::Locations, p.text),
            item(
                format!("{} filtre(s)", d.filters.count()),
                Step::Filters,
                p.muted,
            ),
            item(
                format!(
                    "Comparaison : {}",
                    model::compare_label(d.effective_compare())
                ),
                Step::Safety,
                p.muted,
            ),
            item(
                if d.allow_delete {
                    "Suppressions protégées autorisées".into()
                } else {
                    "Aucune suppression".into()
                },
                Step::Safety,
                if d.allow_delete { p.danger } else { p.muted },
            ),
            item(
                match (d.trash_enabled, d.trash_limit()) {
                    (true, Ok(limit)) => format!("Corbeille {}", model::bytes(limit)),
                    (true, Err(_)) => "Corbeille : taille invalide".into(),
                    (false, _) => "Corbeille désactivée".into(),
                },
                Step::Safety,
                p.muted,
            ),
            item(schedule, Step::Schedule, p.violet),
        ];
        let card = |content: iced::widget::Column<'a, Message>| -> El<'a, Message> {
            container(content)
                .padding(20)
                .width(Length::Fill)
                .style(theme::card(*p, 0.0, false))
                .into()
        };
        column![
            card(
                column![
                    ui::eyebrow(p.muted, "Nom et groupe"),
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
                    row(groups).spacing(6).wrap(),
                ]
                .spacing(12)
            ),
            card(
                column![
                    ui::eyebrow(
                        p.muted,
                        "Récapitulatif · touchez une pastille pour modifier"
                    ),
                    row(chips).spacing(6).wrap(),
                ]
                .spacing(12)
            ),
        ]
        .spacing(14)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_edits_invalidate_consent_but_other_edits_keep_it() {
        let now = Instant::now();
        let mut w = Wizard::open(ProfileDraft::new(""), vec![], now);
        w.draft.mode = Some(SyncMode::Mirror);
        w.draft.allow_delete = true;
        w.draft.schedule.kind = TriggerKind::Logon;
        let consent = |w: &mut Wizard| {
            w.draft.schedule.approve_writes = true;
            w.draft.schedule.approve_deletions = true;
        };
        let kept =
            |w: &Wizard| w.draft.schedule.approve_writes && w.draft.schedule.approve_deletions;
        consent(&mut w);
        // Given authorized consents, When navigating or editing filters,
        // name, group, trash or comparison, Then they stay checked.
        w.update(WizardMsg::Back, now);
        w.update(WizardMsg::Preset(0), now);
        w.update(
            WizardMsg::PatternInput(FilterList::ExcludeFiles, "*.tmp".into()),
            now,
        );
        w.update(WizardMsg::AddPattern(FilterList::ExcludeFiles), now);
        w.update(WizardMsg::CaseInsensitive(true), now);
        w.update(WizardMsg::Name("new profile name".into()), now);
        w.update(WizardMsg::Group("Travail".into()), now);
        w.update(WizardMsg::Trash(true), now);
        w.update(WizardMsg::Compare(CompareMethod::Content), now);
        assert!(kept(&w));
        // Selecting the same values again is not a change either.
        w.update(WizardMsg::Mode(SyncMode::Mirror), now);
        assert!(kept(&w));
        // When the destination changes, Then both consents are required again.
        w.update(
            WizardMsg::Field(Side::Destination, Field::Path, "other-destination".into()),
            now,
        );
        assert!(!w.draft.schedule.approve_writes && !w.draft.schedule.approve_deletions);
        consent(&mut w);
        w.update(WizardMsg::Mode(SyncMode::Bidirectional), now);
        assert!(!w.draft.schedule.approve_writes);
        // Disabling deletions withdraws only the deletion consent.
        consent(&mut w);
        w.update(WizardMsg::AllowDelete(false), now);
        assert!(w.draft.schedule.approve_writes && !w.draft.schedule.approve_deletions);
    }

    #[test]
    fn steps_are_gated_and_name_is_suggested() {
        let now = Instant::now();
        let mut w = Wizard::open(ProfileDraft::new(""), vec![], now);
        assert!(matches!(w.update(WizardMsg::Next, now), Outcome::None));
        assert_eq!(w.step, Step::Locations);
        assert!(w.error.is_some());
        w.update(WizardMsg::Mode(SyncMode::Bidirectional), now);
        w.update(WizardMsg::GoTo(Step::Summary), now);
        assert_eq!(
            w.step,
            Step::Locations,
            "forward jumps stop at the first invalid step"
        );
        assert!(
            w.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Source — ")),
            "the message says which side is invalid"
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
        assert_eq!(w.step, Step::Locations, "the destination is still missing");
        assert!(
            w.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Destination — ")),
        );
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
