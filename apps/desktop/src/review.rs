//! Modales de la carte « À traiter » de l’aperçu des profils :
//! « exécutions à vérifier » (problèmes de la dernière exécution et
//! solutions) et « autorisations à renouveler » (une ou plusieurs
//! planifications).
//!
//! Présentation seulement : les problèmes viennent de l’historique de
//! l’exécution, l’exclusion passe par le dialogue d’exclusion existant et le
//! renouvellement réenregistre la planification avec ses réglages actuels
//! (`schedule_save`), comme le fait le déplacement de groupe.

use crate::app::{App, Dialog, Message, ToastKind};
use crate::backend::{self, Batch, Failure, HistoryPage};
use crate::exclusion::ExclusionMsg;
use crate::icons::Glyph;
use crate::model::{self, HistoryRow, Outcome};
use crate::overlays::dialog_head;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{button, checkbox, column, container, row, scrollable};
use iced::{Alignment, Length, Padding, Task};
use sync_core::ProfileId;
use sync_orchestrator::HistoryFilter;

/// Problèmes affichés au plus ; le reste est dans l’historique.
const SHOWN_PROBLEMS: usize = 40;

/// « Exécutions à vérifier ».
#[derive(Debug, Clone)]
pub(crate) struct ReviewDialog {
    /// Profils dont la dernière exécution n’est pas un succès.
    pub(crate) profiles: Vec<ProfileId>,
    pub(crate) current: usize,
    /// `None` : chargement en cours.
    pub(crate) loaded: Option<Result<(usize, Vec<HistoryRow>), Failure>>,
}

#[derive(Debug, Clone)]
pub(crate) enum ReviewMsg {
    Open,
    Select(usize),
    Loaded(ProfileId, Box<Result<HistoryPage, Failure>>),
    Exclude { path: String, directory: bool },
    Resimulate,
    ShowRun,
}

/// « Autorisations à renouveler ».
#[derive(Debug, Clone)]
pub(crate) struct RenewDialog {
    pub(crate) items: Vec<(ProfileId, bool)>,
    pub(crate) saving: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum RenewMsg {
    Open,
    Toggle(usize, bool),
    All(bool),
    Confirm,
    Done(Box<Batch>),
}

impl App {
    /// Profils dont la dernière exécution demande un examen.
    pub(crate) fn profiles_to_review(&self) -> Vec<ProfileId> {
        self.ws
            .profiles
            .iter()
            .filter(|pr| {
                self.last_run(&pr.id)
                    .is_some_and(|r| r.outcome != Outcome::Success)
            })
            .map(|pr| pr.id.clone())
            .collect()
    }

    /// Profils planifiés dont l’autorisation n’est plus valable.
    pub(crate) fn schedules_to_renew(&self) -> Vec<ProfileId> {
        self.ws
            .schedules
            .iter()
            .filter(|s| self.schedule_valid(&s.schedule.approved.id) != Some(true))
            .filter(|s| self.profile(&s.schedule.approved.id).is_some())
            .map(|s| s.schedule.approved.id.clone())
            .collect()
    }

    fn profile_name(&self, id: &ProfileId) -> String {
        self.profile(id)
            .map_or_else(|| id.to_string(), |pr| pr.name.clone())
    }

    fn load_review(&mut self) -> Task<Message> {
        let (Some(app), Some(Dialog::Review(d))) = (self.backend.clone(), &mut self.dialog) else {
            return Task::none();
        };
        d.loaded = None;
        let Some(id) = d.profiles.get(d.current).cloned() else {
            return Task::none();
        };
        let Some(execution) = self.last_run(&id).map(|r| r.execution.clone()) else {
            if let Some(Dialog::Review(d)) = &mut self.dialog {
                d.loaded = Some(Ok((0, Vec::new())));
            }
            return Task::none();
        };
        let key = id.clone();
        Task::perform(
            backend::history_page(
                &app,
                id,
                execution,
                0,
                HistoryFilter::Problems,
                String::new(),
                0,
            ),
            move |r| Message::Review(ReviewMsg::Loaded(key.clone(), Box::new(r))),
        )
    }

