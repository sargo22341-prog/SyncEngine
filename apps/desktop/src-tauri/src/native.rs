//! Desktop OS integration. Fixed scripts; user data travels only via environment.
use crate::{AppError, transport};
#[cfg(windows)]
fn powershell(script: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("powershell.exe");
    command.creation_flags(0x0800_0000);
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-STA",
        "-WindowStyle",
        "Hidden",
        "-Command",
        script,
    ]);
    command
}
#[cfg(windows)]
fn output(mut command: std::process::Command) -> Result<String, AppError> {
    let result = command.output().map_err(transport)?;
    if !result.status.success() {
        return Err(transport("Intégration Windows indisponible ou refusée"));
    }
    String::from_utf8(result.stdout)
        .map_err(transport)
        .map(|s| s.trim().to_owned())
}
#[cfg(windows)]
pub(super) fn browse() -> Result<Option<String>, AppError> {
    let text = output(powershell(
        r#"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new(); Add-Type -AssemblyName System.Windows.Forms; $picker=[System.Windows.Forms.FolderBrowserDialog]::new(); $picker.Description='Choisir un dossier SyncEngine'; $picker.ShowNewFolderButton=$false; try { if ($picker.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Write($picker.SelectedPath) } } finally { $picker.Dispose() }"#,
    ))?;
    Ok(if text.is_empty() { None } else { Some(text) })
}
#[cfg(not(windows))]
pub(super) fn browse() -> Result<Option<String>, AppError> {
    let result = std::process::Command::new("zenity")
        .args([
            "--file-selection",
            "--directory",
            "--title=Choisir un dossier SyncEngine",
        ])
        .output()
        .map_err(|_| {
            transport("Sélecteur indisponible : installer zenity ou saisir le chemin du dossier")
        })?;
    if result.status.code() == Some(1) {
        return Ok(None);
    }
    if !result.status.success() {
        return Err(transport("Échec du sélecteur de dossiers"));
    }
    Ok(Some(
        String::from_utf8(result.stdout)
            .map_err(transport)?
            .trim_end_matches(['\r', '\n'])
            .into(),
    ))
}
#[cfg(windows)]
pub(super) fn startup_enabled() -> Result<bool, AppError> {
    Ok(output(powershell(
        r#"$ErrorActionPreference='Stop'; $value=Get-ItemPropertyValue -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'SyncEngine' -ErrorAction SilentlyContinue; [Console]::Write([bool]$value)"#,
    ))? == "True")
}
#[cfg(not(windows))]
pub(super) fn startup_enabled() -> Result<bool, AppError> {
    Ok(false)
}
#[cfg(windows)]
pub(super) fn startup_save(enabled: bool) -> Result<(), AppError> {
    let mut command = powershell(if enabled {
        r#"$ErrorActionPreference='Stop'; $key='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'; New-Item -Path $key -Force | Out-Null; New-ItemProperty -LiteralPath $key -Name 'SyncEngine' -PropertyType String -Value ('"'+$env:SYNCENGINE_DESKTOP_EXE+'"') -Force | Out-Null"#
    } else {
        r#"$ErrorActionPreference='Stop'; $key='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'; if(Get-ItemProperty -LiteralPath $key -Name 'SyncEngine' -ErrorAction SilentlyContinue) { Remove-ItemProperty -LiteralPath $key -Name 'SyncEngine' }"#
    });
    command.env(
        "SYNCENGINE_DESKTOP_EXE",
        std::env::current_exe().map_err(transport)?,
    );
    output(command).map(|_| ())
}
#[cfg(not(windows))]
pub(super) fn startup_save(_enabled: bool) -> Result<(), AppError> {
    Err(transport(
        "Démarrage automatique de l’interface disponible uniquement sous Windows",
    ))
}

/// Installs only the two application-owned current-user tasks. Export remains
/// inspectable under application state; a failed second install is reported as
/// partial and can be retried. No profile or synchronization data is mutated.
#[cfg(windows)]
pub(super) fn activate_agent(state: &std::path::Path) -> Result<(), AppError> {
    let executable = std::env::current_exe().map_err(transport)?;
    let parent = executable
        .parent()
        .ok_or_else(|| transport("Dossier d’installation absent"))?;
    let agent = parent.join("syncengine-agent.exe");
    if !agent.is_file() {
        return Err(transport(
            "Compagnon agent absent : utiliser l’installation complète SyncEngine",
        ));
    }
    let export = state.join(format!(
        "native-tasks-{}",
        sync_core::Timestamp::now().map_err(transport)?.secs()
    ));
    std::fs::create_dir(&export).map_err(transport)?;
    let mut command = powershell(
        r#"$ErrorActionPreference='Stop'; & $env:SYNCENGINE_AGENT export $env:SYNCENGINE_STATE $env:SYNCENGINE_EXPORT windows | Out-Null; if ($LASTEXITCODE -ne 0) { throw 'Export failed' }; foreach ($event in @('clock','logon')) { $file=Join-Path $env:SYNCENGINE_EXPORT ('syncengine-'+$event+'.xml'); & schtasks.exe /Create /F /TN ('SyncEngine-'+$event) /XML $file | Out-Null; if ($LASTEXITCODE -ne 0) { throw 'Installation incomplete' } }"#,
    );
    command
        .env("SYNCENGINE_AGENT", agent)
        .env("SYNCENGINE_STATE", state)
        .env("SYNCENGINE_EXPORT", export);
    output(command).map(|_| ()).map_err(|_|transport("Activation incomplète des tâches Windows. Vérifiez le Planificateur de tâches puis réessayez. Les horaires enregistrés sont conservés."))
}
#[cfg(not(windows))]
pub(super) fn activate_agent(_state: &std::path::Path) -> Result<(), AppError> {
    Err(transport(
        "Activation graphique de l’agent disponible uniquement sous Windows ; utiliser le guide systemd utilisateur sous Linux",
    ))
}
