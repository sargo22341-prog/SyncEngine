//! Native scheduling definitions. Adapters render artifacts, never install them.
use std::path::PathBuf;

/// Session event with deliberately distinct OS semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    /// Windows user logon, not logoff.
    Logon,
    /// Linux user manager startup; not every graphical login.
    UserManagerStart,
}
/// Available native triggers. No adapter promises logoff completion.
#[derive(Debug, Clone, Copy)]
pub struct ScheduleCapabilities {
    /// Time polling supported by the native scheduler.
    pub clock: bool,
    /// Supported session event, if any.
    pub session: Option<SessionEvent>,
}
/// Fully qualified executable and existing state directory, without credentials.
#[derive(Debug, Clone)]
pub struct AgentInvocation {
    /// Native executable path, passed directly, never through a shell.
    pub executable: PathBuf,
    /// Application-owned state directory outside synchronized roots.
    pub state: PathBuf,
}
/// One inspectable installation artifact. Names are adapter-controlled constants.
#[derive(Debug, Clone)]
pub struct Artifact {
    /// Relative filename, never derived from user input.
    pub name: &'static str,
    /// UTF-8 definition to review before explicit installation.
    pub content: String,
}
/// Rendering failure; unsupported inputs are not silently transformed.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// Native quoting or path constraints cannot represent the invocation safely.
    #[error("invalid native invocation: {0}")]
    Invalid(&'static str),
}
impl AgentInvocation {
    /// Validates absolute UTF-8 paths and rejects control characters.
    /// # Errors
    /// Relative/non-UTF-8 paths or control characters.
    pub fn validate(&self) -> Result<(), PlatformError> {
        for path in [&self.executable, &self.state] {
            let text = path
                .to_str()
                .ok_or(PlatformError::Invalid("UTF-8 required"))?;
            if !path.is_absolute() || text.chars().any(char::is_control) {
                return Err(PlatformError::Invalid(
                    "absolute paths without controls required",
                ));
            }
        }
        Ok(())
    }
}
/// Native adapter contract; caller installs explicitly using OS tools.
pub trait ScheduleAdapter {
    /// Reports semantics implemented by this adapter, not runtime availability.
    fn capabilities(&self) -> ScheduleCapabilities;
    /// Renders native definitions for clock and supported session events.
    /// # Errors
    /// Invocation cannot be represented safely on this platform.
    fn render(&self, invocation: &AgentInvocation) -> Result<Vec<Artifact>, PlatformError>;
}
