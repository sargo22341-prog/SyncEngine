//! « Supprimer toutes les données de SyncEngine » (Réglages). The program is
//! portable: this removes every trace it leaves on the system, listed in
//! `docs/TRACES_SYSTEME.md` (any new trace must be added there and here).
//!
//! Only our exact names are touched: the folders `org.syncengine.desktop`
//! and `SyncEngine` under the user data directories, known files inside a
//! custom state directory referenced by a task, our scheduled tasks and
//! systemd units, the `SyncEngine` « Run » value. Removal never follows a
//! symbolic link or a junction (the link itself is removed), and a folder
//! holding a synchronized root of a profile is refused. Source and
//! destination folders are never touched.

use crate::backend::Failure;
use std::path::{Path, PathBuf};

/// Name of the default state directory under the user data directory.
const STATE_NAME: &str = "org.syncengine.desktop";
/// Name of the per-user folder holding the execution locks.
const LOCKS_NAME: &str = "SyncEngine";
/// Files SyncEngine writes directly in a state directory (SQLite side files
/// included). A custom state directory loses only these, `run-*.jsonl`,
/// `native-tasks-*` and the folders of [`STATE_FOLDERS`].
const STATE_FILES: [&str; 15] = [
    "profiles.sqlite",
    "state.sqlite",
    "scheduler.sqlite",
    "desktop-runs.sqlite",
    "desktop.lock",
    "window.lock",
    "agent.lock",
    "agent-live.json",
    "agent-queue.json",
    "agent-status.json",
    "agent-pause.request",
    "agent-stop.request",
    "agent-retire.request",
    "cancel.request",
    "desktop-ui.json",
];
/// Side files: SQLite journals and temporary replacements of the files above.
const STATE_SUFFIXES: [&str; 4] = ["-wal", "-shm", "-journal", ".tmp"];
/// Folders of a state directory: the trash, and the copies of older versions.
const STATE_FOLDERS: [&str; 2] = [sync_orchestrator::TRASH_FOLDER, "versions"];

/// Nature of a trace, for presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceKind {
    Folder,
    File,
    #[cfg_attr(
        not(windows),
        expect(
            dead_code,
            reason = "scheduled tasks and the Run value exist only under Windows"
        )
    )]
    Task,
    #[cfg_attr(
        not(windows),
        expect(
            dead_code,
            reason = "scheduled tasks and the Run value exist only under Windows"
        )
    )]
    Registry,
    #[cfg_attr(
        windows,
        expect(
            dead_code,
            reason = "systemd units and autostart files exist only under Linux"
        )
    )]
    Unit,
}

/// How a trace is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Removal {
    /// Folder bearing our exact name: removed whole, links not followed.
    Tree(PathBuf),
    /// Custom state directory: our known entries, then the folder if empty.
    StateEntries(PathBuf),
    /// Single file (spool, unit, link, autostart entry).
    File(PathBuf),
    /// Windows scheduled task of this account.
    #[cfg_attr(
        not(windows),
        expect(
            dead_code,
            reason = "scheduled tasks and the Run value exist only under Windows"
        )
    )]
    Task(String),
    /// `HKCU\…\Run` value `SyncEngine`.
    #[cfg_attr(
        not(windows),
        expect(
            dead_code,
            reason = "scheduled tasks and the Run value exist only under Windows"
        )
    )]
    RunValue,
}

/// One trace left on the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Trace {
    pub(crate) kind: TraceKind,
    /// What it is, in plain words.
    pub(crate) label: String,
    /// Where it is (path, task or value name).
    pub(crate) location: String,
    /// Size on disk, for folders and files.
    pub(crate) bytes: Option<u64>,
    pub(crate) removal: Removal,
}

/// Exact list shown before deletion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub(crate) traces: Vec<Trace>,
    /// State directories whose agent is asked to stop first.
    pub(crate) states: Vec<PathBuf>,
    /// Bytes of profile trashes (`Corbeille`) that will be lost.
    pub(crate) trash_bytes: u64,
    /// Local synchronized roots of the profiles: never removed.
    pub(crate) protected: Vec<PathBuf>,
    /// Per-user execution locks, checked free before deleting.
    pub(crate) lock_root: Option<PathBuf>,
}

