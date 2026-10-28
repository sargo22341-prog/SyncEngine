//! Folder tree of long file lists (simulation, history, trash). Each level is
//! loaded from the facade when its folder is opened: sub-folders with what
//! lies below them, then the entries directly inside, 100 at a time.
//! Presentation only: the lists and their filters are the facade's.

use crate::icons::{self, Glyph};
use crate::model;
use crate::theme::{self, Palette};
use crate::ui::{self, El};
use iced::widget::{button, column, container, row};
use iced::{Alignment, Length, Padding};
use std::collections::{HashMap, HashSet};
use sync_orchestrator::TreeFolder;

/// Indentation of one depth level, in logical pixels.
pub(crate) const INDENT: f32 = 18.0;
/// Sub-folders drawn per level; the search narrows larger levels.
const SHOWN_FOLDERS: usize = 500;

/// How a long list is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ListMode {
    /// By folder, collapsed by default.
    #[default]
    Tree,
    /// One flat list, in plan order.
    Flat,
}

/// One loaded level.
#[derive(Debug, Clone)]
pub(crate) struct Level<T> {
    pub(crate) folders: Vec<TreeFolder>,
    pub(crate) entries: Vec<T>,
    /// Entries directly inside the folder, in total.
    pub(crate) total: usize,
}

/// A level page to load: `folder` (`""` for the root) from `offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) generation: u64,
    pub(crate) folder: String,
    pub(crate) offset: usize,
}

#[derive(Debug, Clone)]
pub(crate) enum TreeMsg {
    Toggle(String),
    More(String),
}

/// Loaded levels and open folders. A new [`Tree::reset`] (new filter,
/// search or list) ignores the answers of the previous generation.
#[derive(Debug, Clone)]
pub(crate) struct Tree<T> {
    levels: HashMap<String, Level<T>>,
    open: HashSet<String>,
    loading: HashSet<String>,
    errors: HashMap<String, String>,
    generation: u64,
}

impl<T> Default for Tree<T> {
    fn default() -> Self {
        Self {
            levels: HashMap::new(),
            open: HashSet::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
            generation: 0,
        }
    }
}

impl<T> Tree<T> {
    /// Forgets everything and asks for the root level.
    pub(crate) fn reset(&mut self) -> Request {
        let generation = self.generation.wrapping_add(1);
        *self = Self {
            generation,
            ..Self::default()
        };
        self.request(String::new(), 0)
    }

    fn request(&mut self, folder: String, offset: usize) -> Request {
        self.loading.insert(folder.clone());
        self.errors.remove(&folder);
        Request {
            generation: self.generation,
            folder,
            offset,
        }
    }

    /// Opens or closes a folder; a level never loaded is then requested.
    pub(crate) fn update(&mut self, msg: TreeMsg) -> Option<Request> {
        match msg {
            TreeMsg::Toggle(folder) => {
                if self.open.remove(&folder) {
                    return None;
                }
                self.open.insert(folder.clone());
                (!self.levels.contains_key(&folder) && !self.loading.contains(&folder))
                    .then(|| self.request(folder, 0))
            }
            TreeMsg::More(folder) => {
                let offset = self.levels.get(&folder)?.entries.len();
                (!self.loading.contains(&folder)).then(|| self.request(folder, offset))
            }
        }
    }

    /// Stores a loaded page; `false` if it belongs to a previous generation.
    pub(crate) fn loaded(&mut self, request: &Request, result: Result<Level<T>, String>) -> bool {
        if request.generation != self.generation {
            return false;
        }
        self.loading.remove(&request.folder);
        match result {
            Ok(level) => match self.levels.get_mut(&request.folder) {
                Some(current) if request.offset > 0 => {
                    current.entries.extend(level.entries);
                    current.folders = level.folders;
                    current.total = level.total;
                }
                _ => {
                    self.levels.insert(request.folder.clone(), level);
                }
            },
            Err(message) => {
                self.errors.insert(request.folder.clone(), message);
            }
        }
        true
    }

