//! « Voir la corbeille » : kept copies of one profile, per synchronization.
//! Presentation only: sizes, listings, copies and emptying are done by the
//! facade on worker threads. There is deliberately no « restore in place »:
//! a mirror would delete the restored file again at its next run.

use crate::app::{App, Dialog, Message, ToastKind};
use crate::backend::{self, Failure};
use crate::icons::{self, Glyph};
use crate::model;
use crate::native;
use crate::theme::{self, Palette, Tone};
use crate::tree::{Level, Request, Tree, TreeMsg};
use crate::ui::{self, El};
use iced::widget::{Space, button, column, container, row, scrollable};
use iced::{Alignment, Length, Padding, Task};
use std::path::PathBuf;
use sync_core::ProfileId;
use sync_orchestrator::{TrashFile, TrashOverview};
use sync_plan::Action;

/// State of the trash dialog.
#[derive(Debug, Clone)]
pub(crate) struct TrashDialog {
    pub(crate) profile: ProfileId,
    /// `None` while loading.
    pub(crate) overview: Option<Result<TrashOverview, Failure>>,
    /// Synchronization whose files are shown, by folder.
    pub(crate) open: Option<String>,
    pub(crate) tree: Tree<TrashFile>,
    /// Second step of « Vider la corbeille ».
    pub(crate) confirm_empty: bool,
    pub(crate) emptying: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum TrashMsg {
    Open(ProfileId),
    Loaded(ProfileId, Result<TrashOverview, Failure>),
    Expand(String),
    Tree(TreeMsg),
    Level(String, Request, Result<Level<TrashFile>, Failure>),
    OpenFolder(PathBuf),
    SaveAs { execution: String, location: String },
    Saved(Result<Option<PathBuf>, Failure>),
    AskEmpty(bool),
    Empty,
    Emptied(Result<(), Failure>),
}

fn wrap(msg: TrashMsg) -> Message {
    Message::Trash(msg)
}

impl App {
    fn trash_state(&mut self) -> Option<&mut TrashDialog> {
        match &mut self.dialog {
            Some(Dialog::Trash(t)) => Some(t),
            _ => None,
        }
    }

    fn load_trash(&self, id: ProfileId) -> Task<Message> {
        let Some(app) = &self.backend else {
            return Task::none();
        };
        let key = id.clone();
        Task::perform(
            backend::call(app, move |a| a.trash_overview(&id)),
            move |r| wrap(TrashMsg::Loaded(key.clone(), r)),
        )
    }

    fn load_trash_level(
        &self,
        id: ProfileId,
        execution: String,
        request: Request,
    ) -> Task<Message> {
        let Some(app) = &self.backend else {
            return Task::none();
        };
        let task = backend::trash_tree(app, id, execution.clone(), &request);
        Task::perform(task, move |r| {
            wrap(TrashMsg::Level(execution.clone(), request.clone(), r))
        })
    }

