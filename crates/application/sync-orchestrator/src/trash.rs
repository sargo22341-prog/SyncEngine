//! Per-profile recycle bin, inside the application state directory:
//! `Corbeille/<profile id>/<execution id>/<logical path>`.
//!
//! The executor fills one execution folder through a [`VersionStore`]
//! (copies verified before every replacement or deletion). This module only
//! sizes, lists and retires those folders. It never follows a link or a
//! junction, and only touches folders named by a valid profile and
//! execution identity under `Corbeille`.
//!
//! [`VersionStore`]: sync_versioning::VersionStore
use crate::{AppError, Application, error, io, runs};
use serde::Serialize;
use std::path::{Path, PathBuf};
use sync_core::{EntryKind, ExecutionId, ProfileId, ResourcePath};
use sync_plan::{Action, ExecutionPlan};
use sync_profiles::TrashPolicy;

/// Name of the trash folder in the state directory.
pub const TRASH_FOLDER: &str = "Corbeille";
/// Synchronizations kept per profile, like the history.
pub const KEPT_TRASH_RUNS: usize = 30;
/// Files returned by one [`Application::trash_page`].
const PAGE: usize = 100;

/// Trash decision for one plan, computed by the preview and again before
/// execution. All or nothing: an overflowing synchronization keeps nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TrashEstimate {
    /// The profile keeps copies.
    pub enabled: bool,
    /// Size limit of the profile trash.
    pub limit: u64,
    /// Known size of the destination files the plan replaces or deletes.
    pub needed: u64,
    /// Enabled, and `needed` exceeds `limit`: the files would be lost for
    /// good, so execution needs an explicit manual confirmation.
    pub overflow: bool,
}

impl TrashEstimate {
    pub(crate) fn of(policy: &TrashPolicy, plan: &ExecutionPlan) -> Self {
        let needed = needed(plan);
        Self {
            enabled: policy.enabled,
            limit: policy.max_bytes,
            needed,
            overflow: policy.enabled && needed > policy.max_bytes,
        }
    }
    /// Copies are made during this execution.
    pub(crate) fn keeps_copies(&self) -> bool {
        self.enabled && !self.overflow
    }
}

/// Size of the files that `plan` replaces or deletes, as observed on their
/// receiving side. Directories are empty when deleted; unknown sizes count 0.
#[must_use]
pub(crate) fn needed(plan: &ExecutionPlan) -> u64 {
    plan.operations()
        .iter()
        .filter(|op| {
            matches!(op.action(), Action::Replace | Action::Delete) && op.kind() == EntryKind::File
        })
        .filter_map(|op| op.observation(op.target()))
        .filter_map(|entry| entry.metadata().size())
        .fold(0, u64::saturating_add)
}

/// Folder of the version store of one execution, relative to the state.
pub(crate) fn store_folder(
    profile: &ProfileId,
    execution: &ExecutionId,
) -> Result<ResourcePath, AppError> {
    ResourcePath::parse(&format!("{TRASH_FOLDER}/{profile}/{execution}")).map_err(io)
}

/// `Corbeille/<profile>`; identities are `[A-Za-z0-9._-]`, dots-only ones
/// (which would name `.` or `..`) are refused.
fn profile_dir(root: &Path, profile: &ProfileId) -> Result<PathBuf, AppError> {
    if profile.as_str().chars().all(|c| c == '.') {
        return Err(error("invalid_input", "identifiant de profil réservé"));
    }
    Ok(root.join(TRASH_FOLDER).join(profile.as_str()))
}

fn run_dir(root: &Path, profile: &ProfileId, execution: &str) -> Result<PathBuf, AppError> {
    let execution = ExecutionId::new(execution).map_err(|e| error("invalid_input", e))?;
    if execution.as_str().chars().all(|c| c == '.') {
        return Err(error("invalid_input", "identifiant d’exécution réservé"));
    }
    Ok(profile_dir(root, profile)?.join(execution.as_str()))
}

/// Regular files and bytes under `dir`, without following links.
fn measure(dir: &Path) -> std::io::Result<(u64, u64)> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    walk(dir, "", &mut |_, size| {
        files += 1;
        bytes = bytes.saturating_add(size);
    })?;
    Ok((files, bytes))
}

