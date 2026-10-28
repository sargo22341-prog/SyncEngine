//! « Synchronisation interrompue » : shown at start-up for every execution
//! stopped by a crash, a power cut or a session end (scheduled ones too).
//! Presentation only: the diagnosis is read from the journal by the facade,
//! which also acknowledges, cleans temporary files and removes the journal.
//! « Reprendre » never replays the journal: it starts a new full simulation.

use crate::app::{App, Dialog, Message, ToastKind};
use crate::backend::{self, Failure};
use crate::icons::{self, Glyph};
use crate::model;
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, row, scrollable};
use iced::{Alignment, Length, Padding, Task};
use sync_orchestrator::{
    FinishedOperation, InterruptedFile, Interruption, InterruptionDetail, JournalOutcome, Resumed,
};
use sync_plan::{Action, PlanSide};

/// Pending interruptions, the first one being shown.
#[derive(Debug, Clone)]
pub(crate) struct InterruptedDialog {
    pub(crate) pending: Vec<Interruption>,
    /// Readable journal of the first one, once asked for (`None`: hidden).
    pub(crate) detail: Option<Option<Box<Result<InterruptionDetail, Failure>>>>,
    /// A choice is being applied.
    pub(crate) working: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum InterruptedMsg {
    Found(Result<Vec<Interruption>, Failure>),
    Detail,
    DetailLoaded(String, Box<Result<InterruptionDetail, Failure>>),
    Resume,
    Resumed(sync_core::ProfileId, Result<Resumed, Failure>),
    Ignore,
    Ignored(Result<Vec<String>, Failure>),
}

fn wrap(msg: InterruptedMsg) -> Message {
    Message::Interrupted(msg)
}

/// Lists interruptions once the facade is open. A command owning the state
/// (`busy`, a scheduled run) means none is interrupted right now.
pub(crate) fn lookup(app: &backend::Shared) -> Task<Message> {
    Task::perform(backend::call(app, |a| a.interruptions()), |r| {
        wrap(InterruptedMsg::Found(r))
    })
}

impl App {
    fn interrupted_state(&mut self) -> Option<&mut InterruptedDialog> {
        match &mut self.dialog {
            Some(Dialog::Interrupted(d)) => Some(d),
            _ => None,
        }
    }

    /// Shows the next pending interruption, or closes the dialog.
    fn next_interruption(&mut self) {
        let Some(d) = self.interrupted_state() else {
            return;
        };
        d.working = false;
        d.detail = None;
        if !d.pending.is_empty() {
            d.pending.remove(0);
        }
        if d.pending.is_empty() {
            self.close_dialog();
        }
    }

    pub(crate) fn interrupted(&mut self, msg: InterruptedMsg) -> Task<Message> {
        match msg {
            InterruptedMsg::Found(Ok(list)) => {
                if !list.is_empty() {
                    self.open_dialog(Dialog::Interrupted(Box::new(InterruptedDialog {
                        pending: list,
                        detail: None,
                        working: false,
                    })));
                }
                Task::none()
            }
            InterruptedMsg::Found(Err(e)) => {
                if e.code != "busy" {
                    self.fail(&e);
                }
                Task::none()
            }
            InterruptedMsg::Detail => {
                let Some(app) = self.backend.clone() else {
                    return Task::none();
                };
                let Some(d) = self.interrupted_state() else {
                    return Task::none();
                };
                if d.detail.is_some() {
                    d.detail = None;
                    return Task::none();
                }
                let Some(first) = d.pending.first().cloned() else {
                    return Task::none();
                };
                d.detail = Some(None);
                let key = first.execution.clone();
                Task::perform(
                    backend::call(&app, move |a| {
                        a.interruption_detail(&first.profile, &first.execution)
                    }),
                    move |r| wrap(InterruptedMsg::DetailLoaded(key.clone(), Box::new(r))),
                )
            }
            InterruptedMsg::DetailLoaded(execution, result) => {
                if let Some(d) = self.interrupted_state()
                    && d.pending.first().is_some_and(|i| i.execution == execution)
                    && d.detail.is_some()
                {
                    d.detail = Some(Some(result));
                }
                Task::none()
            }
            InterruptedMsg::Resume | InterruptedMsg::Ignore => {
                let resume = matches!(msg, InterruptedMsg::Resume);
                let busy = self.busy();
                let Some(app) = self.backend.clone() else {
                    return Task::none();
                };
                let Some(d) = self.interrupted_state() else {
                    return Task::none();
                };
                let Some(first) = d.pending.first().cloned() else {
                    return Task::none();
                };
                if d.working || busy {
                    return Task::none();
                }
                d.working = true;
                let id = first.profile;
                if resume {
                    let key = id.clone();
                    Task::perform(
                        backend::call(&app, move |a| a.resume_interrupted(&id)),
                        move |r| wrap(InterruptedMsg::Resumed(key.clone(), r)),
                    )
                } else {
                    Task::perform(
                        backend::call(&app, move |a| a.acknowledge_interrupted(&id, true)),
                        |r| wrap(InterruptedMsg::Ignored(r)),
                    )
                }
            }
            InterruptedMsg::Resumed(id, result) => match result {
                Ok(resumed) => {
                    self.report_resumed(&resumed);
                    self.next_interruption();
                    self.preview = None;
                    Task::batch([self.refresh(), Task::done(Message::Simulate(id))])
                }
                Err(e) => {
                    if let Some(d) = self.interrupted_state() {
                        d.working = false;
                    }
                    self.fail(&e);
                    Task::none()
                }
            },
            InterruptedMsg::Ignored(result) => match result {
                Ok(warnings) => {
                    self.toast(
                        ToastKind::Info,
                        "Interruption ignorée",
                        "L’exécution reste signalée en échec ; la prochaine simulation repart d’une nouvelle analyse.",
                    );
                    if !warnings.is_empty() {
                        self.toast(
                            ToastKind::Warning,
                            "Nettoyage reporté",
                            warnings.join(" ; "),
                        );
                    }
                    self.next_interruption();
                    self.refresh()
                }
                Err(e) => {
                    if let Some(d) = self.interrupted_state() {
                        d.working = false;
                    }
                    self.fail(&e);
                    Task::none()
                }
            },
        }
    }

