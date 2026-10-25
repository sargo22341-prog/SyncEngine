//! Windows task registration through Task Scheduler COM. Only access-denied
//! failures request UAC; task actions keep the original user's interactive token.
use crate::backend::Failure;
use std::os::windows::process::CommandExt;
use std::path::Path;

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn bind_user(xml: &str, sid: &str) -> String {
    xml.replace("<LogonType>", &format!("<UserId>{sid}</UserId><LogonType>"))
        .replace(
            "</LogonTrigger>",
            &format!("<UserId>{sid}</UserId></LogonTrigger>"),
        )
}

fn registration_script(operations: &str) -> String {
    format!(
        r#"$ErrorActionPreference='Stop'; function ErrorCode($exception) {{ while ($null -ne $exception.InnerException) {{ $exception=$exception.InnerException }}; return $exception.HResult }}; try {{
$service=New-Object -ComObject 'Schedule.Service'; $service.Connect(); $folder=$service.GetFolder('\');
{operations}
exit 0
}} catch {{ [Console]::Error.WriteLine($_.Exception.ToString()); if ((ErrorCode $_.Exception) -eq -2147024891) {{ exit 5 }}; exit 1 }}"#
    )
}

fn powershell(script: &str) -> Result<std::process::Output, Failure> {
    let script = format!("[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); {script}");
    std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| {
            Failure::new(
                "scheduler_install",
                format!("PowerShell indisponible : {e}"),
            )
        })
}

