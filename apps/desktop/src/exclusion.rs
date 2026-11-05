//! « Exclure du profil » : from a row of the history (a warning on a file that
//! has vanished since, say), adds a filter to the profile so that the problem
//! does not come back: this exact file, every file of that name, or every
//! file of that extension. Presentation only: the filter syntax is the one of
//! `sync-filter` (globs on logical paths, a pattern without `/` matches at any
//! depth), and the profile is saved through the facade like any other edit.

use crate::app::{App, Dialog, Message, ToastKind};
use crate::backend::{self, Failure, Saved};
use crate::icons::{self, Glyph};
use crate::theme::{self, Palette, Tone};
use crate::ui::{self, El};
use iced::widget::{button, column, container, row};
use iced::{Alignment, Length, Padding, Task};
use sync_core::ProfileId;

/// How far the exclusion reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// This path only.
    Path,
    /// Every entry of that name, at any depth.
    Name,
    /// Every file with that extension, at any depth.
    Extension,
}

/// State of the exclusion dialog.
#[derive(Debug, Clone)]
pub(crate) struct ExclusionDialog {
    pub(crate) profile: ProfileId,
    /// Relative logical path of the entry, `/`-separated.
    pub(crate) path: String,
    pub(crate) directory: bool,
    pub(crate) scope: Scope,
    pub(crate) saving: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum ExclusionMsg {
    Ask { path: String, directory: bool },
    Scope(Scope),
    Confirm,
    Saved(Result<Saved, Failure>),
}

/// Glob metacharacters: a name holding one would exclude more than itself,
/// and escaping is not guessed (same rule as « Exclure » on a folder).
const GLOB_CHARS: [char; 7] = ['*', '?', '[', ']', '{', '}', '\\'];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Extension of a file name without its dot (`log.db-shm` → `db-shm`); none
/// for a name that only starts with a dot (`.gitignore`) or ends with one.
fn extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    (!stem.is_empty() && !ext.is_empty()).then_some(ext)
}

/// Filter pattern for `path` at `scope`; `None` when it does not apply or
/// cannot be written safely.
pub(crate) fn pattern(path: &str, directory: bool, scope: Scope) -> Option<String> {
    let name = file_name(path);
    let plain = |text: &str| !text.is_empty() && !text.contains(GLOB_CHARS);
    match scope {
        Scope::Path => plain(path).then(|| path.to_owned()),
        Scope::Name => plain(name).then(|| name.to_owned()),
        Scope::Extension if directory => None,
        // Only the extension is checked: the wildcard is ours.
        Scope::Extension => extension(name)
            .filter(|ext| plain(ext))
            .map(|ext| format!("*.{ext}")),
    }
}

/// Scopes offered for `path`, narrowest first, without duplicates: a file at
/// the root has no separate « every file of that name ».
pub(crate) fn scopes(path: &str, directory: bool) -> Vec<Scope> {
    let mut list = Vec::new();
    for scope in [Scope::Path, Scope::Name, Scope::Extension] {
        if scope == Scope::Name && !path.contains('/') {
            continue;
        }
        if pattern(path, directory, scope).is_some() {
            list.push(scope);
        }
    }
    list
}

impl App {
    pub(crate) fn exclusion(&mut self, msg: ExclusionMsg) -> Task<Message> {
        match msg {
            ExclusionMsg::Ask { path, directory } => {
                let Some(id) = self
                    .selected
                    .clone()
                    .filter(|id| self.profile(id).is_some())
                else {
                    return Task::none();
                };
                let Some(scope) = scopes(&path, directory).first().copied() else {
                    self.toast(
                        ToastKind::Warning,
                        "Exclusion impossible ici",
                        "Ce nom contient un caractère de motif (* ? [ ] { } \\) : ajoutez le filtre à la main dans la modification du profil.",
                    );
                    return Task::none();
                };
                self.open_dialog(Dialog::Exclusion(Box::new(ExclusionDialog {
                    profile: id,
                    path,
                    directory,
                    scope,
                    saving: false,
                })));
                Task::none()
            }
            ExclusionMsg::Scope(scope) => {
                if let Some(Dialog::Exclusion(d)) = &mut self.dialog
                    && scopes(&d.path, d.directory).contains(&scope)
                {
                    d.scope = scope;
                }
                Task::none()
            }
            ExclusionMsg::Confirm => self.confirm_exclusion(),
            ExclusionMsg::Saved(result) => {
                self.close_dialog();
                match result {
                    Ok(_) => {
                        self.toast(
                            ToastKind::Success,
                            "Exclusion ajoutée au profil",
                            "Les prochaines simulations et exécutions ignorent ce motif.",
                        );
                        self.refresh()
                    }
                    Err(e) => {
                        self.fail(&e);
                        Task::none()
                    }
                }
            }
        }
    }

    fn confirm_exclusion(&mut self) -> Task<Message> {
        let Some(app) = self.backend.clone() else {
            return Task::none();
        };
        let Some(Dialog::Exclusion(d)) = &self.dialog else {
            return Task::none();
        };
        if d.saving {
            return Task::none();
        }
        if self.busy() || self.running_on(&d.profile) {
            self.toast(
                ToastKind::Info,
                "Patientez",
                "Le profil ne peut pas être modifié pendant une tâche.",
            );
            return Task::none();
        }
        let (Some(profile), Some(pattern)) = (
            self.profile(&d.profile),
            pattern(&d.path, d.directory, d.scope),
        ) else {
            return Task::none();
        };
        let mut profile = profile.clone();
        let present = if d.directory {
            profile.filter.exclude_directories().contains(&pattern)
        } else {
            profile.filter.exclude_files().contains(&pattern)
        };
        if !present {
            profile.filter = if d.directory {
                profile.filter.clone().exclude_directory(pattern)
            } else {
                profile.filter.clone().exclude_file(pattern)
            };
        }
        if let Some(Dialog::Exclusion(d)) = &mut self.dialog {
            d.saving = true;
        }
        Task::perform(
            backend::save(&app, profile, backend::ScheduleChange::Keep),
            |r| Message::Exclusion(ExclusionMsg::Saved(r)),
        )
    }

