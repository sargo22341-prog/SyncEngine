//! Task Scheduler 2.0 definitions and a bounded current-session WTS query.
//! Native runtime access stays here, without synchronization policy or GUI.
use platform_api::{
    AgentInvocation, Artifact, PlatformError, ScheduleAdapter, ScheduleCapabilities, SessionEvent,
};
/// Least-privilege current-user resident Task Scheduler adapter.
pub struct WindowsScheduler;
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
/// Quotes one argument using Windows CommandLineToArgvW rules.
fn argument(s: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in s.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            if c == '"' { slashes * 2 + 1 } else { slashes },
        ));
        result.push(c);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}
impl ScheduleAdapter for WindowsScheduler {
    fn capabilities(&self) -> ScheduleCapabilities {
        ScheduleCapabilities {
            clock: true,
            session: Some(SessionEvent::Logon),
        }
    }
    fn render(&self, invocation: &AgentInvocation) -> Result<Vec<Artifact>, PlatformError> {
        invocation.validate()?;
        let exe = invocation
            .executable
            .to_str()
            .ok_or(PlatformError::Invalid("executable UTF-8"))?;
        let state = invocation
            .state
            .to_str()
            .ok_or(PlatformError::Invalid("state UTF-8"))?;
        let args = xml(&format!("resident {}", argument(state)));
        Ok(vec![Artifact {
            name: "syncengine-resident.xml",
            content: format!(
                r#"<?xml version="1.0"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
<Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
<Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure></Settings>
<Actions Context="Author"><Exec><Command>{}</Command><Arguments>{args}</Arguments></Exec></Actions>
</Task>
"#,
                xml(exe)
            ),
        }])
    }
}
/// Identifies the current interactive Windows session using WTS, not an
/// authentication LUID. The identity includes the full real logon FILETIME and
/// the session ID, so reboot-local ID reuse does not replay a previous login.
/// A different authentication context in the same desktop session has the same
/// identity. No fallback invents a login if WTS is unavailable.
/// # Errors
/// PowerShell/WTS unavailable, no interactive session, timeout or invalid data.
#[cfg(windows)]
pub fn interactive_session() -> Result<(String, i64), Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt;
    let mut child = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("session.ps1"),
        ])
        .creation_flags(0x0800_0000)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("Windows session query timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err("Windows WTS interactive session unavailable".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let (session, ticks) = text
        .trim()
        .split_once(':')
        .ok_or("invalid Windows session response")?;
    let session: u32 = session.parse()?;
    let ticks: i64 = ticks.parse()?;
    let seconds = ticks / 10_000_000 - 11_644_473_600;
    if session == 0 || seconds < 0 {
        return Err("interactive Windows logon required".into());
    }
    Ok((format!("wts:{session}:{ticks}"), seconds))
}

/// Quotes arguments for verifying ownership of a historical one-shot task.
/// Never use these to install a new task; Windows now launches `resident`.
/// # Errors
/// Invocation cannot be represented safely.
pub fn legacy_arguments(
    invocation: &AgentInvocation,
    event: &str,
) -> Result<String, PlatformError> {
    invocation.validate()?;
    if !matches!(event, "clock" | "logon") {
        return Err(PlatformError::Invalid("legacy event"));
    }
    Ok(format!(
        "tick {} {event}",
        argument(
            invocation
                .state
                .to_str()
                .ok_or(PlatformError::Invalid("state UTF-8"))?
        )
    ))
}

/// Quotes the resident command for native ownership checks and installation.
/// # Errors
/// Invocation cannot be represented safely.
pub fn resident_arguments(invocation: &AgentInvocation) -> Result<String, PlatformError> {
    invocation.validate()?;
    Ok(format!(
        "resident {}",
        argument(
            invocation
                .state
                .to_str()
                .ok_or(PlatformError::Invalid("state UTF-8"))?
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trailing_slashes_and_quotes_are_preserved() {
        assert_eq!(argument("C:\\a b\\"), "\"C:\\a b\\\\\"");
        assert_eq!(argument("a\"b"), "\"a\\\"b\"");
    }
    #[test]
    fn capabilities_never_claim_logoff() {
        assert_eq!(
            WindowsScheduler.capabilities().session,
            Some(SessionEvent::Logon)
        );
    }
}
