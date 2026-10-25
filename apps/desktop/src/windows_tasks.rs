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

const OWNERSHIP: &str = r#"function Owned($task,$expectedArguments,$sid) { return $task.Definition.Actions.Count -eq 1 -and [IO.Path]::GetFileName($task.Definition.Actions.Item(1).Path) -eq 'syncengine-agent.exe' -and $task.Definition.Actions.Item(1).Arguments -eq $expectedArguments -and $task.Definition.Principal.UserId -eq $sid -and $task.Definition.Principal.RunLevel -eq 0 -and $task.Definition.Principal.LogonType -eq 3 };"#;

fn registration_script(operations: &str) -> String {
    format!(
        r#"$ErrorActionPreference='Stop'; function ErrorCode($exception) {{ while ($null -ne $exception.InnerException) {{ $exception=$exception.InnerException }}; return $exception.HResult }}; try {{
{OWNERSHIP}
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
        // A new authorized schedule resumes after automatic last-schedule cleanup.
        // Explicit CLI stop remains persistent until the user chooses resume.
        match std::fs::remove_file(state.join("agent-retire.request")) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(fail(e)),
        }
        if !agent.is_file() {
            return Err(Failure::new(
                "scheduler_install",
                "Agent absent : syncengine-agent.exe doit être placé à côté de l’application",
            ));
        }
        // Export canonicalizes the command too (including Windows' verbatim
        // prefix). Compare the same representation when checking a running task.
        let agent = agent.canonicalize().map_err(fail)?;
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
            {
                let event = "resident";
                let xml = std::fs::read_to_string(export.join(format!("syncengine-{event}.xml")))
                    .map_err(fail)?;
                // Bind BOTH principal and trigger before a possible UAC prompt.
                // Elevation with another admin account must never change owner.
                let xml = bind_user(&xml, &sid);
                let invocation = platform_api::AgentInvocation {
                    executable: agent.clone(),
                    state: canonical.clone(),
                };
                operations.push_str(&migration_operations(&invocation, &prefix, &sid, &xml)?);
            }
            Ok(())
        })();
        // Only known generated files, no recursive deletion.
        {
            let event = "resident";
            let path = export.join(format!("syncengine-{event}.xml"));
            if path.exists() {
                std::fs::remove_file(path).map_err(fail)?;
            }
        }
        std::fs::remove_dir(&export).map_err(fail)?;
        rendered?;
    } else {
        std::fs::File::create(state.join("agent-retire.request"))
            .and_then(|f| f.sync_all())
            .map_err(fail)?;
        let invocation = platform_api::AgentInvocation {
            executable: agent.clone(),
            state: canonical.clone(),
        };
        for event in ["resident", "clock", "logon"] {
            let args = if event == "resident" {
                platform_windows::resident_arguments(&invocation)
                    .map_err(|e| Failure::new("scheduler_install", e.to_string()))?
            } else {
                platform_windows::legacy_arguments(&invocation, event)
                    .map_err(|e| Failure::new("scheduler_install", e.to_string()))?
            };
            operations.push_str(&format!("$old=$null; try {{ $old=$folder.GetTask({}) }} catch {{ if ((ErrorCode $_.Exception) -ne -2147024894) {{ throw }} }}; if ($null -ne $old) {{ if (!(Owned $old {} {})) {{ throw 'Tâche étrangère : retrait refusé' }}; $folder.DeleteTask({},0) }};\n", literal(&format!("{prefix}-{event}")),literal(&args),literal(&sid),literal(&format!("{prefix}-{event}"))));
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

fn migration_operations(
    invocation: &platform_api::AgentInvocation,
    prefix: &str,
    sid: &str,
    xml: &str,
) -> Result<String, Failure> {
    let mut operations = String::new();
    let expected_args = platform_windows::resident_arguments(invocation)
        .map_err(|e| Failure::new("scheduler_install", e.to_string()))?;
    operations.push_str(&format!(
                    "$existing=$null; try {{ $existing=$folder.GetTask({}) }} catch {{ if ((ErrorCode $_.Exception) -ne -2147024894) {{ throw }} }}; if ($null -ne $existing) {{ if (!(Owned $existing {} {})) {{ throw 'Tâche résidente étrangère : remplacement refusé' }}; if ($existing.GetInstances(0).Count -gt 0 -and $existing.Definition.Actions.Item(1).Path -ne {}) {{ throw 'Agent actif depuis un ancien emplacement : utilisez stop, attendez son arrêt puis réenregistrez cet horaire' }} }};\n",
                    literal(&format!("{prefix}-resident")), literal(&expected_args), literal(sid), literal(&invocation.executable.to_string_lossy())
                ));
    // TASK_CREATE_OR_UPDATE | TASK_DISABLE: a logon racing migration cannot
    // start the resident before legacy jobs have finished and been removed.
    operations.push_str(&format!(
        "$null=$folder.RegisterTask({}, {}, 14, {}, $null, 3, $null);\n",
        literal(&format!("{prefix}-resident")),
        literal(xml),
        literal(sid)
    ));
    // Disable verified legacy dispatchers before starting the resident.
    // A running job is never killed or declared completed by migration.
    operations.push_str("$legacy=@();\n");
    for old_event in ["clock", "logon"] {
        let args = platform_windows::legacy_arguments(invocation, old_event)
            .map_err(|e| Failure::new("scheduler_install", e.to_string()))?;
        for name in [
            format!("{prefix}-{old_event}"),
            format!("SyncEngine-{old_event}"),
        ] {
            operations.push_str(&format!(
                            "$old=$null; try {{ $old=$folder.GetTask({}) }} catch {{ if ((ErrorCode $_.Exception) -ne -2147024894) {{ throw }} }}; if ($null -ne $old) {{ if (Owned $old {} {}) {{ $old.Enabled=$false; $legacy+=,$old }} else {{ if ({}.StartsWith({})) {{ throw 'Ancienne tâche étrangère : migration refusée' }} }} }};\n",
                            literal(&name), literal(&args), literal(sid), literal(&name), literal(prefix)
                        ));
        }
    }
    operations.push_str("foreach ($old in $legacy) { if ($old.GetInstances(0).Count -gt 0) { throw 'Migration en attente : ancienne exécution en cours. Réenregistrez après sa fin.' } }; foreach ($old in $legacy) { $folder.DeleteTask($old.Name,0) };\n");
    operations.push_str(&format!("$resident=$folder.GetTask({}); $resident.Enabled=$true; if ($resident.GetInstances(0).Count -eq 0) {{ $null=$resident.Run($null) }};\n", literal(&format!("{prefix}-resident"))));
    Ok(operations)
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
            if artifact.name.contains("resident") {
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
    #[test]
    fn migration_is_idempotent_recoverable_and_preserves_foreign_tasks() -> Result<(), Failure> {
        let invocation = AgentInvocation {
            executable: "C:\\portable\\syncengine-agent.exe".into(),
            state: "C:\\etat".into(),
        };
        let xml = platform_windows::WindowsScheduler
            .render(&invocation)
            .map_err(|e| Failure::new("scheduler_install", e.to_string()))?[0]
            .content
            .clone();
        let operations = migration_operations(&invocation, "SyncEngine-test", "S-1-5-21-1", &xml)?;
        let setup = include_str!("windows_tasks_mock.ps1");
        for scenario in ["complete", "running", "denied", "partial", "foreign"] {
            let checks = match scenario {
                "complete" => format!(
                    "{operations} {operations} if ($script:Starts -ne 1 -or $folder.Tasks.ContainsKey('SyncEngine-test-clock') -or !$folder.Tasks.ContainsKey('SyncEngine-logon')) {{ throw 'migration repeat/foreign' }}"
                ),
                "running" => format!(
                    "$folder.Tasks['SyncEngine-test-clock'].State=4; $failed=$false; try {{ {operations} }} catch {{ $failed=$true }}; if (!$failed -or $folder.Tasks['SyncEngine-test-clock'].Enabled -or $script:Starts -ne 0 -or $folder.Tasks['SyncEngine-test-resident'].Enabled) {{ throw 'running job was not deferred' }}; $folder.Tasks['SyncEngine-test-clock'].State=3; {operations} if ($script:Starts -ne 1) {{ throw 'migration did not recover' }}"
                ),
                "denied" => format!(
                    "$folder.Denied=$true; $failed=$false; try {{ {operations} }} catch {{ $failed=$true }}; if (!$failed -or !$folder.Tasks['SyncEngine-test-clock'].Enabled -or $script:Starts -ne 0) {{ throw 'denied changed legacy' }}; $folder.Denied=$false; {operations}"
                ),
                "partial" => format!(
                    "$folder.FailDeletion=$true; $failed=$false; try {{ {operations} }} catch {{ $failed=$true }}; if (!$failed -or $script:Starts -ne 0) {{ throw 'partial migration started resident' }}; $folder.FailDeletion=$false; {operations} if ($script:Starts -ne 1) {{ throw 'partial migration did not recover' }}"
                ),
                _ => format!(
                    "$folder.Tasks['SyncEngine-test-clock'].Definition.Actions.Value.Arguments='foreign state'; $failed=$false; try {{ {operations} }} catch {{ $failed=$true }}; if (!$failed -or !$folder.Tasks.ContainsKey('SyncEngine-test-clock') -or $script:Starts -ne 0) {{ throw 'foreign task overwritten' }}"
                ),
            };
            let output = powershell(&format!(
                "$ErrorActionPreference='Stop'; {setup} {OWNERSHIP} {checks}"
            ))?;
            assert!(
                output.status.success(),
                "{scenario}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}