pub(crate) fn reconcile(state: &Path, needed: bool) -> Result<(), Failure> {
    let fail = |e: std::io::Error| Failure::new("scheduler_install", e.to_string());
    let exe = std::env::current_exe().map_err(fail)?;
    let agent = exe
        .parent()
        .ok_or_else(|| Failure::new("scheduler_install", "Dossier de l’agent absent"))?
        .join("syncengine-agent.exe");
    let identity = powershell(
        "[Console]::Write([System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value)",
    )?;
    let sid = String::from_utf8_lossy(&identity.stdout).trim().to_owned();
    if !identity.status.success()
        || !sid.starts_with("S-1-")
        || !sid
            .chars()
            .all(|c| c.is_ascii_digit() || c == 'S' || c == '-')
    {
        return Err(Failure::new(
            "scheduler_install",
            "Impossible d’identifier le compte Windows",
        ));
    }
    // Stable per account and state directory: isolated installations cannot
    // overwrite each other's dispatchers. Windows paths are case-insensitive.
    let canonical = std::fs::canonicalize(state).map_err(fail)?;
    let hash = blake3::hash(canonical.to_string_lossy().to_lowercase().as_bytes()).to_hex();
    let prefix = format!("SyncEngine-{sid}-{}", &hash[..16]);
    let mut operations = String::new();
    if needed {
        if !agent.is_file() {
            return Err(Failure::new(
                "scheduler_install",
                "Agent absent : syncengine-agent.exe doit être placé à côté de l’application",
            ));
        }
        // A unique temporary export avoids collisions between rapid retries.
        let export = state.join(format!(
            "native-tasks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| Failure::new("scheduler_install", e.to_string()))?
                .as_nanos()
        ));
        std::fs::create_dir(&export).map_err(fail)?;
        let rendered = (|| {
            let output = std::process::Command::new(&agent)
                .arg("export")
                .arg(state)
                .arg(&export)
                .arg("windows")
                .creation_flags(0x0800_0000)
                .output()
                .map_err(fail)?;
            if !output.status.success() {
                return Err(Failure::new(
                    "scheduler_install",
                    format!(
                        "Export des tâches refusé : {} {}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    ),
                ));
            }
            for event in ["clock", "logon"] {
                let xml = std::fs::read_to_string(export.join(format!("syncengine-{event}.xml")))
                    .map_err(fail)?;
                // Bind BOTH principal and trigger before a possible UAC prompt.
                // Elevation with another admin account must never change owner.
                let xml = bind_user(&xml, &sid);
                operations.push_str(&format!(
                    "$null=$folder.RegisterTask({}, {}, 6, {}, $null, 3, $null);\n",
                    literal(&format!("{prefix}-{event}")),
                    literal(&xml),
                    literal(&sid)
                ));
                // Migrate only legacy tasks with exactly the same action.
                operations.push_str(&format!(
                    "$old=$null; try {{ $old=$folder.GetTask({}) }} catch {{ if ((ErrorCode $_.Exception) -ne -2147024894) {{ throw }} }}; if ($null -ne $old) {{ [xml]$expected={}; $action=$old.Definition.Actions.Item(1); if ($action.Path -eq $expected.Task.Actions.Exec.Command -and $action.Arguments -eq $expected.Task.Actions.Exec.Arguments) {{ $folder.DeleteTask({},0) }} }};\n",
                    literal(&format!("SyncEngine-{event}")), literal(&xml), literal(&format!("SyncEngine-{event}"))
                ));
            }
            Ok(())
        })();
        // Only known generated files, no recursive deletion.
        for event in ["clock", "logon"] {
            let path = export.join(format!("syncengine-{event}.xml"));
            if path.exists() {
                std::fs::remove_file(path).map_err(fail)?;
            }
        }
        std::fs::remove_dir(&export).map_err(fail)?;
        rendered?;
    } else {
        for event in ["clock", "logon"] {
            operations.push_str(&format!("try {{ $folder.DeleteTask({},0) }} catch {{ if ((ErrorCode $_.Exception) -ne -2147024894) {{ throw }} }};\n", literal(&format!("{prefix}-{event}"))));
        }
    }
    // No mutable script/XML is read by the elevated helper. The exact same
    // command is retried. Partial installs are harmless and retries idempotent.
    let script = registration_script(&operations);
    let output = powershell(&script)?;
    if output.status.success() {
        return Ok(());
    }
    if output.status.code() != Some(5) {
        return Err(Failure::new(
            "scheduler_install",
            format!(
                "Planificateur Windows : {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }
    let elevated = powershell(&format!(
        r#"$ErrorActionPreference='Stop'; try {{
$encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes({}));
$p=Start-Process -FilePath "$PSHOME\powershell.exe" -Verb RunAs -WindowStyle Hidden -ArgumentList @('-NoProfile','-NonInteractive','-WindowStyle','Hidden','-EncodedCommand',$encoded) -Wait -PassThru;
exit $p.ExitCode
}} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}"#,
        literal(&script)
    ))?;
    if elevated.status.success() {
        Ok(())
    } else {
        Err(Failure::new(
            "scheduler_install",
            format!(
                "Enregistrement Windows non terminé (UAC refusée ou erreur du Planificateur, code {:?}). Réenregistrez la planification pour réessayer. {}",
                elevated.status.code(),
                String::from_utf8_lossy(&elevated.stderr)
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_api::{AgentInvocation, ScheduleAdapter};

    #[test]
    fn only_access_denied_requests_elevation_and_preserves_diagnostics() -> Result<(), Failure> {
        for (hresult, expected) in [(-2147024891_i32, 5), (-2147024809_i32, 1)] {
            // Throw a wrapped COM failure without changing the Task Scheduler.
            let operations = format!(
                "throw [System.Exception]::new('registration refused', [System.Runtime.InteropServices.COMException]::new('native diagnostic', {hresult}))"
            );
            let output = powershell(&registration_script(&operations))?;
            assert_eq!(output.status.code(), Some(expected));
            assert!(String::from_utf8_lossy(&output.stderr).contains("native diagnostic"));
        }
        Ok(())
    }

    #[test]
    fn account_bound_tasks_pass_native_validation_without_installation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let identity = powershell(
            "[Console]::Write([System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value)",
        ).map_err(|e| std::io::Error::other(e.to_string()))?;
        assert!(identity.status.success());
        let sid = String::from_utf8(identity.stdout)?;
        let invocation = AgentInvocation {
            executable: std::env::current_exe()?,
            state: temp.path().join("l'utilisateur & état $HOME"),
        };
        for artifact in platform_windows::WindowsScheduler.render(&invocation)? {
            let xml = bind_user(&artifact.content, &sid);
            assert!(xml.contains(&format!(
                "<UserId>{sid}</UserId><LogonType>InteractiveToken"
            )));
            assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
            if artifact.name.contains("logon") {
                assert!(xml.contains(&format!("<UserId>{sid}</UserId></LogonTrigger>")));
            }
            // TASK_VALIDATE_ONLY=1: the service parses and validates the real
            // schema but creates no task and never executes the action.
            let output = powershell(&format!(
                "$ErrorActionPreference='Stop'; $s=New-Object -ComObject Schedule.Service; $s.Connect(); $null=$s.GetFolder('\\').RegisterTask('SyncEngine-validation-only',{},1,{},$null,3,$null)",
                literal(&xml),
                literal(&sid)
            )).map_err(|e| std::io::Error::other(e.to_string()))?;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
    #[test]
    fn powershell_literals_preserve_untrusted_paths() -> Result<(), Failure> {
        let value = "C:\\l'utilisateur\\$HOME; $(exit 9) & état";
        let output = powershell(&format!("[Console]::Write({})", literal(value)))?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), value);
        Ok(())
    }
}