/// Where SyncEngine leaves traces on this system.
#[derive(Debug, Clone)]
pub(crate) struct Places {
    /// User data directory holding `org.syncengine.desktop`.
    pub(crate) data_home: Option<PathBuf>,
    /// Execution locks directory (`…/SyncEngine/job-locks`).
    pub(crate) lock_root: Option<PathBuf>,
    /// System temporary directory, where a remote upload spools its file.
    pub(crate) temp_dir: Option<PathBuf>,
    /// Whether native integrations (tasks, registry, units) are listed.
    pub(crate) native: bool,
}

impl Places {
    /// The real locations of this account.
    pub(crate) fn system() -> Self {
        Self {
            data_home: data_home(),
            lock_root: sync_orchestrator::user_lock_root().ok(),
            temp_dir: Some(std::env::temp_dir()).filter(|p| p.is_absolute()),
            native: true,
        }
    }
}

impl Inventory {
    pub(crate) fn bytes(&self) -> u64 {
        self.traces.iter().filter_map(|t| t.bytes).sum()
    }
}

/// Outcome of the deletion, trace by trace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) removed: Vec<String>,
    /// Trace and cause.
    pub(crate) failed: Vec<(String, String)>,
}

fn io(e: impl std::fmt::Display) -> Failure {
    Failure::new("unavailable", e.to_string())
}

/// Size of a tree, without following links or junctions.
fn tree_bytes(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else {
        return 0;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| tree_bytes(&e.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// Removes a link or junction itself, never its target.
fn remove_link(path: &Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_DIRECTORY: a directory link or junction.
        if meta.file_attributes() & 0x10 != 0 {
            return std::fs::remove_dir(path);
        }
    }
    #[cfg(not(windows))]
    let _ = meta;
    std::fs::remove_file(path)
}

/// Removes a tree without following links or junctions. Every entry is
/// tried; the first failure is returned.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.file_type().is_symlink() {
        return remove_link(path, &meta);
    }
    if !meta.is_dir() {
        return std::fs::remove_file(path);
    }
    let mut first = None;
    for entry in std::fs::read_dir(path)? {
        if let Err(e) = entry.and_then(|e| remove_tree(&e.path())) {
            first.get_or_insert(e);
        }
    }
    match first {
        Some(e) => Err(e),
        None => std::fs::remove_dir(path),
    }
}

/// Whether `name` is one of our entries in a state directory.
fn state_entry(name: &str) -> bool {
    let base = STATE_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name);
    STATE_FILES.contains(&name)
        || STATE_FILES.contains(&base)
        || STATE_FOLDERS.contains(&name)
        || (name.starts_with("run-")
            && name.ends_with(".jsonl")
            && sync_core::ExecutionId::new(&name[..name.len() - 6]).is_ok())
        || name.strip_prefix("native-tasks-").is_some_and(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        })
}

/// Our entries of a custom state directory.
fn state_entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_str().is_some_and(state_entry))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Removes our entries, then the folder only if nothing else remains.
fn remove_state_entries(dir: &Path) -> Result<(), String> {
    let mut failures = Vec::new();
    for entry in state_entries(dir) {
        if let Err(e) = remove_tree(&entry) {
            failures.push(format!("{} : {e}", entry.display()));
        }
    }
    if !failures.is_empty() {
        return Err(failures.join(" ; "));
    }
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(_) if dir.read_dir().is_ok_and(|mut d| d.next().is_some()) => Err(
            "dossier conservé : il contient des fichiers qui ne sont pas à SyncEngine".to_owned(),
        ),
        Err(e) => Err(e.to_string()),
    }
}

/// User data directory holding the default state directory.
fn data_home() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
    .filter(|p| p.is_absolute())
}

/// User configuration directory (systemd units, autostart), Linux.
#[cfg(not(windows))]
fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .filter(|p| p.is_absolute())
}

fn same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// What a folder trace weighs and how much of it is trash.
fn folder(label: &str, path: &Path, removal: Removal, inventory: &mut Inventory) {
    let bytes = match &removal {
        Removal::StateEntries(dir) => state_entries(dir).iter().map(|e| tree_bytes(e)).sum(),
        _ => tree_bytes(path),
    };
    inventory.trash_bytes += STATE_FOLDERS
        .iter()
        .map(|name| tree_bytes(&path.join(name)))
        .sum::<u64>();
    inventory.traces.push(Trace {
        kind: TraceKind::Folder,
        label: label.to_owned(),
        location: path.display().to_string(),
        bytes: Some(bytes),
        removal,
    });
}