    pub(crate) fn trash(&mut self, msg: TrashMsg) -> Task<Message> {
        match msg {
            TrashMsg::Open(id) => {
                self.open_dialog(Dialog::Trash(Box::new(TrashDialog {
                    profile: id.clone(),
                    overview: None,
                    open: None,
                    tree: Tree::default(),
                    confirm_empty: false,
                    emptying: false,
                })));
                self.load_trash(id)
            }
            TrashMsg::Loaded(id, result) => {
                if let Some(t) = self.trash_state().filter(|t| t.profile == id) {
                    t.overview = Some(result);
                }
                Task::none()
            }
            TrashMsg::Expand(execution) => {
                let Some(t) = self.trash_state() else {
                    return Task::none();
                };
                if t.open.as_deref() == Some(execution.as_str()) {
                    t.open = None;
                    return Task::none();
                }
                t.open = Some(execution.clone());
                let request = t.tree.reset();
                let id = t.profile.clone();
                self.load_trash_level(id, execution, request)
            }
            TrashMsg::Tree(msg) => {
                let Some(t) = self.trash_state() else {
                    return Task::none();
                };
                let (Some(execution), Some(request)) = (t.open.clone(), t.tree.update(msg)) else {
                    return Task::none();
                };
                let id = t.profile.clone();
                self.load_trash_level(id, execution, request)
            }
            TrashMsg::Level(execution, request, result) => {
                if let Some(t) = self
                    .trash_state()
                    .filter(|t| t.open.as_deref() == Some(execution.as_str()))
                {
                    t.tree.loaded(&request, result.map_err(|e| e.message));
                }
                Task::none()
            }
            TrashMsg::OpenFolder(path) => {
                if let Err(e) = native::open_folder(&path) {
                    self.fail(&e);
                }
                Task::none()
            }
            TrashMsg::SaveAs {
                execution,
                location,
            } => {
                let (Some(app), Some(id)) = (
                    self.backend.clone(),
                    self.trash_state().map(|t| t.profile.clone()),
                ) else {
                    return Task::none();
                };
                let name = location.rsplit('/').next().unwrap_or("fichier").to_owned();
                Task::perform(
                    backend::blocking(move || {
                        let Some(target) =
                            native::pick_save_file("Enregistrer une copie de la corbeille", &name)
                        else {
                            return Ok(None);
                        };
                        app.trash_save_as(&id, &execution, &location, &target)
                            .map_err(Failure::from)?;
                        Ok(Some(target))
                    }),
                    |r| wrap(TrashMsg::Saved(r)),
                )
            }
            TrashMsg::Saved(result) => {
                match result {
                    Ok(Some(path)) => self.toast(
                        ToastKind::Success,
                        "Copie enregistrée",
                        path.display().to_string(),
                    ),
                    Ok(None) => (),
                    Err(e) => self.fail(&e),
                }
                Task::none()
            }
            TrashMsg::AskEmpty(v) => {
                if let Some(t) = self.trash_state() {
                    t.confirm_empty = v;
                }
                Task::none()
            }
            TrashMsg::Empty => {
                let busy = self.busy();
                let (Some(app), Some(t)) = (self.backend.clone(), self.trash_state()) else {
                    return Task::none();
                };
                if busy || !t.confirm_empty {
                    return Task::none();
                }
                t.emptying = true;
                let id = t.profile.clone();
                Task::perform(backend::call(&app, move |a| a.trash_empty(&id)), |r| {
                    wrap(TrashMsg::Emptied(r))
                })
            }
            TrashMsg::Emptied(result) => {
                let Some(t) = self.trash_state() else {
                    return Task::none();
                };
                t.emptying = false;
                t.confirm_empty = false;
                t.open = None;
                t.tree = Tree::default();
                let id = t.profile.clone();
                match &result {
                    Ok(()) => self.toast(
                        ToastKind::Info,
                        "Corbeille vidée",
                        "Les copies conservées ont été supprimées.",
                    ),
                    Err(e) => self.fail(e),
                }
                self.load_trash(id)
            }
        }
    }

    pub(crate) fn trash_dialog<'a>(&'a self, p: &Palette, t: &'a TrashDialog) -> El<'a, Message> {
        let name = self
            .profile(&t.profile)
            .map_or(String::new(), |pr| pr.name.clone());
        let head = row![
            ui::glyph_box(Glyph::Trash, p.accent, 40.0),
            column![
                ui::title(p, "Corbeille", 19.0),
                ui::caption(p, model::ellipsize(&name, 60)),
            ]
            .spacing(2),
            ui::fill(),
            button(icons::icon(Glyph::Close, 14.0, p.faint))
                .padding(6)
                .style(theme::btn(*p, Tone::Ghost))
                .on_press(Message::CloseDialog),
        ]
        .spacing(14)
        .align_y(Alignment::Center);
        let body: El<'a, Message> = match &t.overview {
            None => ui::muted(p, "Lecture de la corbeille…"),
            Some(Err(e)) => ui::callout(p, p.danger, Glyph::Alert, e.title(), e.message.clone()),
            Some(Ok(overview)) => self.trash_content(p, t, overview),
        };
        column![head, body].spacing(16).into()
    }