    pub(crate) fn review(&mut self, msg: ReviewMsg) -> Task<Message> {
        match msg {
            ReviewMsg::Open => {
                let profiles = self.profiles_to_review();
                if profiles.is_empty() {
                    return Task::none();
                }
                self.open_dialog(Dialog::Review(Box::new(ReviewDialog {
                    profiles,
                    current: 0,
                    loaded: None,
                })));
                self.load_review()
            }
            ReviewMsg::Select(index) => {
                if let Some(Dialog::Review(d)) = &mut self.dialog
                    && index < d.profiles.len()
                {
                    d.current = index;
                    return self.load_review();
                }
                Task::none()
            }
            ReviewMsg::Loaded(id, result) => {
                if let Some(Dialog::Review(d)) = &mut self.dialog
                    && d.profiles.get(d.current) == Some(&id)
                {
                    d.loaded = Some((*result).map(|page| {
                        let total = page.total;
                        (total, page.rows.into_iter().take(SHOWN_PROBLEMS).collect())
                    }));
                }
                Task::none()
            }
            ReviewMsg::Exclude { path, directory } => {
                let Some(id) = self.review_target() else {
                    return Task::none();
                };
                self.selected = Some(id);
                self.exclusion(ExclusionMsg::Ask { path, directory })
            }
            ReviewMsg::Resimulate => {
                let Some(id) = self.review_target() else {
                    return Task::none();
                };
                self.close_dialog();
                Task::done(Message::Simulate(id))
            }
            ReviewMsg::ShowRun => {
                let Some(id) = self.review_target() else {
                    return Task::none();
                };
                let Some(execution) = self.last_run(&id).map(|r| r.execution.clone()) else {
                    return Task::none();
                };
                self.close_dialog();
                Task::done(Message::ShowRun(id, execution))
            }
        }
    }

    fn review_target(&self) -> Option<ProfileId> {
        match &self.dialog {
            Some(Dialog::Review(d)) => d.profiles.get(d.current).cloned(),
            _ => None,
        }
    }

    pub(crate) fn renew(&mut self, msg: RenewMsg) -> Task<Message> {
        match msg {
            RenewMsg::Open => {
                let items: Vec<(ProfileId, bool)> = self
                    .schedules_to_renew()
                    .into_iter()
                    .map(|id| (id, true))
                    .collect();
                if !items.is_empty() {
                    self.open_dialog(Dialog::Renew(Box::new(RenewDialog {
                        items,
                        saving: false,
                        error: None,
                    })));
                }
                Task::none()
            }
            RenewMsg::Toggle(index, on) => {
                if let Some(Dialog::Renew(d)) = &mut self.dialog
                    && let Some(item) = d.items.get_mut(index)
                {
                    item.1 = on;
                }
                Task::none()
            }
            RenewMsg::All(on) => {
                if let Some(Dialog::Renew(d)) = &mut self.dialog {
                    for item in &mut d.items {
                        item.1 = on;
                    }
                }
                Task::none()
            }
            RenewMsg::Confirm => self.confirm_renew(),
            RenewMsg::Done(result) => {
                match *result {
                    Ok(names) => {
                        self.close_dialog();
                        self.toast(
                            ToastKind::Success,
                            "Autorisations renouvelées",
                            names.join(", "),
                        );
                    }
                    Err((done, e)) => {
                        let message = if done.is_empty() {
                            e.message.clone()
                        } else {
                            format!("{} Déjà renouvelés : {}.", e.message, done.join(", "))
                        };
                        if let Some(Dialog::Renew(d)) = &mut self.dialog {
                            d.error = Some(message);
                            d.saving = false;
                        } else {
                            self.toast(ToastKind::Error, e.title(), message);
                        }
                    }
                }
                self.refresh()
            }
        }
    }