/// Builds the exact list of traces. `state` is the directory in use,
/// `protected` the local roots of its profiles.
/// # Errors
/// Native tasks or units cannot be listed: the list would not be exact.
pub(crate) fn inventory(
    state: &Path,
    protected: Vec<PathBuf>,
    places: &Places,
) -> Result<Inventory, Failure> {
    let mut inventory = Inventory {
        protected,
        lock_root: places.lock_root.clone(),
        ..Inventory::default()
    };
    let default = places.data_home.as_ref().map(|home| home.join(STATE_NAME));
    let mut states: Vec<PathBuf> = default.iter().filter(|d| exists(d)).cloned().collect();
    if !states.iter().any(|d| same(d, state)) && exists(state) {
        states.push(state.to_path_buf());
    }
    // Native integrations, which may name further state directories.
    let mut referenced = Vec::new();
    if places.native {
        native_traces(&mut inventory, &mut referenced)?;
    }
    for dir in referenced {
        if exists(&dir) && !states.iter().any(|d| same(d, &dir)) {
            states.push(dir);
        }
    }
    for dir in &states {
        let ours = dir.file_name().and_then(|n| n.to_str()) == Some(STATE_NAME);
        if ours {
            folder(
                "Dossier d’état : profils, historique, journaux et corbeille",
                dir,
                Removal::Tree(dir.clone()),
                &mut inventory,
            );
        } else {
            folder(
                "Dossier d’état personnalisé (fichiers de SyncEngine seulement)",
                dir,
                Removal::StateEntries(dir.clone()),
                &mut inventory,
            );
        }
    }
    if let Some(locks) = places
        .lock_root
        .as_ref()
        .and_then(|root| root.parent().map(Path::to_path_buf))
        .filter(|dir| dir.file_name().and_then(|n| n.to_str()) == Some(LOCKS_NAME))
        .filter(|dir| exists(dir))
    {
        folder(
            "Verrous d’exécution",
            &locks,
            Removal::Tree(locks.clone()),
            &mut inventory,
        );
    }
    if let Some(temp) = &places.temp_dir {
        spools(temp, &mut inventory);
    }
    inventory.states = states;
    Ok(inventory)
}

/// Upload spools left in the system temporary directory by an interrupted
/// copy to a remote endpoint (`.syncengine-….partial`, `connector-staging`).
/// Top level only; links are not files here.
fn spools(temp: &Path, inventory: &mut Inventory) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let ours = entry.file_name().to_str().is_some_and(|name| {
            name.len() > ".syncengine-.partial".len()
                && name.starts_with(".syncengine-")
                && name.ends_with(".partial")
        });
        if ours && entry.file_type().is_ok_and(|t| t.is_file()) {
            let path = entry.path();
            inventory.traces.push(Trace {
                kind: TraceKind::File,
                label: "Fichier d’envoi temporaire d’une copie interrompue".into(),
                location: path.display().to_string(),
                bytes: entry.metadata().ok().map(|m| m.len()),
                removal: Removal::File(path),
            });
        }
    }
}

#[cfg(windows)]
fn native_traces(inventory: &mut Inventory, referenced: &mut Vec<PathBuf>) -> Result<(), Failure> {
    for (name, arguments) in crate::windows_tasks::purgeable_tasks()? {
        if let Some(state) = crate::windows_tasks::task_state(&arguments) {
            referenced.push(state);
        }
        inventory.traces.push(Trace {
            kind: TraceKind::Task,
            label: "Tâche planifiée de l’agent".into(),
            location: name.clone(),
            bytes: None,
            removal: Removal::Task(name),
        });
    }
    if let Some(command) = crate::native::startup_command()?
        && command.to_lowercase().contains("syncengine-desktop")
    {
        inventory.traces.push(Trace {
            kind: TraceKind::Registry,
            label: "Démarrage automatique de l’interface".into(),
            location: r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\SyncEngine".into(),
            bytes: None,
            removal: Removal::RunValue,
        });
    }
    Ok(())
}