    fn report_resumed(&mut self, resumed: &Resumed) {
        let removed = match resumed.removed.len() {
            0 => "Aucun fichier temporaire à retirer.".to_owned(),
            n => format!(
                "{} fichier(s) temporaire(s) retiré(s).",
                model::count(n as u64)
            ),
        };
        self.toast(
            ToastKind::Info,
            "Reprise : nouvelle simulation",
            format!("{removed} Les fichiers déjà copiés apparaîtront identiques et seront sautés."),
        );
        if !resumed.leftovers.is_empty() {
            self.toast(
                ToastKind::Warning,
                "Fichiers temporaires à vérifier",
                model::ellipsize(&resumed.leftovers.join(" ; "), 400),
            );
        }
        if !resumed.warnings.is_empty() {
            self.toast(
                ToastKind::Warning,
                "Nettoyage reporté",
                model::ellipsize(&resumed.warnings.join(" ; "), 400),
            );
        }
    }

    pub(crate) fn interrupted_dialog<'a>(
        &'a self,
        p: &Palette,
        d: &'a InterruptedDialog,
    ) -> El<'a, Message> {
        let Some(first) = d.pending.first() else {
            return Space::new().into();
        };
        let name = self
            .profile(&first.profile)
            .map_or_else(|| first.profile.to_string(), |pr| pr.name.clone());
        let date = first
            .started
            .map_or_else(|| "date inconnue".to_owned(), model::utc_label);
        let head = row![
            ui::glyph_box(Glyph::Alert, p.warning, 40.0),
            column![
                ui::title(p, "Synchronisation interrompue", 19.0),
                ui::caption(p, format!("« {} » — {date}", model::ellipsize(&name, 60))),
            ]
            .spacing(2),
            ui::fill(),
            if d.pending.len() > 1 {
                ui::caption(p, format!("1 sur {}", d.pending.len()))
            } else {
                Space::new().into()
            },
            button(icons::icon(Glyph::Close, 14.0, p.faint))
                .padding(6)
                .style(theme::btn(*p, Tone::Ghost))
                .on_press(Message::CloseDialog),
        ]
        .spacing(14)
        .align_y(Alignment::Center);
        let facts: El<'a, Message> = match &first.journal_error {
            Some(e) => ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Journal illisible : avancement inconnu",
                e.clone(),
            ),
            None => column![
                ui::strong(
                    p.text,
                    format!(
                        "{} opération(s) terminée(s) sur {}",
                        model::count(first.finished),
                        model::count(first.planned)
                    ),
                    15.0
                ),
                ui::colored(
                    if first.in_flight > 0 {
                        p.warning
                    } else {
                        p.muted
                    },
                    format!(
                        "{} fichier(s) en cours au moment de l’arrêt (état inconnu)",
                        model::count(first.in_flight)
                    ),
                    14.0
                ),
            ]
            .spacing(4)
            .into(),
        };
        let mut col = column![
            head,
            facts,
            ui::muted(
                p,
                "Rien n’est rejoué. « Reprendre » retire les fichiers temporaires de la copie interrompue puis lance une nouvelle simulation complète : les fichiers déjà copiés apparaîtront identiques et seront sautés. Le journal est supprimé une fois l’interruption acquittée."
            ),
        ]
        .spacing(14);
        if let Some(detail) = &d.detail {
            col = col.push(match detail.as_deref() {
                None => ui::caption(p, "Lecture du journal…"),
                Some(Err(e)) => {
                    ui::callout(p, p.danger, Glyph::Alert, e.title(), e.message.clone())
                }
                Some(Ok(detail)) => detail_view(p, detail, self.window.height),
            });
        }
        let ready = !d.working && !self.busy();
        col.push(
            row![
                ui::action(
                    p,
                    Tone::Ghost,
                    Some(Glyph::Eye),
                    if d.detail.is_some() {
                        "Masquer le détail"
                    } else {
                        "Voir le détail"
                    },
                    Some(wrap(InterruptedMsg::Detail))
                ),
                ui::fill(),
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Check),
                    "J’ai vérifié, ignorer",
                    ready.then(|| wrap(InterruptedMsg::Ignore))
                ),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Refresh),
                    if d.working { "Reprise…" } else { "Reprendre" },
                    ready.then(|| wrap(InterruptedMsg::Resume))
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .into()
    }
}