    pub(crate) fn root(&self) -> Option<&Level<T>> {
        self.levels.get("")
    }
}

/// Default folder summary: entries below, by action, and their size.
pub(crate) fn action_summary<'a, M: 'a>(p: &Palette, folder: &TreeFolder) -> El<'a, M> {
    let mut summary = row![].spacing(6).align_y(Alignment::Center);
    for (code, count) in &folder.actions {
        let action = model::action_from_key(code);
        let color = action.map_or(p.muted, |a| crate::profile::action_color(p, a));
        let label = match action {
            Some(a) => model::action_noun(a).to_owned(),
            None if code.is_empty() => "fichiers".to_owned(),
            None => code.clone(),
        };
        summary = summary.push(ui::badge(
            theme::alpha(color, 0.14),
            color,
            format!("{} {label}", model::count(*count)),
        ));
    }
    if folder.bytes > 0 {
        summary = summary.push(ui::caption(p, model::bytes(folder.bytes)));
    }
    summary.into()
}

/// The tree, starting at the root level, open folders drawn below their row.
/// `entry` draws one entry at an indentation; `summary` describes a folder.
pub(crate) fn view<'a, T, M: Clone + 'a>(
    p: &Palette,
    tree: &'a Tree<T>,
    on: fn(TreeMsg) -> M,
    entry: &dyn Fn(&'a T, f32) -> El<'a, M>,
    summary: &dyn Fn(&TreeFolder) -> El<'a, M>,
    empty: &'a str,
) -> El<'a, M> {
    let mut col = column![].spacing(2);
    match tree.root() {
        None if tree.errors.contains_key("") => {
            col = col.push(ui::colored(p.danger, tree.errors[""].clone(), 12.0));
        }
        None => col = col.push(ui::caption(p, "Chargement…")),
        Some(level) if level.folders.is_empty() && level.total == 0 => {
            col = col.push(
                container(ui::muted(p, empty))
                    .padding(18)
                    .center_x(Length::Fill),
            );
        }
        Some(_) => col = level_rows(p, tree, "", 0, on, entry, summary, col),
    }
    col.into()
}

