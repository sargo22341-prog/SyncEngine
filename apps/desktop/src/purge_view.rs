//! Réglages › « Supprimer toutes les données de SyncEngine » : exact list
//! with sizes, typed confirmation « SUPPRIMER », report, then exit. The
//! removal itself is in [`crate::purge`], on a worker thread.

use crate::app::{App, Dialog, Message, ToastKind};
use crate::backend::{self, Failure};
use crate::icons::{self, Glyph};
use crate::model;
use crate::purge::{self, Inventory, Places, Report, TraceKind};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, row, scrollable, text_input};
use iced::{Alignment, Length, Padding, Task};
use std::path::PathBuf;
use std::time::Duration;
use sync_profiles::{Profile, RemoteEndpoint};

/// Word to type to confirm.
pub(crate) const CONFIRMATION: &str = "SUPPRIMER";
/// Longest wait for the agents to stop cooperatively.
const AGENT_STOP_WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub(crate) struct PurgeDialog {
    /// `None` while the list is built.
    pub(crate) inventory: Option<Result<Inventory, Failure>>,
    pub(crate) typed: String,
    pub(crate) running: bool,
    /// Error of the last attempt (nothing was removed).
    pub(crate) error: Option<Failure>,
    /// Final report: the application must then close.
    pub(crate) report: Option<Report>,
}

#[derive(Debug, Clone)]
pub(crate) enum PurgeMsg {
    Ask,
    Listed(Box<Result<Inventory, Failure>>),
    Typed(String),
    Confirm,
    Done(Result<Report, Failure>),
}

fn wrap(msg: PurgeMsg) -> Message {
    Message::Purge(msg)
}

/// Local folders synchronized by the profiles (sources, destinations, SMB
/// roots): a folder containing one of them is never removed.
fn protected_roots(profiles: &[Profile]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for profile in profiles {
        for (path, remote) in [
            (&profile.source, &profile.remote_source),
            (&profile.destination, &profile.remote_destination),
        ] {
            match remote {
                None => roots.push(path.clone()),
                Some(RemoteEndpoint::Smb { root }) => roots.push(root.clone()),
                Some(_) => {}
            }
        }
    }
    roots
}

impl App {
    fn purge_state(&mut self) -> Option<&mut PurgeDialog> {
        match &mut self.dialog {
            Some(Dialog::Purge(d)) => Some(d),
            _ => None,
        }
    }

    fn running_anything(&self) -> bool {
        self.busy() || self.agent_job.is_some()
    }

    pub(crate) fn purge(&mut self, msg: PurgeMsg) -> Task<Message> {
        match msg {
            PurgeMsg::Ask => {
                if self.running_anything() {
                    self.toast(
                        ToastKind::Warning,
                        "Synchronisation en cours",
                        "Attendez sa fin (ou annulez-la) avant de supprimer les données.",
                    );
                    return Task::none();
                }
                let Some(app) = self.backend.clone() else {
                    return Task::none();
                };
                self.open_dialog(Dialog::Purge(Box::new(PurgeDialog {
                    inventory: None,
                    typed: String::new(),
                    running: false,
                    error: None,
                    report: None,
                })));
                Task::perform(
                    backend::blocking(move || {
                        let profiles = app.profiles_list().map_err(Failure::from)?;
                        purge::inventory(
                            app.state_directory(),
                            protected_roots(&profiles),
                            &Places::system(),
                        )
                    }),
                    |r| wrap(PurgeMsg::Listed(Box::new(r))),
                )
            }
            PurgeMsg::Listed(result) => {
                if let Some(d) = self.purge_state() {
                    d.inventory = Some(*result);
                }
                Task::none()
            }
            PurgeMsg::Typed(value) => {
                if let Some(d) = self.purge_state() {
                    d.typed = value.chars().take(32).collect();
                }
                Task::none()
            }
            PurgeMsg::Confirm => {
                let running = self.running_anything();
                let Some(d) = self.purge_state() else {
                    return Task::none();
                };
                let Some(Ok(inventory)) = d.inventory.clone() else {
                    return Task::none();
                };
                if d.typed.trim() != CONFIRMATION || d.running || d.report.is_some() || running {
                    return Task::none();
                }
                d.running = true;
                d.error = None;
                // The window's presence lock lives in the state directory.
                self.presence = None;
                Task::perform(
                    backend::blocking(move || purge::purge(&inventory, AGENT_STOP_WAIT)),
                    |r| wrap(PurgeMsg::Done(r)),
                )
            }
            PurgeMsg::Done(result) => {
                match result {
                    Ok(report) => {
                        // Nothing may write into the state any more.
                        self.purged = true;
                        if let Some(d) = self.purge_state() {
                            d.running = false;
                            d.report = Some(report);
                        }
                    }
                    Err(e) => {
                        if let Some(d) = self.purge_state() {
                            d.running = false;
                            d.error = Some(e);
                        }
                        if let Some(state) = self.state_dir.as_deref() {
                            self.presence = sync_orchestrator::hold_desktop_presence(state)
                                .ok()
                                .flatten();
                        }
                    }
                }
                Task::none()
            }
        }
    }