    fn confirm_renew(&mut self) -> Task<Message> {
        let Some(app) = self.backend.clone() else {
            return Task::none();
        };
        let Some(Dialog::Renew(d)) = &self.dialog else {
            return Task::none();
        };
        if d.saving {
            return Task::none();
        }
        // (id, nom, déclencheur, rattrapage, suppressions, fenêtre)
        let mut jobs = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        for (id, _) in d.items.iter().filter(|(_, on)| *on) {
            let (Some(profile), Some(s)) = (self.profile(id), self.schedule_of(id)) else {
                continue;
            };
            if s.running_slot.is_some() {
                skipped.push(profile.name.clone());
                continue;
            }
            jobs.push((
                id.clone(),
                profile.name.clone(),
                s.schedule.trigger.clone(),
                s.schedule.missed,
                s.schedule.allow_deletions && profile.allow_delete,
                s.schedule.show_window,
            ));
        }
        for name in &skipped {
            self.toast_later_busy(name);
        }
        if jobs.is_empty() {
            if let Some(Dialog::Renew(d)) = &mut self.dialog {
                d.error = Some("Aucune planification renouvelable n’est cochée.".into());
            }
            return Task::none();
        }
        if let Some(Dialog::Renew(d)) = &mut self.dialog {
            d.saving = true;
            d.error = None;
        }
        let task = backend::blocking(move || {
            crate::native::prepare_agent(app.state_directory())?;
            let mut done = Vec::new();
            for (id, name, trigger, missed, deletions, window) in jobs {
                if let Err(e) = app.schedule_save(id, trigger, missed, true, deletions, window) {
                    return Ok(Err((
                        done,
                        Failure::new(&e.code, format!("« {name} » : {}", e.message)),
                    )));
                }
                done.push(name);
            }
            Ok(Ok(done))
        });
        Task::perform(task, |r: Result<Batch, Failure>| {
            Message::Renew(RenewMsg::Done(Box::new(
                r.unwrap_or_else(|e| Err((Vec::new(), e))),
            )))
        })
    }

    fn toast_later_busy(&mut self, name: &str) {
        self.toast(
            ToastKind::Info,
            "Planification ignorée",
            format!("« {name} » a une tâche en cours ou interrompue : renouvelez-la après."),
        );
    }

