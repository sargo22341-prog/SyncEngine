//! Native offline unit validation; no services are installed or started.
#![cfg(target_os = "linux")]
use platform_api::{AgentInvocation, ScheduleAdapter};
#[test]
fn systemd_analyze_verifies_rendered_units() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let invocation = AgentInvocation {
        executable: "/bin/true".into(),
        state: temp.path().join("état %n $HOME"),
    };
    let artifacts = platform_linux::LinuxScheduler.render(&invocation)?;
    let mut command = std::process::Command::new("systemd-analyze");
    command.args(["verify", "--man=no"]);
    for artifact in artifacts {
        let path = temp.path().join(artifact.name);
        std::fs::write(&path, artifact.content)?;
        command.arg(path);
    }
    let result = command.output()?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(())
}
