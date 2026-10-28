//! Desktop OS integration in pure Rust: native file dialogs (rfd), the
//! per-user Windows « Run » registry value (winreg) and installation of the
//! application-owned agent tasks (Task Scheduler COM via Windows PowerShell).

use crate::backend::Failure;
use std::path::{Path, PathBuf};

/// Native folder picker. `None` when the user cancels.
pub(crate) fn pick_folder(title: &str, start: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = start.filter(|p| p.is_absolute()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// Native file picker (known_hosts, CA certificate).
pub(crate) fn pick_file(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).pick_file()
}

/// Native « save as » dialog proposing `name`. `None` when the user cancels.
pub(crate) fn pick_save_file(title: &str, name: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .set_file_name(name)
        .save_file()
}

/// Shows a folder in the system file manager (Explorer, or `xdg-open`).
/// The viewer is detached: its own exit status is not awaited.
pub(crate) fn open_folder(path: &Path) -> Result<(), Failure> {
    if !path.is_dir() {
        return Err(Failure::new(
            "unavailable",
            format!("dossier introuvable : {}", path.display()),
        ));
    }
    let viewer = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(viewer)
        .arg(path)
        .spawn()
        .map(drop)
        .map_err(|e| Failure::new("unavailable", format!("ouverture du dossier : {e}")))
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "SyncEngine";

#[cfg(windows)]
fn os(error: impl std::fmt::Display) -> Failure {
    Failure::new("unavailable", format!("intégration système : {error}"))
}

/// Whether the window opens at Windows session start.
#[cfg(windows)]
pub(crate) fn startup_enabled() -> Result<bool, Failure> {
    Ok(startup_command()?.is_some_and(|value| !value.is_empty()))
}

/// Command of the current-user Run value `SyncEngine`, when present.
#[cfg(windows)]
pub(crate) fn startup_command() -> Result<Option<String>, Failure> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ) {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(os(e)),
    };
    match key.get_value::<String, _>(RUN_VALUE) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(os(e)),
    }
}

#[cfg(not(windows))]
pub(crate) fn startup_enabled() -> Result<bool, Failure> {
    Ok(false)
}

/// Adds/removes the current-user Run value pointing at this executable.
#[cfg(windows)]
pub(crate) fn startup_save(enabled: bool) -> Result<(), Failure> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(RUN_KEY)
        .map_err(os)?;
    if enabled {
        let exe = std::env::current_exe().map_err(os)?;
        let exe = exe
            .to_str()
            .ok_or_else(|| os("chemin de l’exécutable non UTF-8"))?;
        key.set_value(RUN_VALUE, &format!("\"{exe}\"")).map_err(os)
    } else {
        match key.delete_value(RUN_VALUE) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(os(e)),
            _ => Ok(()),
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn startup_save(_enabled: bool) -> Result<(), Failure> {
    Err(Failure::new(
        "not_supported",
        "Démarrage automatique de l’interface disponible uniquement sous Windows",
    ))
}

/// Removes shared dispatchers after the final active authorization is removed.
pub(crate) fn reconcile_agent(app: &sync_orchestrator::Application) -> Result<(), Failure> {
    #[cfg(windows)]
    {
        let needed = app
            .schedules()
            .map_err(Failure::from)?
            .iter()
            .any(|s| s.schedule.enabled);
        if needed {
            Ok(())
        } else {
            crate::windows_tasks::reconcile(app.state_directory(), false)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Ok(())
    }
}

/// Ensures dispatchers exist before authorizing a new scheduled execution.
pub(crate) fn prepare_agent(state: &Path) -> Result<(), Failure> {
    #[cfg(windows)]
    {
        crate::windows_tasks::reconcile(state, true)
    }
    #[cfg(not(windows))]
    {
        let _ = state;
        Ok(())
    }
}

fn app_error(e: Failure) -> sync_orchestrator::AppError {
    sync_orchestrator::AppError {
        code: e.code,
        message: e.message,
        profile: None,
    }
}

pub(crate) fn prepare_schedule(
    app: &sync_orchestrator::Application,
) -> Result<(), sync_orchestrator::AppError> {
    prepare_agent(app.state_directory()).map_err(app_error)
}

pub(crate) fn remove_schedule(
    app: &sync_orchestrator::Application,
    id: sync_core::ProfileId,
) -> Result<(), sync_orchestrator::AppError> {
    app.schedule_remove(id)?;
    reconcile_agent(app).map_err(app_error)
}
