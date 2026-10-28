//! Folder views over long lists of logical paths (plan, history, trash):
//! one level at a time, so that 10 000 deletions read as a few folders with
//! their counts instead of a list to scroll. Presentation only; the lists
//! themselves (and the executable plan) are unchanged.
use crate::{AppError, error};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use sync_core::ResourcePath;

/// Entries of a level returned per request.
pub const TREE_PAGE: usize = 100;

/// A sub-folder of the requested level, with what lies below it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeFolder {
    /// Last segment.
    pub name: String,
    /// Logical path from the root (`a/b`).
    pub path: String,
    /// Matching entries below this folder (its own operation excluded).
    pub count: u64,
    /// Bytes of those entries, when the list knows them.
    pub bytes: u64,
    /// Count per action code (`copy`, `delete`…), for the summary.
    pub actions: BTreeMap<String, u64>,
    /// Action of the folder's own entry (its creation or deletion…).
    pub own: Option<String>,
}

/// One entry of a list, as seen by [`level`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Item<'a> {
    pub(crate) path: &'a str,
    pub(crate) action: &'a str,
    pub(crate) bytes: u64,
}

/// One level: sub-folders, then a page of the entries directly inside.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Level {
    pub(crate) folders: Vec<TreeFolder>,
    /// Indexes (in the given list) of the page of direct entries.
    pub(crate) entries: Vec<usize>,
    /// Direct entries in total (folders excluded).
    pub(crate) total: usize,
}

/// Checks a requested folder: empty for the root, else a logical path.
/// # Errors
/// Invalid path or offset.
pub(crate) fn check(folder: &str, offset: usize) -> Result<(), AppError> {
    if offset > u32::MAX as usize {
        return Err(error("invalid_input", "position hors limites"));
    }
    if !folder.is_empty() {
        ResourcePath::parse(folder).map_err(|e| error("invalid_input", e))?;
    }
    Ok(())
}

/// Groups `items` (index, entry) below `folder` by their next segment.
/// Entries keep their order; folders are sorted by name, ignoring case.
/// A folder whose own entry is in the list shows it as [`TreeFolder::own`]
/// instead of a separate row.
pub(crate) fn level<'a>(
    items: impl IntoIterator<Item = (usize, Item<'a>)>,
    folder: &str,
    offset: usize,
) -> Level {
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut folders: BTreeMap<&str, TreeFolder> = BTreeMap::new();
    let mut direct: Vec<(usize, &str, &str)> = Vec::new();
    for (index, item) in items {
        let Some(rest) = item.path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.split_once('/') {
            Some((name, _)) => {
                let node = folders.entry(name).or_insert_with(|| TreeFolder {
                    name: name.to_owned(),
                    path: format!("{prefix}{name}"),
                    ..TreeFolder::default()
                });
                node.count += 1;
                node.bytes = node.bytes.saturating_add(item.bytes);
                *node.actions.entry(item.action.to_owned()).or_default() += 1;
            }
            None => direct.push((index, rest, item.action)),
        }
    }
    direct.retain(|(_, name, action)| match folders.get_mut(name) {
        Some(node) => {
            node.own = Some((*action).to_owned());
            false
        }
        None => true,
    });
    let mut folders: Vec<_> = folders.into_values().collect();
    folders.sort_by_cached_key(|f| (f.name.to_lowercase(), f.name.clone()));
    Level {
        folders,
        total: direct.len(),
        entries: direct
            .into_iter()
            .skip(offset)
            .take(TREE_PAGE)
            .map(|(index, _, _)| index)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(list: &[(&'static str, &'static str, u64)]) -> Vec<(usize, Item<'static>)> {
        list.iter()
            .enumerate()
            .map(|(i, (path, action, bytes))| {
                (
                    i,
                    Item {
                        path,
                        action,
                        bytes: *bytes,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn folders_aggregate_their_descendants_and_entries_stay_in_order() {
        // Given deletions spread over two folders and a root file.
        let list = items(&[
            ("Photos", "delete", 0),
            ("Photos/2024/a.jpg", "delete", 10),
            ("Photos/2024/b.jpg", "delete", 20),
            ("Photos/c.jpg", "copy", 5),
            ("Archives/x.zip", "delete", 100),
            ("readme.txt", "replace", 1),
            ("Photo.txt", "copy", 2),
        ]);
        // When reading the root level.
        let root = level(list.clone(), "", 0);
        // Then the folders show counts per action, the folder's own deletion
        // is attached to it, and only files remain as entries.
        assert_eq!(
            root.folders
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["Archives", "Photos"]
        );
        let photos = &root.folders[1];
        assert_eq!(photos.path, "Photos");
        assert_eq!(photos.count, 3);
        assert_eq!(photos.bytes, 35);
        assert_eq!(photos.actions.get("delete"), Some(&2));
        assert_eq!(photos.actions.get("copy"), Some(&1));
        assert_eq!(photos.own.as_deref(), Some("delete"));
        assert_eq!(root.entries, vec![5, 6]);
        assert_eq!(root.total, 2);
        // A sub-level only sees its own descendants (not « Photo.txt »).
        let sub = level(list.clone(), "Photos", 0);
        assert_eq!(sub.folders.len(), 1);
        assert_eq!(sub.folders[0].path, "Photos/2024");
        assert_eq!(sub.entries, vec![3]);
        let leaf = level(list, "Photos/2024", 0);
        assert!(leaf.folders.is_empty());
        assert_eq!(leaf.entries, vec![1, 2]);
    }

    #[test]
    fn entries_are_paged_and_folders_are_sorted_ignoring_case() {
        let mut list = Vec::new();
        for n in 0..250 {
            list.push((format!("f{n:03}"), "copy"));
        }
        list.push(("b/x".to_owned(), "copy"));
        list.push(("A/y".to_owned(), "copy"));
        let items: Vec<_> = list
            .iter()
            .enumerate()
            .map(|(i, (path, action))| {
                (
                    i,
                    Item {
                        path,
                        action,
                        bytes: 0,
                    },
                )
            })
            .collect();
        let page = level(items.clone(), "", 200);
        assert_eq!(page.total, 250);
        assert_eq!(page.entries.len(), 50);
        assert_eq!(page.entries[0], 200);
        assert_eq!(
            page.folders
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "b"]
        );
        assert!(check("", 0).is_ok());
        assert!(check("a/b", 0).is_ok());
        assert!(check("../a", 0).is_err());
        assert!(check("/a", 0).is_err());
    }
}
