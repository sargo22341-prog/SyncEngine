//! Native Task Scheduler validation only: no persistent task is installed.
#![cfg(windows)]
use platform_api::{AgentInvocation, ScheduleAdapter};
#[test]
fn windows_scheduler_validates_xml_with_native_com() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let invocation = AgentInvocation {
        executable: std::env::current_exe()?,
        state: temp.path().join("état & espaces"),
    };
    for artifact in platform_windows::WindowsScheduler.render(&invocation)? {
        let path = temp.path().join(artifact.name);
        std::fs::write(&path, artifact.content)?;
        let script = r#"$ErrorActionPreference='Stop'; $service=New-Object -ComObject 'Schedule.Service'; $service.Connect(); $folder=$service.GetFolder('\'); $xml=[System.IO.File]::ReadAllText($env:SYNCENGINE_TEST_XML); $null=$folder.RegisterTask('SyncEngine-validation-only',$xml,1,$null,$null,3,$null)"#;
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("SYNCENGINE_TEST_XML", &path)
            .output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}