/// Visits the regular files under `dir` with their `/`-separated relative
/// path, in name order. Links, junctions and special files are skipped.
fn walk(dir: &Path, prefix: &str, visit: &mut dyn FnMut(String, u64)) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        // `symlink_metadata` reports links and junctions as links.
        let meta = std::fs::symlink_metadata(entry.path())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(&entry.path(), &path, visit)?;
        } else if meta.is_file() {
            visit(path, meta.len());
        }
    }
    Ok(())
}

/// Removes one trash folder. A link or junction in its place is refused,
/// never followed; `remove_dir_all` does not follow nested ones either.
fn remove_tree(path: &Path) -> Result<(), AppError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error("trash_retention", e)),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => Err(error(
            "trash_retention",
            format!("{} n’est pas un dossier de corbeille", path.display()),
        )),
        Ok(_) => std::fs::remove_dir_all(path)
            .map_err(|e| error("trash_retention", format!("{} : {e}", path.display()))),
    }
}

/// Execution folders of a profile trash with their history rank (newest
/// first) and state; foreign names are ignored.
struct Folder {
    execution: String,
    path: PathBuf,
    rank: Option<usize>,
    running: bool,
    started: Option<i64>,
}

fn folders(root: &Path, profile: &ProfileId) -> Result<Vec<Folder>, AppError> {
    let dir = profile_dir(root, profile)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(e)),
    };
    let runs = runs::profile_runs(root, profile)?;
    let mut list = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let meta = std::fs::symlink_metadata(entry.path()).map_err(io)?;
        if ExecutionId::new(name.as_str()).is_err()
            || meta.file_type().is_symlink()
            || !meta.is_dir()
        {
            continue;
        }
        let known = runs.iter().position(|r| r.execution == name);
        let started = known.and_then(|i| runs[i].started).or_else(|| {
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_secs()).ok())
        });
        list.push(Folder {
            path: entry.path(),
            running: known.is_some_and(|i| runs[i].state == "running"),
            rank: known,
            started,
            execution: name,
        });
    }
    // Known folders newest first, then folders without history by date.
    list.sort_by(|a, b| match (a.rank, b.rank) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.started.cmp(&a.started),
    });
    Ok(list)
}

/// Applies the retention of a profile trash: folders of synchronizations
/// gone from the history are removed, then the oldest ones until at most
/// [`KEPT_TRASH_RUNS`] remain within `limit`. `current` (the run just
/// recorded) and interrupted runs are always kept. Every removal is tried;
/// the last failure is returned (retried at the next synchronization).
pub(crate) fn enforce(
    root: &Path,
    profile: &ProfileId,
    limit: u64,
    current: Option<&ExecutionId>,
) -> Result<(), AppError> {
    let mut failure = None;
    let mut kept = Vec::new();
    for folder in folders(root, profile)? {
        let protected = folder.running || current.is_some_and(|c| c.as_str() == folder.execution);
        if folder.rank.is_none() && !protected {
            if let Err(e) = remove_tree(&folder.path) {
                failure = Some(e);
            }
            continue;
        }
        let (_, bytes) = measure(&folder.path).map_err(io)?;
        kept.push((folder, bytes, protected));
    }
    let mut total = kept.iter().map(|(_, b, _)| *b).fold(0, u64::saturating_add);
    let mut count = kept.len();
    // Oldest first, skipping the protected ones.
    for (folder, bytes, protected) in kept.iter().rev() {
        if total <= limit && count <= KEPT_TRASH_RUNS {
            break;
        }
        if *protected {
            continue;
        }
        match remove_tree(&folder.path) {
            Ok(()) => {
                total = total.saturating_sub(*bytes);
                count -= 1;
            }
            Err(e) => failure = Some(e),
        }
    }
    failure.map_or(Ok(()), Err)
}

/// One synchronization in a profile trash.
#[derive(Debug, Clone, Serialize)]
pub struct TrashRun {
    /// Execution identity, also the folder name.
    pub execution: String,
    /// UTC start (seconds), from the history or the folder date.
    pub started: Option<i64>,
    /// Files kept.
    pub files: u64,
    /// Bytes kept.
    pub bytes: u64,
    /// Absolute folder, for « Ouvrir le dossier ».
    pub folder: PathBuf,
    /// The synchronization is still marked running (interrupted).
    pub running: bool,
}

/// Content of a profile trash.
#[derive(Debug, Clone, Serialize)]
pub struct TrashOverview {
    /// Current policy of the profile.
    pub enabled: bool,
    /// Size limit.
    pub limit: u64,
    /// Bytes currently kept.
    pub used: u64,
    /// Synchronizations, newest first.
    pub runs: Vec<TrashRun>,
}