/// systemd user units written by `syncengine-agent export … linux`, with
/// the text that proves they are ours.
#[cfg(not(windows))]
const UNITS: [(&str, &str); 3] = [
    ("syncengine-clock.service", "syncengine-agent"),
    ("syncengine-session.service", "syncengine-agent"),
    (
        "syncengine-clock.timer",
        "Description=SyncEngine schedule dispatcher",
    ),
];
/// Links created by `systemctl --user enable` for those units.
#[cfg(not(windows))]
const UNIT_LINKS: [&str; 3] = [
    "timers.target.wants/syncengine-clock.timer",
    "default.target.wants/syncengine-clock.service",
    "default.target.wants/syncengine-session.service",
];
/// Desktop autostart entries that may launch the interface.
#[cfg(not(windows))]
const AUTOSTART: [&str; 3] = [
    "syncengine.desktop",
    "syncengine-desktop.desktop",
    "org.syncengine.desktop.desktop",
];

/// State directory of an `ExecStart="…" tick "STATE" …` line (systemd quoting).
#[cfg(any(not(windows), test))]
fn unit_state(unit: &str) -> Option<PathBuf> {
    let line = unit.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
    let mut quoted = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (c, current.as_mut()) {
            ('"', None) => current = Some(String::new()),
            ('"', Some(_)) => quoted.push(current.take()?),
            ('\\', Some(value)) => value.push(chars.next()?),
            ('%', Some(value)) => {
                if chars.next()? != '%' {
                    return None;
                }
                value.push('%');
            }
            ('$', Some(value)) => {
                if chars.next()? != '$' {
                    return None;
                }
                value.push('$');
            }
            (other, Some(value)) => value.push(other),
            (_, None) => {}
        }
    }
    quoted.get(1).map(PathBuf::from)
}

#[cfg(not(windows))]
fn native_traces(inventory: &mut Inventory, referenced: &mut Vec<PathBuf>) -> Result<(), Failure> {
    let Some(config) = config_home() else {
        return Ok(());
    };
    let units = config.join("systemd/user");
    for (name, marker) in UNITS {
        let path = units.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) if text.contains(marker) => {
                if let Some(state) = unit_state(&text) {
                    referenced.push(state);
                }
                inventory.traces.push(Trace {
                    kind: TraceKind::Unit,
                    label: "Unité systemd utilisateur de l’agent".into(),
                    location: path.display().to_string(),
                    bytes: Some(text.len() as u64),
                    removal: Removal::File(path),
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(format!("{} : {e}", path.display()))),
        }
    }
    for link in UNIT_LINKS {
        let path = units.join(link);
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            inventory.traces.push(Trace {
                kind: TraceKind::Unit,
                label: "Activation systemd (lien)".into(),
                location: path.display().to_string(),
                bytes: None,
                removal: Removal::File(path),
            });
        }
    }
    for name in AUTOSTART {
        let path = config.join("autostart").join(name);
        if std::fs::read_to_string(&path).is_ok_and(|text| text.contains("syncengine-desktop")) {
            inventory.traces.push(Trace {
                kind: TraceKind::File,
                label: "Démarrage automatique de l’interface".into(),
                location: path.display().to_string(),
                bytes: None,
                removal: Removal::File(path),
            });
        }
    }
    Ok(())
}

