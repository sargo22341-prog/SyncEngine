//! Desktop OS integration in pure Rust: native file dialogs (rfd), the
//! per-user Windows « Run » registry value (winreg) and installation of the
//! two application-owned agent tasks. No script interpreter is involved.

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

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const RUN_VALUE: &str = "SyncEngine";

fn os(error: impl std::fmt::Display) -> Failure {
    Failure::new("unavailable", format!("intégration système : {error}"))
}

/// Whether the window opens at Windows session start.
#[cfg(windows)]
pub(crate) fn startup_enabled() -> Result<bool, Failure> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ) {
        Ok(key) => key,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(os(e)),
    };
    match key.get_value::<String, _>(RUN_VALUE) {
        Ok(value) => Ok(!value.is_empty()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
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

/// Installs only the two application-owned current-user tasks from the
/// adjacent agent. The export stays inspectable under the state directory;
/// a failed second installation is reported as partial and can be retried.
#[cfg(windows)]
pub(crate) fn activate_agent(state: &Path) -> Result<(), Failure> {
    use std::os::windows::process::CommandExt;
    const NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().map_err(os)?;
    let agent = exe
        .parent()
        .ok_or_else(|| os("dossier d’installation absent"))?
        .join("syncengine-agent.exe");
    if !agent.is_file() {
        return Err(Failure::new(
            "unavailable",
            "Compagnon agent absent : utiliser l’installation complète de SyncEngine",
        ));
    }
    let stamp = crate::model::now_secs();
    let export = state.join(format!("native-tasks-{stamp}"));
    std::fs::create_dir(&export).map_err(os)?;
    let status = std::process::Command::new(&agent)
        .arg("export")
        .arg(state)
        .arg(&export)
        .arg("windows")
        .creation_flags(NO_WINDOW)
        .status()
        .map_err(os)?;
    if !status.success() {
        return Err(Failure::new(
            "unavailable",
            "Export des tâches refusé par l’agent",
        ));
    }
    let mut installed = Vec::new();
    for event in ["clock", "logon"] {
        let file = export.join(format!("syncengine-{event}.xml"));
        let status = std::process::Command::new("schtasks.exe")
            .args(["/Create", "/F", "/TN"])
            .arg(format!("SyncEngine-{event}"))
            .arg("/XML")
            .arg(&file)
            .creation_flags(NO_WINDOW)
            .status()
            .map_err(os)?;
        if !status.success() {
            return Err(Failure::new(
                "unavailable",
                format!(
                    "Activation incomplète ({}). Vérifiez le Planificateur de tâches puis réessayez ; les horaires enregistrés sont conservés.",
                    if installed.is_empty() {
                        "aucune tâche installée".to_owned()
                    } else {
                        format!("installée : {}", installed.join(", "))
                    }
                ),
            ));
        }
        installed.push(event);
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn activate_agent(_state: &Path) -> Result<(), Failure> {
    Err(Failure::new(
        "not_supported",
        "Sous Linux, activez les unités systemd utilisateur décrites dans le guide de l’agent",
    ))
}