    fn trash_content<'a>(
        &'a self,
        p: &Palette,
        t: &'a TrashDialog,
        overview: &'a TrashOverview,
    ) -> El<'a, Message> {
        let state = if overview.enabled {
            "activée"
        } else {
            "désactivée : aucune nouvelle copie"
        };
        let usage = row![
            column![
                ui::strong(
                    p.text,
                    format!(
                        "{} / {}",
                        model::bytes(overview.used),
                        model::bytes(overview.limit)
                    ),
                    16.0
                ),
                ui::caption(p, format!("Corbeille {state}")),
            ]
            .spacing(2),
            ui::fill(),
            if t.confirm_empty {
                El::from(
                    row![
                        ui::action(
                            p,
                            Tone::Ghost,
                            None,
                            "Annuler",
                            Some(wrap(TrashMsg::AskEmpty(false)))
                        ),
                        ui::action(
                            p,
                            Tone::Danger,
                            Some(Glyph::Trash),
                            if t.emptying {
                                "Suppression…"
                            } else {
                                "Confirmer : tout supprimer"
                            },
                            (!t.emptying && !self.busy()).then(|| wrap(TrashMsg::Empty)),
                        ),
                    ]
                    .spacing(8),
                )
            } else {
                ui::action(
                    p,
                    Tone::Secondary,
                    Some(Glyph::Trash),
                    "Vider la corbeille",
                    (!overview.runs.is_empty() && !self.busy())
                        .then(|| wrap(TrashMsg::AskEmpty(true))),
                )
            },
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let mut col = column![usage].spacing(12);
        if t.confirm_empty {
            col = col.push(ui::callout(
                p,
                p.danger,
                Glyph::Alert,
                "Vider la corbeille ?",
                "Toutes les anciennes versions gardées pour ce profil seront supprimées définitivement. Les copies d’une synchronisation interrompue restent jusqu’à son diagnostic.",
            ));
        }
        if overview.runs.is_empty() {
            col = col.push(ui::muted(
                p,
                if overview.enabled {
                    "La corbeille est vide : aucune synchronisation n’a encore remplacé ou supprimé de fichier."
                } else {
                    "La corbeille est vide. Activez-la dans « Sécurité » pour garder les fichiers remplacés ou supprimés."
                },
            ));
            return col.into();
        }
        let mut list = column![].spacing(8);
        for run in &overview.runs {
            let open = t.open.as_deref() == Some(run.execution.as_str());
            let date = run
                .started
                .map_or_else(|| run.execution.clone(), model::utc_label);
            let header = row![
                ui::strong(p.text, date, 14.0),
                ui::icon_button(
                    p,
                    Glyph::Folder,
                    "Ouvrir le dossier",
                    Some(wrap(TrashMsg::OpenFolder(run.folder.clone())))
                ),
                if run.running {
                    ui::pill(p.warning, "interrompue")
                } else {
                    Space::new().into()
                },
                ui::fill(),
                ui::caption(
                    p,
                    format!(
                        "{} fichier(s) · {}",
                        model::count(run.files),
                        model::bytes(run.bytes)
                    )
                ),
                button(icons::icon(
                    if open {
                        Glyph::ChevronDown
                    } else {
                        Glyph::ChevronRight
                    },
                    14.0,
                    p.muted
                ))
                .padding(6)
                .style(theme::btn(*p, Tone::Ghost))
                .on_press(wrap(TrashMsg::Expand(run.execution.clone()))),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            let mut card = column![header].spacing(8);
            if open {
                card = card.push(self.trash_files(p, t, &run.execution));
            }
            list = list.push(
                container(card)
                    .padding(Padding::from([10, 14]))
                    .width(Length::Fill)
                    .style(theme::outline(*p, p.border, 0.0)),
            );
        }
        col.push(
            scrollable(list)
                .style(theme::scroll(*p))
                .height(Length::Fixed(
                    (self.window.height - 320.0).clamp(160.0, 520.0),
                )),
        )
        .into()
    }

    fn trash_files<'a>(
        &'a self,
        p: &Palette,
        t: &'a TrashDialog,
        execution: &'a str,
    ) -> El<'a, Message> {
        let pal = *p;
        crate::tree::view(
            p,
            &t.tree,
            |msg| wrap(TrashMsg::Tree(msg)),
            &|file, indent| trash_file_row(&pal, file, indent, execution),
            &|folder| crate::tree::action_summary(&pal, folder),
            "Aucun fichier dans cette synchronisation.",
        )
    }
}

/// One kept file, below its folder.
fn trash_file_row<'a>(
    p: &Palette,
    file: &'a TrashFile,
    indent: f32,
    execution: &'a str,
) -> El<'a, Message> {
    let kind = match file.action {
        Some(Action::Replace) => "remplacé",
        Some(Action::Delete) => "supprimé",
        _ => "conservé",
    };
    let name = file.path.rsplit('/').next().unwrap_or(&file.path);
    row![
        container(ui::mono(p.text, model::ellipsize(name, 64), 12.0)).width(Length::Fill),
        ui::caption(p, kind),
        ui::caption(
            p,
            file.bytes.map_or_else(|| "absent".to_owned(), model::bytes)
        ),
        ui::action(
            p,
            Tone::Ghost,
            None,
            "Enregistrer sous…",
            file.bytes.is_some().then(|| {
                wrap(TrashMsg::SaveAs {
                    execution: execution.to_owned(),
                    location: file.location.clone(),
                })
            })
        ),
    ]
    .spacing(10)
    .padding(Padding {
        left: indent,
        ..Padding::ZERO
    })
    .align_y(Alignment::Center)
    .into()
}