    pub(crate) fn review_dialog<'a>(&'a self, p: &Palette, d: &'a ReviewDialog) -> El<'a, Message> {
        let id = d.profiles.get(d.current);
        let name = id.map(|id| self.profile_name(id)).unwrap_or_default();
        let run = id.and_then(|id| self.last_run(id));
        let mut col = column![dialog_head(
            p,
            Glyph::Alert,
            p.warning,
            "Exécutions à vérifier"
        )]
        .spacing(14);
        if d.profiles.len() > 1 {
            let mut tabs = column![].spacing(4);
            for (index, other) in d.profiles.iter().enumerate() {
                let chosen = index == d.current;
                tabs = tabs.push(
                    button(ui::colored(
                        if chosen { p.text } else { p.muted },
                        model::ellipsize(&self.profile_name(other), 60),
                        13.0,
                    ))
                    .padding(Padding::from([6, 10]))
                    .width(Length::Fill)
                    .style(theme::row_btn(*p, chosen))
                    .on_press(Message::Review(ReviewMsg::Select(index))),
                );
            }
            col = col.push(
                container(scrollable(tabs).style(theme::scroll(*p)))
                    .max_height(130.0)
                    .width(Length::Fill),
            );
        }
        col = col.push(ui::strong(
            p.text,
            format!("« {} »", model::ellipsize(&name, 70)),
            15.0,
        ));
        if let Some(run) = run {
            col = col.push(ui::muted(p, run.label.clone()));
            if let Some(message) = &run.message {
                col = col.push(ui::callout(
                    p,
                    p.warning,
                    Glyph::Info,
                    "Diagnostic",
                    message.clone(),
                ));
            }
        }
        match &d.loaded {
            None => col = col.push(ui::caption(p, "Lecture des problèmes…")),
            Some(Err(e)) => {
                col = col.push(ui::callout(
                    p,
                    p.danger,
                    Glyph::Alert,
                    e.title(),
                    e.message.clone(),
                ));
            }
            Some(Ok((total, rows))) => {
                if rows.is_empty() {
                    col = col.push(ui::caption(
                        p,
                        "Aucune opération en cause n’est enregistrée pour cette exécution.",
                    ));
                } else {
                    let mut list = column![].spacing(6);
                    for row_ in rows {
                        list = list.push(problem_row(p, row_));
                    }
                    col = col.push(
                        container(scrollable(list).style(theme::scroll(*p)))
                            .max_height((self.window.height - 460.0).clamp(120.0, 340.0)),
                    );
                    if *total > rows.len() {
                        col = col.push(ui::caption(
                            p,
                            format!(
                                "{} sur {} problèmes affichés ; le reste est dans l’historique.",
                                rows.len(),
                                model::count(*total as u64)
                            ),
                        ));
                    }
                }
            }
        }
        col = col.push(ui::muted(
            p,
            "Fichier disparu depuis : excluez-le du profil, ou relancez une simulation, qui repart d’une analyse neuve et ne le prévoit plus. Fichier refusé ou modifié pendant la copie : relancez une simulation pour resynchroniser.",
        ));
        let idle = !self.busy();
        col.push(
            row![
                ui::action(
                    p,
                    Tone::Ghost,
                    Some(Glyph::History),
                    "Voir dans l’historique",
                    Some(Message::Review(ReviewMsg::ShowRun)),
                ),
                ui::fill(),
                ui::action(p, Tone::Ghost, None, "Fermer", Some(Message::CloseDialog)),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Refresh),
                    "Relancer une simulation",
                    idle.then_some(Message::Review(ReviewMsg::Resimulate)),
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .into()
    }

    pub(crate) fn renew_dialog<'a>(&'a self, p: &Palette, d: &'a RenewDialog) -> El<'a, Message> {
        let mut col = column![dialog_head(
            p,
            Glyph::Clock,
            p.warning,
            "Autorisations à renouveler"
        )]
        .spacing(14);
        let chosen = d.items.iter().filter(|(_, on)| *on).count();
        if let [(id, _)] = d.items.as_slice() {
            col = col.push(ui::muted(
                p,
                format!(
                    "Le profil « {} » a changé depuis l’autorisation de sa planification : elle ne se lance plus tant qu’elle n’est pas renouvelée. Le renouvellement garde l’horaire et les réglages actuels et autorise de nouveau les écritures automatiques.",
                    model::ellipsize(&self.profile_name(id), 60)
                ),
            ));
        } else {
            col = col.push(ui::muted(
                p,
                "Ces profils ont changé depuis l’autorisation de leur planification : ils ne se lancent plus tant qu’elle n’est pas renouvelée. Cochez ceux à renouveler (horaire et réglages actuels conservés).",
            ));
            col = col.push(
                row![
                    ui::action_sm(
                        p,
                        Tone::Ghost,
                        None,
                        "Tout cocher",
                        Some(Message::Renew(RenewMsg::All(true)))
                    ),
                    ui::action_sm(
                        p,
                        Tone::Ghost,
                        None,
                        "Tout décocher",
                        Some(Message::Renew(RenewMsg::All(false)))
                    ),
                ]
                .spacing(8),
            );
            let mut list = column![].spacing(6);
            for (index, (id, on)) in d.items.iter().enumerate() {
                list = list.push(
                    checkbox(*on)
                        .label(model::ellipsize(&self.profile_name(id), 70))
                        .on_toggle(move |v| Message::Renew(RenewMsg::Toggle(index, v)))
                        .text_size(13)
                        .style(theme::check(*p)),
                );
            }
            col = col.push(container(scrollable(list).style(theme::scroll(*p))).max_height(260.0));
        }
        if let Some(error) = &d.error {
            col = col.push(ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Renouvellement impossible",
                error.clone(),
            ));
        }
        let label = if d.items.len() == 1 {
            "Renouveler la planification".to_owned()
        } else {
            format!("Renouveler ({chosen})")
        };
        col.push(
            row![
                ui::fill(),
                ui::action(p, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Check),
                    label,
                    (!d.saving && chosen > 0 && !self.busy())
                        .then_some(Message::Renew(RenewMsg::Confirm)),
                ),
            ]
            .spacing(10),
        )
        .into()
    }
}

/// Une opération en cause, avec le raccourci d’exclusion.
fn problem_row<'a>(p: &Palette, r: &'a HistoryRow) -> El<'a, Message> {
    let path = r.op.as_ref().map_or("Chemin inconnu", |o| o.path.as_str());
    let exclude: El<'a, Message> = match &r.op {
        Some(op) if !op.path.is_empty() => ui::action_sm(
            p,
            Tone::Secondary,
            Some(Glyph::Shield),
            "Exclure…",
            Some(Message::Review(ReviewMsg::Exclude {
                path: op.path.clone(),
                directory: op.kind == "directory",
            })),
        ),
        _ => iced::widget::Space::new().into(),
    };
    container(
        row![
            column![
                ui::mono(p.text, model::ellipsize(path, 70), 12.0),
                ui::caption(p, model::ellipsize(&r.detail, 140)),
            ]
            .spacing(2)
            .width(Length::Fill),
            exclude,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([8, 10]))
    .width(Length::Fill)
    .style(theme::outline(*p, p.warning, 0.5))
    .into()
}
