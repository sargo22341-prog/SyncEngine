//! « Synchroniser tout le groupe » : the profiles of a group are simulated
//! then executed one after the other, after a single confirmation. Each run
//! executes exactly the plan of its own simulation; nothing is decided here
//! that the single-profile path would not decide. A plan that would need a
//! consent the confirmation did not give (deletions, permanent loss beyond
//! the trash) is left aside without any write, and the chain goes on.

use crate::app::{App, Dialog, JobKind, Message, Nav, ToastKind};
use crate::backend::{self, Executed, Failure};
use crate::icons::{self, Glyph};
use crate::model::{self, Outcome, PreviewView};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{Space, checkbox, column, container, row, scrollable};
use iced::{Alignment, Length, Task};
use std::collections::VecDeque;
use sync_core::ProfileId;
use sync_orchestrator::DeletionCeiling;

/// Chain in progress.
#[derive(Debug, Clone)]
pub(crate) struct GroupRun {
    pub(crate) group: String,
    queue: VecDeque<ProfileId>,
    current: Option<ProfileId>,
    allow_deletions: bool,
    results: Vec<GroupResult>,
}

/// What happened to one profile of the group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupResult {
    pub(crate) name: String,
    pub(crate) step: GroupStep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupStep {
    /// Executed: outcome and its label (« Réussi »…).
    Executed(Outcome, String),
    /// The simulation found nothing to change.
    UpToDate,
    /// Left aside before any write, with the reason.
    Skipped(String),
    Failed(String),
    /// Not reached: the chain was cancelled before.
    NotStarted,
}

/// Decision on a simulated plan, before any write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Next {
    Execute { confirm_delete: bool },
    UpToDate,
    Skip(String),
}

/// Same consents as the execution dialog, given once for the whole group.
pub(crate) fn decide(view: &PreviewView, allow_deletions: bool) -> Next {
    let s = &view.summary;
    if s.mutations() == 0 {
        return Next::UpToDate;
    }
    if s.trash.overflow {
        return Next::Skip(s.trash.overflow_message());
    }
    if s.delete > 0 && !allow_deletions {
        return Next::Skip(format!(
            "{} suppression(s) prévue(s), non autorisée(s) pour ce lancement",
            model::count(s.delete)
        ));
    }
    Next::Execute {
        confirm_delete: s.delete > 0,
    }
}

impl App {
    /// Profiles of a group, in the order of the list.
    fn group_members(&self, group: &str) -> Vec<ProfileId> {
        let mut list: Vec<_> = self
            .ws
            .profiles
            .iter()
            .filter(|p| p.group == group)
            .collect();
        list.sort_by_key(|p| p.name.to_lowercase());
        list.into_iter().map(|p| p.id.clone()).collect()
    }

    pub(crate) fn ask_group_run(&mut self, group: String) -> Task<Message> {
        let profiles = self.group_members(&group);
        if profiles.is_empty() {
            self.toast(
                ToastKind::Info,
                "Groupe vide",
                "Ajoutez d’abord des profils à ce groupe.",
            );
            return Task::none();
        }
        self.open_dialog(Dialog::GroupRun {
            group,
            profiles,
            allow_deletions: false,
        });
        Task::none()
    }

    pub(crate) fn start_group_run(&mut self) -> Task<Message> {
        let Some(Dialog::GroupRun {
            group,
            profiles,
            allow_deletions,
        }) = self.dialog.clone()
        else {
            return Task::none();
        };
        if self.busy() {
            return Task::none();
        }
        self.close_dialog();
        self.preview = None;
        self.last_run = None;
        self.group_run = Some(GroupRun {
            group,
            queue: profiles.into(),
            current: None,
            allow_deletions,
            results: Vec::new(),
        });
        self.go(Nav::Activity);
        self.group_next()
    }

    /// Profile the chain is waiting for, if any.
    pub(crate) fn group_waits_for(&self, id: &ProfileId) -> bool {
        self.group_run
            .as_ref()
            .is_some_and(|g| g.current.as_ref() == Some(id))
    }