/// One kept file.
#[derive(Debug, Clone, Serialize)]
pub struct TrashFile {
    /// Logical path in the synchronized tree.
    pub path: String,
    /// `replace` or `delete` when the history knows it.
    pub action: Option<Action>,
    /// Current size of the copy, absent when it is gone.
    pub bytes: Option<u64>,
    /// Path of the copy inside the synchronization folder.
    pub location: String,
}

/// One page of the files of a synchronization.
#[derive(Debug, Clone, Serialize)]
pub struct TrashPage {
    /// Files of the synchronization.
    pub total: usize,
    /// At most 100 files from the requested offset.
    pub files: Vec<TrashFile>,
}

/// One folder level of a synchronization in the trash (see [`crate::TreeFolder`]).
#[derive(Debug, Clone, Serialize)]
pub struct TrashTree {
    /// Sub-folders with their number of kept files.
    pub folders: Vec<crate::TreeFolder>,
    /// Files directly inside the folder in total.
    pub total: usize,
    /// At most 100 of them from the requested offset.
    pub files: Vec<TrashFile>,
}

/// Kept file: logical path, action when known, location in its folder.
type Kept = (String, Option<Action>, String);

/// Files recorded by the history (`status.version` of each outcome) for
/// one execution, or `None` without a final report.
fn recorded_files(
    root: &Path,
    profile: &ProfileId,
    execution: &str,
) -> Result<Option<Vec<Kept>>, AppError> {
    let Some(report) = runs::report_of(root, profile, execution)? else {
        return Ok(None);
    };
    let prefix = format!("{TRASH_FOLDER}/{profile}/{execution}/");
    let outcomes = report
        .get("outcomes")
        .or_else(|| report.get("execution").and_then(|e| e.get("outcomes")))
        .and_then(serde_json::Value::as_array);
    let Some(outcomes) = outcomes else {
        return Ok(None);
    };
    Ok(Some(
        outcomes
            .iter()
            .filter_map(|outcome| {
                let location = outcome["status"]["version"]
                    .as_str()?
                    .strip_prefix(&prefix)?;
                let path = outcome["path"].as_str()?.to_owned();
                let action = serde_json::from_value(outcome["action"].clone()).ok();
                Some((path, action, location.to_owned()))
            })
            .collect(),
    ))
}

/// Kept files of a synchronization: recorded by its history, else found in
/// its folder (interrupted run).
fn kept_files(
    root: &Path,
    profile: &ProfileId,
    execution: &str,
    dir: &Path,
) -> Result<Vec<Kept>, AppError> {
    match recorded_files(root, profile, execution)? {
        Some(list) if !list.is_empty() => Ok(list),
        _ => {
            let mut list = Vec::new();
            match walk(dir, "", &mut |path, _| {
                list.push((path.clone(), None, path))
            }) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(io(e)),
            }
            Ok(list)
        }
    }
}

/// Presentation of one kept file, with the current size of its copy.
fn trash_file(dir: &Path, (path, action, location): &Kept) -> TrashFile {
    let bytes = copy_path(dir, location)
        .ok()
        .and_then(|file| std::fs::symlink_metadata(file).ok())
        .filter(std::fs::Metadata::is_file)
        .map(|meta| meta.len());
    TrashFile {
        path: path.clone(),
        action: *action,
        bytes,
        location: location.clone(),
    }
}

/// File of the copy `location` (a logical path) inside `dir`.
fn copy_path(dir: &Path, location: &str) -> Result<PathBuf, AppError> {
    let logical = ResourcePath::parse(location).map_err(|e| error("invalid_input", e))?;
    if logical.is_root() {
        return Err(error("invalid_input", "fichier de corbeille requis"));
    }
    Ok(logical.segments().fold(dir.to_path_buf(), |p, s| p.join(s)))
}

impl Application {
    /// Content of a profile trash, newest synchronization first. Read-only.
    /// # Errors
    /// Unknown profile, unreadable folder or history.
    pub fn trash_overview(&self, id: &ProfileId) -> Result<TrashOverview, AppError> {
        let policy = self.profile(id)?.trash;
        let mut runs = Vec::new();
        let mut used = 0u64;
        for folder in folders(&self.state, id)? {
            let (files, bytes) = measure(&folder.path).map_err(io)?;
            used = used.saturating_add(bytes);
            runs.push(TrashRun {
                execution: folder.execution,
                started: folder.started,
                files,
                bytes,
                folder: folder.path,
                running: folder.running,
            });
        }
        Ok(TrashOverview {
            enabled: policy.enabled,
            limit: policy.max_bytes,
            used,
            runs,
        })
    }

