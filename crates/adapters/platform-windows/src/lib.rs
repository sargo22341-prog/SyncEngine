//! Task Scheduler 2.0 definitions; no COM, shell, credentials or GUI dependency.
use platform_api::{
    AgentInvocation, Artifact, PlatformError, ScheduleAdapter, ScheduleCapabilities, SessionEvent,
};
/// Least-privilege current-user Task Scheduler adapter.
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
        let mut artifacts = Vec::new();
        for (name, trigger, event) in [
            (
                "syncengine-clock.xml",
                "<TimeTrigger><Repetition><Interval>PT1M</Interval><StopAtDurationEnd>false</StopAtDurationEnd></Repetition><StartBoundary>2026-01-01T00:00:00Z</StartBoundary><Enabled>true</Enabled></TimeTrigger>",
                "clock",
            ),
            (
                "syncengine-logon.xml",
                "<LogonTrigger><Enabled>true</Enabled></LogonTrigger>",
                "logon",
            ),
        ] {
            let args = xml(&format!("tick {} {event}", argument(state)));
            artifacts.push(Artifact { name, content:format!(r#"<?xml version="1.0"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
<Triggers>{trigger}</Triggers>
<Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
<Actions Context="Author"><Exec><Command>{}</Command><Arguments>{args}</Arguments></Exec></Actions>
</Task>
"#,xml(exe)) });
        }
        Ok(artifacts)
    }
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
