//! systemd --user artifacts. Runtime availability is checked by the installer.
use platform_api::{
    AgentInvocation, Artifact, PlatformError, ScheduleAdapter, ScheduleCapabilities, SessionEvent,
};
/// Per-user systemd timer; no system-service/root requirement.
pub struct LinuxScheduler;
fn quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    )
}
impl ScheduleAdapter for LinuxScheduler {
    fn capabilities(&self) -> ScheduleCapabilities {
        ScheduleCapabilities {
            clock: true,
            session: Some(SessionEvent::UserManagerStart),
        }
    }
    fn render(&self, invocation: &AgentInvocation) -> Result<Vec<Artifact>, PlatformError> {
        invocation.validate()?;
        let exe = quote(
            invocation
                .executable
                .to_str()
                .ok_or(PlatformError::Invalid("executable UTF-8"))?,
        );
        let state = quote(
            invocation
                .state
                .to_str()
                .ok_or(PlatformError::Invalid("state UTF-8"))?,
        );
        let service = |event| {
            format!(
                "[Unit]\nDescription=SyncEngine headless profiles\n[Service]\nType=oneshot\nExecStart={exe} tick {state} {event}\nTimeoutStartSec=infinity\nUMask=0077\n[Install]\nWantedBy=default.target\n"
            )
        };
        Ok(vec![
            Artifact{name:"syncengine-clock.service",content:service("clock")},
            Artifact{name:"syncengine-clock.timer",content:"[Unit]\nDescription=SyncEngine schedule dispatcher\n[Timer]\nOnCalendar=*-*-* *:*:00 UTC\nPersistent=true\nAccuracySec=1s\n[Install]\nWantedBy=timers.target\n".into()},
            Artifact{name:"syncengine-session.service",content:service("manager-start")},
        ])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_expansion_and_shell_characters_are_literal() {
        assert_eq!(quote("/a %n/$HOME/\"b\\"), "\"/a %%n/$$HOME/\\\"b\\\\\"");
        assert_eq!(
            LinuxScheduler.capabilities().session,
            Some(SessionEvent::UserManagerStart)
        );
    }
}