/// Stops the agents of every listed state directory (cooperative stop
/// request), waiting at most `wait` for their presence lock to be released.
fn stop_agents(states: &[PathBuf], wait: std::time::Duration) -> Result<(), Failure> {
    for state in states {
        if sync_orchestrator::resident_status(state)
            .map_err(Failure::from)?
            .available
        {
            sync_orchestrator::request_agent_stop(state).map_err(io)?;
        }
    }
    let deadline = std::time::Instant::now() + wait;
    loop {
        let mut running = Vec::new();
        for state in states {
            if sync_orchestrator::resident_status(state)
                .map_err(Failure::from)?
                .available
            {
                running.push(state.display().to_string());
            }
        }
        if running.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(Failure::new(
                "busy",
                format!(
                    "l’agent ne s’est pas arrêté ({}) : rien n’a été supprimé, réessayez",
                    running.join(", ")
                ),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Holds the command lock of a state directory: no command runs on it.
fn hold_state(state: &Path) -> Result<Option<std::fs::File>, Failure> {
    let path = state.join("desktop.lock");
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(Failure::new(
            "busy",
            "une commande SyncEngine est en cours : rien n’a été supprimé",
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(io(e)),
    }
}

/// Whether removing `path` would touch a synchronized root.
fn protected_by<'a>(path: &Path, protected: &'a [PathBuf]) -> Option<&'a PathBuf> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    protected.iter().find(|root| {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        root.starts_with(&path)
    })
}

/// Deletes every trace of `inventory`: stops the agents, removes tasks and
/// the « Run » value, checks and releases the locks, then removes folders.
/// The window must have released its presence lock beforehand.
/// # Errors
/// Nothing is removed when an agent does not stop or a command or an
/// execution holds its lock; later failures are reported trace by trace.
pub(crate) fn purge(inventory: &Inventory, wait: std::time::Duration) -> Result<Report, Failure> {
    stop_agents(&inventory.states, wait)?;
    // No command and no execution may run while traces disappear.
    let mut held = Vec::new();
    for state in &inventory.states {
        held.extend(hold_state(state)?);
    }
    let execution = inventory
        .lock_root
        .as_ref()
        .filter(|root| root.is_dir())
        .map(|root| sync_orchestrator::hold_execution_lock(root))
        .transpose()
        .map_err(Failure::from)?;
    let mut report = Report::default();
    let record = |report: &mut Report, trace: &Trace, result: Result<(), String>| match result {
        Ok(()) => report.removed.push(trace.location.clone()),
        Err(cause) => report.failed.push((trace.location.clone(), cause)),
    };
    // Native integrations first: nothing may start the agent again.
    let tasks: Vec<String> = inventory
        .traces
        .iter()
        .filter_map(|t| match &t.removal {
            Removal::Task(name) => Some(name.clone()),
            _ => None,
        })
        .collect();
    let removed_tasks = remove_tasks(&tasks);
    for trace in inventory
        .traces
        .iter()
        .filter(|t| t.kind == TraceKind::Task)
    {
        record(&mut report, trace, removed_tasks.clone());
    }
    for trace in inventory
        .traces
        .iter()
        .filter(|t| matches!(t.removal, Removal::RunValue | Removal::File(_)))
    {
        let result = match &trace.removal {
            Removal::RunValue => remove_run_value(),
            Removal::File(path) => std::fs::remove_file(path).map_err(|e| e.to_string()),
            _ => Ok(()),
        };
        record(&mut report, trace, result);
    }
    reload_units(inventory, &mut report);
    // Locks are released before their folders go.
    drop(held);
    drop(execution);
    for trace in &inventory.traces {
        let result = match &trace.removal {
            Removal::Tree(path) | Removal::StateEntries(path)
                if protected_by(path, &inventory.protected).is_some() =>
            {
                Err(format!(
                    "refusé : contient le dossier synchronisé {}",
                    protected_by(path, &inventory.protected)
                        .map_or_else(String::new, |p| p.display().to_string())
                ))
            }
            Removal::Tree(path) => match remove_tree(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other.map_err(|e| e.to_string()),
            },
            Removal::StateEntries(path) => remove_state_entries(path),
            _ => continue,
        };
        record(&mut report, trace, result);
    }
    Ok(report)
}

#[cfg(windows)]
fn remove_tasks(names: &[String]) -> Result<(), String> {
    crate::windows_tasks::remove_tasks(names).map_err(|e| e.message)
}

#[cfg(not(windows))]
fn remove_tasks(_names: &[String]) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn remove_run_value() -> Result<(), String> {
    crate::native::startup_save(false).map_err(|e| e.message)
}

#[cfg(not(windows))]
fn remove_run_value() -> Result<(), String> {
    Ok(())
}

/// Disables the removed units in the running user manager (best effort:
/// systemd may be absent; the files are gone either way).
#[cfg(not(windows))]
fn reload_units(inventory: &Inventory, report: &mut Report) {
    if !inventory.traces.iter().any(|t| t.kind == TraceKind::Unit) {
        return;
    }
    let units = UNITS.map(|(name, _)| name);
    for args in [
        &["--user", "disable", "--now"][..],
        &["--user", "daemon-reload"][..],
    ] {
        let mut command = std::process::Command::new("systemctl");
        command.args(args);
        if args.len() == 3 {
            command.args(units);
        }
        if let Err(e) = command.output().map_err(|e| e.to_string()).and_then(|out| {
            if out.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
            }
        }) {
            report
                .failed
                .push((format!("systemctl {}", args.join(" ")), e));
        }
    }
}