    /// At most 100 files of one synchronization, from the history, or from
    /// the folder content when the history has no final report for it.
    /// # Errors
    /// Invalid identity, unreadable folder or history.
    pub fn trash_page(
        &self,
        id: &ProfileId,
        execution: &str,
        offset: usize,
    ) -> Result<TrashPage, AppError> {
        let dir = run_dir(&self.state, id, execution)?;
        let entries = kept_files(&self.state, id, execution, &dir)?;
        let files = entries
            .iter()
            .skip(offset)
            .take(PAGE)
            .map(|kept| trash_file(&dir, kept))
            .collect();
        Ok(TrashPage {
            total: entries.len(),
            files,
        })
    }

    /// One folder level (`""` for the root) of the files kept by a
    /// synchronization, by their path in the synchronized tree. Read-only.
    /// # Errors
    /// Unknown profile or synchronization, invalid folder, unreadable files.
    pub fn trash_tree(
        &self,
        id: &ProfileId,
        execution: &str,
        folder: &str,
        offset: usize,
    ) -> Result<TrashTree, AppError> {
        crate::tree::check(folder, offset)?;
        let dir = run_dir(&self.state, id, execution)?;
        let entries = kept_files(&self.state, id, execution, &dir)?;
        let codes: Vec<String> = entries
            .iter()
            .map(|(_, action, _)| {
                action
                    .map(|a| serde_json::to_value(a).map_err(io))
                    .transpose()
                    .map(|v| {
                        v.and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    })
            })
            .collect::<Result<_, _>>()?;
        let level = crate::tree::level(
            entries
                .iter()
                .zip(&codes)
                .enumerate()
                .map(|(index, ((path, _, _), code))| {
                    (
                        index,
                        crate::tree::Item {
                            path,
                            action: code,
                            bytes: 0,
                        },
                    )
                }),
            folder,
            offset,
        );
        Ok(TrashTree {
            folders: level.folders,
            total: level.total,
            files: level
                .entries
                .iter()
                .filter_map(|index| entries.get(*index))
                .map(|kept| trash_file(&dir, kept))
                .collect(),
        })
    }

    /// Copies one kept file to `target` (chosen by the user) through a
    /// temporary file next to it, renamed once complete. The trash is unchanged.
    /// # Errors
    /// Busy, invalid location, missing copy, or write failure.
    pub fn trash_save_as(
        &self,
        id: &ProfileId,
        execution: &str,
        location: &str,
        target: &Path,
    ) -> Result<(), AppError> {
        let _gate = self.gate()?;
        let source = copy_path(&run_dir(&self.state, id, execution)?, location)?;
        let meta = std::fs::symlink_metadata(&source).map_err(io)?;
        if !meta.is_file() {
            return Err(error("not_found", "copie absente de la corbeille"));
        }
        let name = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_input", "nom de fichier requis"))?;
        let temporary = target.with_file_name(format!(".{name}.syncengine-tmp"));
        let copied =
            std::fs::copy(&source, &temporary).and_then(|_| std::fs::rename(&temporary, target));
        if let Err(e) = copied {
            // The partial temporary is ours: remove it, keep the copy error.
            match std::fs::remove_file(&temporary) {
                Ok(()) => (),
                Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => (),
                Err(cleanup) => {
                    return Err(io(format!(
                        "{e} ; temporaire {} : {cleanup}",
                        temporary.display()
                    )));
                }
            }
            return Err(io(e));
        }
        Ok(())
    }