#[allow(clippy::too_many_arguments)] // Recursion carries the whole drawing context.
fn level_rows<'a, T, M: Clone + 'a>(
    p: &Palette,
    tree: &'a Tree<T>,
    folder: &str,
    depth: usize,
    on: fn(TreeMsg) -> M,
    entry: &dyn Fn(&'a T, f32) -> El<'a, M>,
    summary: &dyn Fn(&TreeFolder) -> El<'a, M>,
    mut col: iced::widget::Column<'a, M>,
) -> iced::widget::Column<'a, M> {
    let indent = depth as f32 * INDENT;
    let Some(level) = tree.levels.get(folder) else {
        let note = match tree.errors.get(folder) {
            Some(message) => ui::colored(p.danger, message.clone(), 12.0),
            None => ui::caption(p, "Chargement…"),
        };
        return col.push(container(note).padding(Padding {
            left: indent + INDENT + 12.0,
            top: 4.0,
            bottom: 4.0,
            right: 0.0,
        }));
    };
    for node in level.folders.iter().take(SHOWN_FOLDERS) {
        let open = tree.open.contains(&node.path);
        let mut label = row![
            ui::hspace(indent),
            icons::icon(
                if open {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronRight
                },
                13.0,
                p.muted
            ),
            icons::icon(Glyph::Folder, 15.0, p.accent),
            ui::strong(p.text, model::ellipsize(&node.name, 60), 13.0),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if let Some(own) = node.own.as_deref().and_then(model::action_from_key) {
            let color = crate::profile::action_color(p, own);
            label = label.push(ui::badge(
                theme::alpha(color, 0.14),
                color,
                format!("dossier : {}", model::action_label(own).to_lowercase()),
            ));
        }
        let row_el = button(
            row![
                label,
                ui::fill(),
                ui::caption(p, format!("{} élément(s)", model::count(node.count))),
                summary(node),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([5, 10]))
        .width(Length::Fill)
        .style(theme::row_btn(*p, open))
        .on_press(on(TreeMsg::Toggle(node.path.clone())));
        col = col.push(row_el);
        if open {
            col = level_rows(p, tree, &node.path, depth + 1, on, entry, summary, col);
        }
    }
    if level.folders.len() > SHOWN_FOLDERS {
        col = col.push(
            container(ui::caption(
                p,
                format!(
                    "… et {} autres dossiers : précisez la recherche pour les voir.",
                    model::count((level.folders.len() - SHOWN_FOLDERS) as u64)
                ),
            ))
            .padding(Padding {
                left: indent + 12.0,
                top: 4.0,
                bottom: 4.0,
                right: 0.0,
            }),
        );
    }
    for item in &level.entries {
        col = col.push(entry(item, indent + INDENT));
    }
    if level.entries.len() < level.total {
        let loading = tree.loading.contains(folder);
        col = col.push(
            container(
                row![
                    ui::caption(
                        p,
                        format!(
                            "{} sur {} fichiers de ce dossier",
                            model::count(level.entries.len() as u64),
                            model::count(level.total as u64)
                        )
                    ),
                    ui::action(
                        p,
                        theme::Tone::Ghost,
                        None,
                        if loading {
                            "Chargement…"
                        } else {
                            "Afficher 100 de plus"
                        },
                        (!loading).then(|| on(TreeMsg::More(folder.to_owned()))),
                    ),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding(Padding {
                left: indent + INDENT + 12.0,
                top: 2.0,
                bottom: 2.0,
                right: 0.0,
            }),
        );
    }
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(folders: &[&str], entries: usize, total: usize) -> Level<usize> {
        Level {
            folders: folders
                .iter()
                .map(|f| TreeFolder {
                    name: (*f).to_owned(),
                    path: (*f).to_owned(),
                    ..TreeFolder::default()
                })
                .collect(),
            entries: (0..entries).collect(),
            total,
        }
    }

    #[test]
    fn folders_load_once_pages_append_and_old_generations_are_ignored() -> Result<(), &'static str>
    {
        let mut tree = Tree::<usize>::default();
        let root = tree.reset();
        assert_eq!(root.folder, "");
        assert!(tree.loading.contains(""));
        assert!(tree.loaded(&root, Ok(level(&["a"], 100, 150))));
        // Opening a folder loads it once; closing keeps it loaded.
        let a = tree.update(TreeMsg::Toggle("a".into())).ok_or("load")?;
        assert_eq!((a.folder.as_str(), a.offset), ("a", 0));
        assert!(tree.update(TreeMsg::Toggle("a".into())).is_none());
        assert!(tree.loaded(&a, Ok(level(&[], 2, 2))));
        assert!(tree.update(TreeMsg::Toggle("a".into())).is_none());
        // « Afficher 100 de plus » continues after the loaded entries.
        let more = tree.update(TreeMsg::More(String::new())).ok_or("more")?;
        assert_eq!(more.offset, 100);
        assert!(tree.loaded(&more, Ok(level(&["a"], 50, 150))));
        assert_eq!(tree.root().map(|l| l.entries.len()), Some(150));
        // A reset (new search) drops the answers of the previous one.
        let stale = tree.update(TreeMsg::Toggle("b".into())).ok_or("b")?;
        let fresh = tree.reset();
        assert!(!tree.loaded(&stale, Ok(level(&[], 1, 1))));
        assert!(tree.loaded(&fresh, Err("échec".into())));
        assert!(tree.root().is_none());
        assert!(tree.errors.contains_key(""));
        Ok(())
    }
}