    fn group_record(&mut self, step: GroupStep) {
        let Some(run) = &mut self.group_run else {
            return;
        };
        if let Some(id) = run.current.take() {
            let name = self
                .ws
                .profiles
                .iter()
                .find(|p| p.id == id)
                .map_or_else(|| id.as_str().to_owned(), |p| p.name.clone());
            run.results.push(GroupResult { name, step });
        }
    }

    /// Simulates the next profile, or ends the chain.
    fn group_next(&mut self) -> Task<Message> {
        let Some(app) = self.backend.clone() else {
            return self.group_finish(false);
        };
        let next = loop {
            let Some(run) = &mut self.group_run else {
                return Task::none();
            };
            let Some(id) = run.queue.pop_front() else {
                return self.group_finish(false);
            };
            // A profile removed meanwhile is simply not run.
            if self.profile(&id).is_some() {
                break id;
            }
        };
        if let Some(run) = &mut self.group_run {
            run.current = Some(next.clone());
        }
        self.selected = Some(next.clone());
        self.start_job(JobKind::Preview, next.clone(), None);
        Task::perform(
            backend::preview(&app, next.clone(), DeletionCeiling::Guarded),
            move |r| Message::Previewed(next.clone(), r),
        )
    }

    /// Simulation of the current profile done: execute, skip or stop.
    pub(crate) fn group_previewed(
        &mut self,
        result: Result<PreviewView, Failure>,
    ) -> Task<Message> {
        let cancelled = self.job.take().is_some_and(|j| j.cancel_requested);
        let refresh = self.finish_job_tasks();
        if cancelled {
            self.group_record(GroupStep::NotStarted);
            return Task::batch([refresh, self.group_finish(true)]);
        }
        let (view, allow) = match (result, &self.group_run) {
            (Ok(view), Some(run)) => (view, run.allow_deletions),
            (Err(e), _) => {
                self.group_record(GroupStep::Failed(format!("Simulation : {}", e.message)));
                return Task::batch([refresh, self.group_next()]);
            }
            (Ok(_), None) => return refresh,
        };
        match decide(&view, allow) {
            Next::UpToDate => {
                self.group_record(GroupStep::UpToDate);
                Task::batch([refresh, self.group_next()])
            }
            Next::Skip(reason) => {
                self.group_record(GroupStep::Skipped(reason));
                Task::batch([refresh, self.group_next()])
            }
            Next::Execute { confirm_delete } => {
                let (Some(app), Some(id)) = (
                    self.backend.clone(),
                    self.group_run.as_ref().and_then(|g| g.current.clone()),
                ) else {
                    return refresh;
                };
                let s = view.summary;
                self.start_job(JobKind::Execute, id.clone(), Some((s.mutations(), s.bytes)));
                Task::batch([
                    refresh,
                    Task::perform(
                        backend::execute(&app, id, view.plan_id, confirm_delete, false),
                        Message::Executed,
                    ),
                ])
            }
        }
    }

    /// Execution of the current profile done (already reported as usual).
    pub(crate) fn group_executed(
        &mut self,
        result: &Result<Executed, Failure>,
        cancelled: bool,
    ) -> Task<Message> {
        self.group_record(match result {
            Ok(done) => GroupStep::Executed(done.outcome, done.label.clone()),
            Err(e) => GroupStep::Failed(e.message.clone()),
        });
        if cancelled {
            self.group_finish(true)
        } else {
            self.group_next()
        }
    }