    pub(crate) fn exclusion_dialog<'a>(
        &'a self,
        p: &Palette,
        d: &'a ExclusionDialog,
    ) -> El<'a, Message> {
        let name = self
            .profile(&d.profile)
            .map_or(String::new(), |pr| pr.name.clone());
        let option = |scope: Scope, title: String, hint: String| -> El<'a, Message> {
            let chosen = d.scope == scope;
            button(
                row![
                    icons::icon(
                        if chosen { Glyph::Check } else { Glyph::Plus },
                        14.0,
                        if chosen { p.accent } else { p.faint }
                    ),
                    column![
                        ui::strong(p.text, title, 13.0),
                        ui::mono(p.muted, hint, 12.0),
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding(Padding::from([10, 12]))
            .width(Length::Fill)
            .style(theme::row_btn(*p, chosen))
            .on_press(Message::Exclusion(ExclusionMsg::Scope(scope)))
            .into()
        };
        let noun = if d.directory { "dossier" } else { "fichier" };
        let mut options = column![].spacing(6);
        for scope in scopes(&d.path, d.directory) {
            let hint = pattern(&d.path, d.directory, scope).unwrap_or_default();
            let title = match scope {
                Scope::Path => format!("Ce {noun} seulement"),
                Scope::Name if d.directory => "Tous les dossiers portant ce nom".to_owned(),
                Scope::Name => "Tous les fichiers portant ce nom".to_owned(),
                Scope::Extension => format!(
                    "Tous les fichiers « .{} »",
                    extension(file_name(&d.path)).unwrap_or_default()
                ),
            };
            options = options.push(option(scope, title, hint));
        }
        let scheduled = self.schedule_of(&d.profile).is_some();
        let mut col = column![
            row![
                ui::glyph_box(Glyph::Shield, p.warning, 40.0),
                ui::title(p, format!("Exclure de « {name} »"), 19.0),
                ui::fill(),
                button(icons::icon(Glyph::Close, 14.0, p.faint))
                    .padding(6)
                    .style(theme::btn(*p, Tone::Ghost))
                    .on_press(Message::CloseDialog),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
            ui::muted(
                p,
                "Le profil ignorera ce qui correspond : ces éléments ne seront plus copiés, comparés ni supprimés, et ne produiront plus d’avertissement. Un filtre ne supprime jamais rien de lui-même."
            ),
            container(ui::mono(p.text, crate::model::ellipsize(&d.path, 70), 12.0))
                .padding(10)
                .width(Length::Fill)
                .style(theme::outline(*p, p.border, 0.0)),
            options,
        ]
        .spacing(14);
        if scheduled {
            col = col.push(ui::callout(
                p,
                p.warning,
                Glyph::Clock,
                "Planification à renouveler",
                "Le profil est modifié : son autorisation de planification devra être renouvelée.",
            ));
        }
        col.push(
            row![
                ui::fill(),
                ui::action(p, Tone::Ghost, None, "Annuler", Some(Message::CloseDialog)),
                ui::action(
                    p,
                    Tone::Primary,
                    Some(Glyph::Check),
                    "Ajouter l’exclusion",
                    (!d.saving).then_some(Message::Exclusion(ExclusionMsg::Confirm)),
                ),
            ]
            .spacing(10),
        )
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_offers_its_path_its_name_and_its_extension() {
        // Given the SQLite side file of the report, Then three reaches exist.
        let path = "sonarr/config/logs.db-shm";
        assert_eq!(
            scopes(path, false),
            [Scope::Path, Scope::Name, Scope::Extension]
        );
        assert_eq!(pattern(path, false, Scope::Path).as_deref(), Some(path));
        assert_eq!(
            pattern(path, false, Scope::Name).as_deref(),
            Some("logs.db-shm")
        );
        assert_eq!(
            pattern(path, false, Scope::Extension).as_deref(),
            Some("*.db-shm")
        );
    }

    #[test]
    fn a_root_file_has_no_separate_name_scope_and_a_dot_file_no_extension() {
        assert_eq!(scopes("notes.txt", false), [Scope::Path, Scope::Extension]);
        assert_eq!(scopes(".gitignore", false), [Scope::Path]);
        assert_eq!(scopes("archive.", false), [Scope::Path]);
    }

    #[test]
    fn a_folder_has_no_extension_scope() {
        assert_eq!(scopes("a/b.d", true), [Scope::Path, Scope::Name]);
    }

    #[test]
    fn a_pattern_character_in_a_name_refuses_the_scopes_it_would_widen() {
        // Given a name holding a glob character, Then no pattern is guessed.
        assert_eq!(pattern("a/[x].log", false, Scope::Path), None);
        assert_eq!(pattern("a/f*.log", false, Scope::Name), None);
        // The extension alone is plain: it still applies.
        assert_eq!(
            pattern("a/[x].log", false, Scope::Extension).as_deref(),
            Some("*.log")
        );
        assert_eq!(scopes("a/[x].log", false), [Scope::Extension]);
        assert_eq!(pattern("a/b.l*g", false, Scope::Extension), None);
    }
}