#[cfg(windows)]
fn reload_units(_inventory: &Inventory, _report: &mut Report) {}

#[cfg(test)]
mod tests {
    use super::*;
    type Failure = Box<dyn std::error::Error>;

    fn file(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, bytes)?;
        Ok(())
    }

    /// A junction (Windows) or symbolic link (Linux) at `link` to `target`.
    fn link_dir(target: &Path, link: &Path) -> Result<(), Failure> {
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()?
                .status;
            if !status.success() {
                return Err("mklink /J failed".into());
            }
        }
        #[cfg(not(windows))]
        std::os::unix::fs::symlink(target, link)?;
        Ok(())
    }

    #[test]
    fn our_tree_goes_whole_but_a_junction_inside_is_never_followed() -> Result<(), Failure> {
        // Given our state folder holding a link to an outside folder,
        let temp = tempfile::tempdir()?;
        let outside = temp.path().join("dossier-utilisateur");
        file(&outside.join("précieux.txt"), b"keep")?;
        let state = temp.path().join(STATE_NAME);
        file(&state.join("profiles.sqlite"), b"db")?;
        file(&state.join("Corbeille/p/run-1/a.txt"), b"old")?;
        link_dir(&outside, &state.join("Corbeille").join("p").join("lien"))?;
        let inventory = Inventory {
            traces: vec![Trace {
                kind: TraceKind::Folder,
                label: String::new(),
                location: state.display().to_string(),
                bytes: None,
                removal: Removal::Tree(state.clone()),
            }],
            ..Inventory::default()
        };
        // When everything is deleted,
        let report =
            purge(&inventory, std::time::Duration::from_secs(1)).map_err(|e| e.message.clone())?;
        // Then our folder is gone and the linked folder is intact.
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!state.exists());
        assert_eq!(std::fs::read(outside.join("précieux.txt"))?, b"keep");
        Ok(())
    }

    #[test]
    fn a_custom_state_directory_loses_only_our_exact_names() -> Result<(), Failure> {
        // Given a custom state directory shared with user files,
        let temp = tempfile::tempdir()?;
        let state = temp.path().join("Mes documents");
        for name in [
            "profiles.sqlite",
            "profiles.sqlite-wal",
            "agent-status.json.tmp",
            "run-123-4-5-6.jsonl",
            "Corbeille/p/run-1/a.txt",
            "versions/run-0/b.txt",
            "native-tasks-12-34/syncengine-resident.xml",
        ] {
            file(&state.join(name), b"ours")?;
        }
        for name in [
            "notes.txt",
            "profiles.sqlite.bak",
            "run-notes.txt",
            "native-tasks-x/y",
        ] {
            file(&state.join(name), b"user")?;
        }
        // When its entries are removed,
        let result = remove_state_entries(&state);
        // Then only our names went, and the folder stays with the user files.
        assert!(result.is_err());
        let mut left: Vec<String> = std::fs::read_dir(&state)?
            .map(|e| Ok(e?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<_, std::io::Error>>()?;
        left.sort();
        assert_eq!(
            left,
            [
                "native-tasks-x",
                "notes.txt",
                "profiles.sqlite.bak",
                "run-notes.txt"
            ]
        );
        // Once alone, the folder itself goes.
        for name in &left {
            remove_tree(&state.join(name))?;
        }
        file(&state.join("desktop.lock"), b"")?;
        remove_state_entries(&state)?;
        assert!(!state.exists());
        Ok(())
    }

    #[test]
    fn a_folder_holding_a_synchronized_root_is_refused() -> Result<(), Failure> {
        // Given a profile whose destination lies inside a folder to delete,
        let temp = tempfile::tempdir()?;
        let state = temp.path().join(STATE_NAME);
        let destination = state.join("destination");
        file(&destination.join("photo.jpg"), b"data")?;
        let inventory = Inventory {
            traces: vec![Trace {
                kind: TraceKind::Folder,
                label: String::new(),
                location: state.display().to_string(),
                bytes: None,
                removal: Removal::Tree(state.clone()),
            }],
            protected: vec![destination.clone()],
            ..Inventory::default()
        };
        // When everything is deleted, Then that folder is refused, untouched.
        let report =
            purge(&inventory, std::time::Duration::from_secs(1)).map_err(|e| e.message.clone())?;
        assert_eq!(report.failed.len(), 1);
        assert!(destination.join("photo.jpg").exists());
        Ok(())
    }

    #[test]
    fn a_running_command_blocks_the_whole_deletion() -> Result<(), Failure> {
        // Given a command holding the state lock,
        let temp = tempfile::tempdir()?;
        let state = temp.path().join(STATE_NAME);
        file(&state.join("desktop.lock"), b"")?;
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(state.join("desktop.lock"))?;
        owner.try_lock()?;
        let inventory = Inventory {
            traces: vec![Trace {
                kind: TraceKind::Folder,
                label: String::new(),
                location: state.display().to_string(),
                bytes: None,
                removal: Removal::Tree(state.clone()),
            }],
            states: vec![state.clone()],
            ..Inventory::default()
        };
        // When deletion is asked, Then it is refused and nothing is removed.
        let error = purge(&inventory, std::time::Duration::from_secs(1))
            .err()
            .ok_or("deletion was not refused")?;
        assert_eq!(error.code, "busy");
        assert!(state.join("desktop.lock").exists());
        Ok(())
    }

    #[test]
    fn the_inventory_finds_a_custom_state_and_counts_the_trash() -> Result<(), Failure> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join("etat");
        file(&state.join("profiles.sqlite"), b"0123456789")?;
        file(&state.join("Corbeille/p/run-1/a.txt"), b"01234")?;
        file(&state.join("notes.txt"), b"user file, not counted")?;
        // Only temporary places: no personal folder, task or value is read.
        let places = Places {
            data_home: Some(temp.path().join("données")),
            lock_root: Some(temp.path().join("SyncEngine/job-locks")),
            temp_dir: Some(temp.path().join("tmp")),
            native: false,
        };
        file(
            &temp.path().join("SyncEngine/job-locks/execution.lock"),
            b"",
        )?;
        file(&temp.path().join("tmp/.syncengine-abc.partial"), b"spool")?;
        file(&temp.path().join("tmp/.syncengine-.partial"), b"not ours")?;
        file(&temp.path().join("tmp/autre.partial"), b"not ours")?;
        let inventory = inventory(&state, Vec::new(), &places).map_err(|e| e.message.clone())?;
        assert_eq!(inventory.traces.len(), 3);
        assert!(
            inventory.traces.iter().any(
                |t| t.removal == Removal::File(temp.path().join("tmp/.syncengine-abc.partial"))
            )
        );
        assert!(
            inventory
                .traces
                .iter()
                .any(|t| t.removal == Removal::Tree(temp.path().join("SyncEngine")))
        );
        let trace = inventory
            .traces
            .iter()
            .find(|t| t.removal == Removal::StateEntries(state.clone()))
            .ok_or("custom state not listed")?;
        assert_eq!(trace.bytes, Some(15));
        assert!(inventory.trash_bytes >= 5);
        Ok(())
    }

    #[test]
    fn systemd_units_give_back_their_state_directory() {
        // Same quoting as `platform_linux` (backslash, quote, % and $ escaped).
        let unit = "[Service]\nType=oneshot\nExecStart=\"/opt/syncengine/syncengine-agent\" tick \"/home/u/donn\\\\ées $$HOME/100%% \\\"x\\\"\" clock\n";
        assert_eq!(
            unit_state(unit),
            Some(PathBuf::from("/home/u/donn\\ées $HOME/100% \"x\""))
        );
        assert_eq!(unit_state("ExecStart=/bin/true"), None);
        assert_eq!(unit_state("ExecStart=\"/a\" tick \"/b%x\" clock"), None);
    }
}