    fn group_finish(&mut self, cancelled: bool) -> Task<Message> {
        let Some(mut run) = self.group_run.take() else {
            return Task::none();
        };
        for id in run.queue.drain(..) {
            if let Some(p) = self.profile(&id) {
                run.results.push(GroupResult {
                    name: p.name.clone(),
                    step: GroupStep::NotStarted,
                });
            }
        }
        let problems = run
            .results
            .iter()
            .filter(|r| {
                !matches!(
                    r.step,
                    GroupStep::UpToDate | GroupStep::Executed(Outcome::Success, _)
                )
            })
            .count();
        self.toast(
            if problems == 0 {
                ToastKind::Success
            } else {
                ToastKind::Warning
            },
            format!("Groupe « {} »", group_label(&run.group)),
            if cancelled {
                "Synchronisation du groupe annulée.".to_owned()
            } else if problems == 0 {
                "Tous les profils sont synchronisés.".to_owned()
            } else {
                format!("{problems} profil(s) à examiner dans le bilan.")
            },
        );
        self.open_dialog(Dialog::GroupReport {
            group: run.group,
            results: run.results,
            cancelled,
        });
        Task::none()
    }

    pub(crate) fn group_run_dialog<'a>(
        &'a self,
        p: &Palette,
        group: &'a str,
        profiles: &'a [ProfileId],
        allow_deletions: bool,
    ) -> El<'a, Message> {
        let members: Vec<_> = profiles.iter().filter_map(|id| self.profile(id)).collect();
        let deletions = members.iter().any(|m| m.allow_delete);
        let list = members.iter().fold(column![].spacing(6), |col, m| {
            col.push(
                row![
                    icons::icon(Glyph::Layers, 14.0, p.muted),
                    ui::colored(p.text, model::ellipsize(&m.name, 50), 13.0),
                    ui::fill(),
                    ui::caption(p, model::mode_label(m.mode)),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
        });
        let mut col = column![
            dialog_head(p, &format!("Synchroniser le groupe « {} » ?", group_label(group))),
            ui::muted(p, format!(
                "Les {} profils sont simulés puis exécutés l’un après l’autre, sans autre confirmation. Chaque exécution applique exactement le plan de sa simulation ; les garde-fous restent actifs.",
                members.len()
            )),
            container(scrollable(list).style(theme::scroll(*p)).height(Length::Shrink))
                .padding(12)
                .max_height(220.0)
                .width(Length::Fill)
                .style(theme::outline(*p, p.border, 0.0)),
            ui::caption(p, "Un profil dont la corbeille ne peut pas garder les fichiers remplacés ou supprimés est laissé de côté, sans aucune écriture. Annuler arrête le profil en cours et les suivants."),
        ]
        .spacing(14);
        if deletions {
            col = col.push(
                container(
                    column![
                        checkbox(allow_deletions)
                            .label("J’autorise les suppressions prévues par les simulations")
                            .on_toggle(Message::GroupRunDeletions)
                            .text_size(13)
                            .style(theme::check(*p)),
                        ui::caption(p, "Seulement pour les profils qui les permettent, dans les limites des garde-fous. Sans cette case, un profil dont la simulation prévoit des suppressions est laissé de côté."),
                    ]
                    .spacing(6),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::outline(*p, p.danger, if allow_deletions { 1.0 } else { 0.4 })),
            );
        }
        col.push(
            row![
                ui::fill(),
                ui::action(p, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                ui::action(
                    p,
                    if allow_deletions {
                        Tone::Danger
                    } else {
                        Tone::Success
                    },
                    Some(Glyph::Bolt),
                    format!("Synchroniser {} profil(s)", members.len()),
                    (!self.busy() && !members.is_empty()).then_some(Message::StartGroupRun),
                ),
            ]
            .spacing(10),
        )
        .into()
    }

    pub(crate) fn group_report_dialog<'a>(
        &'a self,
        p: &Palette,
        group: &'a str,
        results: &'a [GroupResult],
        cancelled: bool,
    ) -> El<'a, Message> {
        let list = results.iter().fold(column![].spacing(10), |col, r| {
            let (glyph, color, label) = match &r.step {
                GroupStep::Executed(Outcome::Success, label) => {
                    (Glyph::Check, p.success, label.clone())
                }
                GroupStep::Executed(Outcome::Warning, label) => {
                    (Glyph::Alert, p.warning, label.clone())
                }
                GroupStep::Executed(_, label) => (Glyph::Alert, p.danger, label.clone()),
                GroupStep::UpToDate => (
                    Glyph::Check,
                    p.success,
                    "Déjà à jour, rien à copier".to_owned(),
                ),
                GroupStep::Skipped(reason) => (
                    Glyph::Shield,
                    p.warning,
                    format!("Laissé de côté : {reason}"),
                ),
                GroupStep::Failed(message) => {
                    (Glyph::Alert, p.danger, format!("Échec : {message}"))
                }
                GroupStep::NotStarted => (Glyph::Stop, p.faint, "Non lancé".to_owned()),
            };
            col.push(
                row![
                    icons::icon(glyph, 15.0, color),
                    column![
                        ui::strong(p.text, model::ellipsize(&r.name, 60), 13.0),
                        ui::colored(p.muted, label, 12.0),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                ]
                .spacing(10)
                .align_y(Alignment::Start),
            )
        });
        column![
            dialog_head(p, &format!("Bilan du groupe « {} »", group_label(group))),
            if cancelled {
                ui::callout(
                    p,
                    p.warning,
                    Glyph::Stop,
                    "Annulé",
                    "L’état partiel du profil interrompu est conservé et journalisé.",
                )
            } else {
                El::from(Space::new())
            },
            container(
                scrollable(list)
                    .style(theme::scroll(*p))
                    .height(Length::Shrink)
            )
            .max_height(360.0)
            .width(Length::Fill),
            ui::caption(
                p,
                "Le détail de chaque exécution est dans l’historique de son profil."
            ),
            row![
                ui::fill(),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Check),
                    "Fermer",
                    Some(Message::CloseDialog)
                ),
            ],
        ]
        .spacing(14)
        .into()
    }
}