    /// Empties a profile trash. Folders of interrupted synchronizations stay
    /// until their diagnosis.
    /// # Errors
    /// Busy, or a folder that cannot be removed (the others are).
    pub fn trash_empty(&self, id: &ProfileId) -> Result<(), AppError> {
        let _gate = self.gate()?;
        let mut failure = None;
        for folder in folders(&self.state, id)? {
            if !folder.running
                && let Err(e) = remove_tree(&folder.path)
            {
                failure = Some(e);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

/// Removes the whole trash of a removed profile.
pub(crate) fn remove_profile(root: &Path, profile: &ProfileId) -> Result<(), AppError> {
    remove_tree(&profile_dir(root, profile)?)
}

/// « 1,0 Go » : binary prefixes and French decimal comma, like the desktop.
pub(crate) fn human(value: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} o")
    } else {
        format!("{size:.2} {}", UNITS[unit]).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    type Failure = Box<dyn std::error::Error>;

    fn record(root: &Path, execution: &str, state: &str) -> Result<(), Failure> {
        runs::open(root)?.execute(
            "INSERT INTO runs VALUES(?1,'p','{\"secs\":1}',?2,'{}',NULL)",
            [execution, state],
        )?;
        Ok(())
    }
    fn fill(root: &Path, execution: &str, bytes: usize) -> Result<PathBuf, Failure> {
        let dir = root.join(TRASH_FOLDER).join("p").join(execution);
        std::fs::create_dir_all(dir.join("sub"))?;
        std::fs::write(dir.join("sub/file"), vec![0u8; bytes])?;
        Ok(dir)
    }

    #[test]
    fn retention_keeps_30_runs_then_drops_the_oldest_beyond_the_size() -> Result<(), Failure> {
        let root = tempfile::tempdir()?;
        let p = ProfileId::new("p")?;
        // Given 33 synchronizations with a trash folder, one interrupted,
        // and a folder whose synchronization left the history,
        for n in 0..33 {
            record(
                root.path(),
                &format!("run-{n:02}"),
                if n == 1 { "running" } else { "finished" },
            )?;
            fill(root.path(), &format!("run-{n:02}"), 10)?;
        }
        fill(root.path(), "run-gone", 10)?;
        std::fs::write(
            root.path().join(TRASH_FOLDER).join("p").join("notes.txt"),
            b"keep",
        )?;
        // When retention runs with a large limit,
        enforce(
            root.path(),
            &p,
            1_000_000,
            Some(&ExecutionId::new("run-32")?),
        )?;
        let names = |root: &Path| -> Result<Vec<String>, Failure> {
            let mut list: Vec<String> = std::fs::read_dir(root.join(TRASH_FOLDER).join("p"))?
                .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect::<Result<_, _>>()?;
            list.sort();
            Ok(list)
        };
        // Then 30 folders remain: the newest, plus the interrupted one;
        // the folder without history is gone and foreign files stay.
        let kept = names(root.path())?;
        assert_eq!(kept.len(), 31, "{kept:?}");
        assert!(kept.contains(&"run-01".to_owned()));
        assert!(!kept.contains(&"run-00".to_owned()) && !kept.contains(&"run-02".to_owned()));
        assert!(!kept.contains(&"run-gone".to_owned()));
        assert!(kept.contains(&"notes.txt".to_owned()));
        // When the limit only fits two synchronizations,
        enforce(root.path(), &p, 25, Some(&ExecutionId::new("run-32")?))?;
        // Then the oldest go first; the current and interrupted ones stay.
        assert_eq!(names(root.path())?, ["notes.txt", "run-01", "run-32"]);
        // Even a limit below the current synchronization keeps it.
        enforce(root.path(), &p, 1, Some(&ExecutionId::new("run-32")?))?;
        assert_eq!(names(root.path())?, ["notes.txt", "run-01", "run-32"]);
        Ok(())
    }

    #[test]
    fn listing_falls_back_to_the_folder_and_never_leaves_it() -> Result<(), Failure> {
        let root = tempfile::tempdir()?;
        let p = ProfileId::new("p")?;
        record(root.path(), "run-a", "running")?;
        let dir = fill(root.path(), "run-a", 7)?;
        assert_eq!(measure(&dir)?, (1, 7));
        assert!(run_dir(root.path(), &p, "..").is_err());
        assert!(copy_path(&dir, "../run-b/x").is_err());
        assert!(profile_dir(root.path(), &ProfileId::new("..")?).is_err());
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5,00 Go");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn links_are_neither_measured_nor_followed() -> Result<(), Failure> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::write(outside.path().join("precious"), b"never touched")?;
        let dir = fill(root.path(), "run-a", 3)?;
        std::os::unix::fs::symlink(outside.path(), dir.join("link"))?;
        assert_eq!(measure(&dir)?, (1, 3));
        remove_tree(&dir)?;
        assert!(outside.path().join("precious").exists());
        Ok(())
    }
}