    pub(crate) fn purge_dialog<'a>(&'a self, p: &Palette, d: &'a PurgeDialog) -> El<'a, Message> {
        let head = row![
            ui::glyph_box(Glyph::Trash, p.danger, 40.0),
            ui::title(
                p,
                if d.report.is_some() {
                    "Données supprimées"
                } else {
                    "Supprimer toutes les données de SyncEngine"
                },
                19.0
            ),
            ui::fill(),
            button(icons::icon(Glyph::Close, 14.0, p.faint))
                .padding(6)
                .style(theme::btn(*p, Tone::Ghost))
                .on_press_maybe((!d.running).then_some(Message::CloseDialog)),
        ]
        .spacing(14)
        .align_y(Alignment::Center);
        let body: El<'a, Message> = match (&d.report, &d.inventory) {
            (Some(report), _) => report_view(p, report),
            (None, None) => ui::muted(
                p,
                "Recherche de tout ce que SyncEngine a laissé sur ce système…",
            ),
            (None, Some(Err(e))) => ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Liste impossible à établir : rien ne sera supprimé",
                format!("{} — {}", e.title(), e.message),
            ),
            (None, Some(Ok(inventory))) => self.confirm_view(p, d, inventory),
        };
        column![head, body].spacing(16).into()
    }

    fn confirm_view<'a>(
        &'a self,
        p: &Palette,
        d: &'a PurgeDialog,
        inventory: &'a Inventory,
    ) -> El<'a, Message> {
        let mut list = column![].spacing(6);
        for trace in &inventory.traces {
            let glyph = match trace.kind {
                TraceKind::Folder => Glyph::Folder,
                TraceKind::File => Glyph::Copy,
                TraceKind::Task | TraceKind::Unit => Glyph::Clock,
                TraceKind::Registry => Glyph::Power,
            };
            list = list.push(
                row![
                    icons::icon(glyph, 15.0, p.muted),
                    column![
                        ui::strong(p.text, trace.label.clone(), 13.0),
                        ui::mono(p.muted, model::ellipsize(&trace.location, 90), 12.0),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    ui::caption(p, trace.bytes.map(model::bytes).unwrap_or_default()),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
        }
        if inventory.traces.is_empty() {
            list = list.push(ui::muted(p, "Aucune trace trouvée."));
        }
        let ready = d.typed.trim() == CONFIRMATION && !d.running && !self.running_anything();
        let mut col = column![
            ui::muted(
                p,
                format!(
                    "SyncEngine est portable : après cette suppression, il ne reste qu’à supprimer le dossier du programme. Voici exactement ce qui sera supprimé ({} au total) :",
                    model::bytes(inventory.bytes())
                )
            ),
            container(scrollable(list).style(theme::scroll(*p)))
                .padding(Padding::from([10, 12]))
                .max_height((self.window.height - 460.0).clamp(120.0, 360.0))
                .width(Length::Fill)
                .style(theme::outline(*p, p.border, 0.0)),
            ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                &format!(
                    "La corbeille de tous les profils sera perdue ({})",
                    model::bytes(inventory.trash_bytes)
                ),
                "Profils, planifications, historique et anciennes versions gardées disparaissent définitivement. Vos dossiers source et destination ne sont jamais touchés.",
            ),
            column![
                ui::caption(p, format!("Pour confirmer, tapez {CONFIRMATION} :")),
                text_input(CONFIRMATION, &d.typed)
                    .on_input(|v| wrap(PurgeMsg::Typed(v)))
                    .on_submit(wrap(PurgeMsg::Confirm))
                    .padding(Padding::from([10, 12]))
                    .size(15)
                    .style(theme::input(*p, false)),
            ]
            .spacing(6),
        ]
        .spacing(14);
        if let Some(e) = &d.error {
            col = col.push(ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Rien n’a été supprimé",
                e.message.clone(),
            ));
        }
        col.push(
            row![
                ui::fill(),
                ui::action(
                    p,
                    Tone::Ghost,
                    None,
                    "Annuler",
                    (!d.running).then_some(Message::CloseDialog)
                ),
                ui::action(
                    p,
                    Tone::Danger,
                    Some(Glyph::Trash),
                    if d.running {
                        "Suppression…"
                    } else {
                        "Supprimer définitivement"
                    },
                    ready.then(|| wrap(PurgeMsg::Confirm))
                ),
            ]
            .spacing(10),
        )
        .into()
    }
}

fn report_view<'a>(p: &Palette, report: &'a Report) -> El<'a, Message> {
    let mut list = column![].spacing(6);
    for location in &report.removed {
        list = list.push(
            row![
                icons::icon(Glyph::Check, 14.0, p.success),
                ui::mono(p.text, model::ellipsize(location, 90), 12.0),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    for (location, cause) in &report.failed {
        list = list.push(
            column![
                row![
                    icons::icon(Glyph::Alert, 14.0, p.danger),
                    ui::mono(p.text, model::ellipsize(location, 90), 12.0),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                ui::colored(p.danger, format!("Impossible : {cause}"), 12.0),
            ]
            .spacing(2),
        );
    }
    column![
        ui::muted(
            p,
            if report.failed.is_empty() {
                "Toutes les traces de SyncEngine ont été supprimées. Il ne reste qu’à supprimer le dossier du programme."
            } else {
                "Certaines traces n’ont pas pu être supprimées ; leur cause est indiquée. Vous pouvez les retirer à la main."
            }
        ),
        container(scrollable(list).style(theme::scroll(*p)))
            .padding(Padding::from([10, 12]))
            .max_height(360.0)
            .width(Length::Fill)
            .style(theme::outline(*p, p.border, 0.0)),
        row![
            ui::fill(),
            ui::action(p, Tone::Primary, Some(Glyph::Power), "Fermer SyncEngine", Some(Message::Quit)),
        ],
        Space::new(),
    ]
    .spacing(14)
    .into()
}