fn group_label(group: &str) -> &str {
    if group.is_empty() {
        "Sans groupe"
    } else {
        group
    }
}

fn dialog_head<'a>(p: &Palette, title: &str) -> El<'a, Message> {
    row![
        ui::glyph_box(Glyph::Bolt, p.success, 40.0),
        ui::title(p, title.to_owned(), 19.0),
        ui::fill(),
        iced::widget::button(icons::icon(Glyph::Close, 14.0, p.faint))
            .padding(6)
            .style(theme::btn(*p, Tone::Ghost))
            .on_press(Message::CloseDialog),
    ]
    .spacing(14)
    .align_y(Alignment::Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Summary, TrashSummary};

    fn view(summary: Summary) -> PreviewView {
        PreviewView {
            plan_id: "plan".into(),
            profile: "p".into(),
            source: String::new(),
            destination: String::new(),
            source_complete: true,
            destination_complete: true,
            summary,
            first: Vec::new(),
        }
    }

    #[test]
    fn group_consents_match_the_single_profile_dialog() {
        // Given nothing to change, Then nothing is executed.
        assert_eq!(decide(&view(Summary::default()), true), Next::UpToDate);
        // Given copies only, Then the plan runs without deletion consent.
        let copies = Summary {
            copy: 3,
            ..Summary::default()
        };
        assert_eq!(
            decide(&view(copies), false),
            Next::Execute {
                confirm_delete: false
            }
        );
        // Given deletions, Then they need the group consent.
        let mirror = Summary {
            copy: 1,
            delete: 2,
            ..Summary::default()
        };
        assert!(matches!(decide(&view(mirror), false), Next::Skip(_)));
        assert_eq!(
            decide(&view(mirror), true),
            Next::Execute {
                confirm_delete: true
            }
        );
        // Given a trash too small, Then the permanent loss is never implied.
        let overflow = Summary {
            replace: 1,
            trash: TrashSummary {
                enabled: true,
                limit: 1,
                needed: 2,
                overflow: true,
            },
            ..Summary::default()
        };
        assert!(matches!(decide(&view(overflow), true), Next::Skip(_)));
    }
}
