//! Structured, user-presentable update errors.

/// Class of an update failure, stable for the interface (see [`Self::code`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Network unreachable, DNS failure or timeout: never means « up to date ».
    Offline,
    /// GitHub API request limit reached (403 without remaining quota, or 429).
    RateLimited,
    /// Other HTTP refusal or transport error.
    Http,
    /// No published release yet (404 on `releases/latest`).
    NoRelease,
    /// Unreadable release: invalid JSON, tag that is not a semver version,
    /// draft or pre-release, untrusted download address.
    InvalidRelease,
    /// The release has no archive for the compiled target.
    NoAsset,
    /// The checksum block is absent, malformed, or lacks the archive name.
    ChecksumMissing,
    /// The downloaded file differs from its published checksum or size.
    ChecksumMismatch,
    /// The archive contains anything other than the expected files.
    InvalidArchive,
    /// Download cancelled by the user.
    Cancelled,
    /// Another update is in progress, a leftover file blocks it, or a
    /// synchronization runs.
    Busy,
    /// The program folder cannot be written without elevated rights.
    NotWritable,
    /// A replacement step failed; every file was restored.
    ReplaceFailed,
    /// A replacement step failed and restoring did not complete: the folder
    /// may hold a mix of versions (listed in the message).
    RollbackIncomplete,
    /// Platform or installation layout not supported (renamed executable).
    Unsupported,
    /// Other local storage error.
    Io,
}

impl ErrorKind {
    /// Stable snake_case code shown to and interpreted by the interface.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Offline => "offline",
            Self::RateLimited => "rate_limited",
            Self::Http => "http",
            Self::NoRelease => "no_release",
            Self::InvalidRelease => "invalid_release",
            Self::NoAsset => "no_asset",
            Self::ChecksumMissing => "checksum_missing",
            Self::ChecksumMismatch => "checksum_mismatch",
            Self::InvalidArchive => "invalid_archive",
            Self::Cancelled => "cancelled",
            Self::Busy => "busy",
            Self::NotWritable => "not_writable",
            Self::ReplaceFailed => "replace_failed",
            Self::RollbackIncomplete => "rollback_incomplete",
            Self::Unsupported => "unsupported",
            Self::Io => "io",
        }
    }
}

/// Update failure: a class and a French message with its cause, without
/// credentials or signed URLs (download addresses are stripped from
/// transport errors).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct UpdateError {
    kind: ErrorKind,
    message: String,
}

impl UpdateError {
    /// Builds an error of `kind`.
    #[must_use]
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    /// Failure class.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
    /// Stable code, see [`ErrorKind::code`].
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
    /// Message with its cause.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Appends a secondary failure (for example a temporary file that could
    /// not be removed) without hiding the original cause.
    #[must_use]
    pub(crate) fn also(mut self, extra: impl std::fmt::Display) -> Self {
        self.message = format!("{} ; {extra}", self.message);
        self
    }
}

/// Classifies a local storage error (`PermissionDenied` and read-only media
/// are [`ErrorKind::NotWritable`]).
#[must_use]
pub fn io_error(error: std::io::Error) -> UpdateError {
    let kind = match error.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
            ErrorKind::NotWritable
        }
        _ => ErrorKind::Io,
    };
    UpdateError::new(kind, error.to_string())
}

/// Error for a build whose target has no published archive.
#[must_use]
pub fn unsupported_platform() -> UpdateError {
    UpdateError::new(
        ErrorKind::Unsupported,
        "mise à jour non disponible pour cette plateforme",
    )
}

pub(crate) fn fail(kind: ErrorKind, message: impl Into<String>) -> UpdateError {
    UpdateError::new(kind, message)
}

/// Contextualized local storage error.
pub(crate) fn io_at(what: &str, path: &std::path::Path, error: std::io::Error) -> UpdateError {
    let base = io_error(error);
    UpdateError::new(
        base.kind(),
        format!("{what} {} : {}", path.display(), base.message()),
    )
}