fn action_text(action: Action) -> &'static str {
    match action {
        Action::Copy => "copie",
        Action::Replace => "remplacement",
        Action::Delete => "suppression",
        _ => "opération",
    }
}

fn side_text(side: PlanSide) -> &'static str {
    match side {
        PlanSide::Source => "côté source",
        PlanSide::Destination => "côté destination",
    }
}

fn outcome_text(outcome: JournalOutcome) -> &'static str {
    match outcome {
        JournalOutcome::Done => "terminé",
        JournalOutcome::NoAction => "rien à faire",
        JournalOutcome::Refused => "refusé",
        JournalOutcome::Failed => "en échec",
        JournalOutcome::Cancelled => "annulé",
        JournalOutcome::NotStarted => "non commencé",
        _ => "inconnu",
    }
}

fn unfinished_row<'a>(p: &Palette, file: &'a InterruptedFile) -> El<'a, Message> {
    let mut lines = column![
        row![
            icons::icon(Glyph::Alert, 14.0, p.warning),
            container(ui::mono(p.text, model::ellipsize(&file.path, 70), 12.0)).width(Length::Fill),
            ui::caption(
                p,
                format!("{} · {}", action_text(file.action), side_text(file.target))
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
    ]
    .spacing(2);
    if let Some(staging) = &file.staging {
        lines = lines.push(ui::caption(
            p,
            format!(
                "Fichier temporaire possible : {}",
                model::ellipsize(staging, 80)
            ),
        ));
    }
    if let Some(version) = &file.version {
        lines = lines.push(ui::caption(
            p,
            format!(
                "Ancienne version gardée : {}",
                model::ellipsize(version, 80)
            ),
        ));
    }
    container(lines)
        .padding(Padding::from([8, 10]))
        .width(Length::Fill)
        .style(theme::outline(*p, p.warning, 0.6))
        .into()
}

fn finished_row<'a>(p: &Palette, op: &'a FinishedOperation) -> El<'a, Message> {
    let failed = matches!(op.outcome, JournalOutcome::Failed | JournalOutcome::Refused);
    let mut line = row![
        container(ui::mono(p.text, model::ellipsize(&op.path, 64), 12.0)).width(Length::Fill),
        ui::caption(p, action_text(op.action)),
        ui::colored(
            if failed { p.danger } else { p.muted },
            outcome_text(op.outcome),
            12.0
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if op.bytes > 0 {
        line = line.push(ui::caption(p, model::bytes(op.bytes)));
    }
    let mut col = column![line].spacing(2);
    if let Some(detail) = op.detail.as_ref().filter(|_| failed) {
        col = col.push(ui::caption(p, model::ellipsize(detail, 160)));
    }
    col.into()
}

fn detail_view<'a>(p: &Palette, d: &'a InterruptionDetail, height: f32) -> El<'a, Message> {
    let mut col = column![].spacing(10);
    if let (Some(source), Some(destination)) = (&d.source, &d.destination) {
        col = col.push(
            column![
                ui::caption(p, format!("Source : {}", model::ellipsize(source, 90))),
                ui::caption(
                    p,
                    format!("Destination : {}", model::ellipsize(destination, 90))
                ),
            ]
            .spacing(2),
        );
    }
    let t = d.totals;
    col = col.push(ui::caption(
        p,
        format!(
            "{} réalisée(s) · {} refusée(s) · {} en échec · {} annulée(s) · {} validés{}",
            model::count(t.done),
            model::count(t.refused),
            model::count(t.failed),
            model::count(t.cancelled),
            model::bytes(t.bytes),
            d.last_record
                .map(|at| format!(" · dernier enregistrement {}", model::utc_label(at)))
                .unwrap_or_default()
        ),
    ));
    if d.truncated_tail {
        col = col.push(ui::caption(
            p,
            "La dernière ligne du journal, coupée par l’arrêt, est ignorée.",
        ));
    }
    col = col.push(ui::eyebrow(p.warning, "Commencés mais jamais terminés"));
    if d.in_flight.is_empty() {
        col = col.push(ui::caption(
            p,
            "Aucun : l’arrêt est survenu entre deux opérations.",
        ));
    }
    for file in &d.in_flight {
        col = col.push(unfinished_row(p, file));
    }
    if !d.problems.is_empty() {
        col = col.push(ui::eyebrow(p.danger, "Refus et échecs"));
        for op in &d.problems {
            col = col.push(finished_row(p, op));
        }
    }
    if !d.recent.is_empty() {
        col = col.push(ui::eyebrow(
            p.muted,
            "Dernières opérations terminées (les plus récentes en bas)",
        ));
        for op in &d.recent {
            col = col.push(finished_row(p, op));
        }
    }
    scrollable(col)
        .style(theme::scroll(*p))
        .height(Length::Fixed((height - 380.0).clamp(140.0, 460.0)))
        .into()
}
