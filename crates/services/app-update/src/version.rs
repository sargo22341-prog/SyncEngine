//! Compiled targets, archive names and semver comparison.

use crate::error::{ErrorKind, UpdateError, fail};

/// Portable archive format published for a target (`scripts/release.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// `.zip` (Windows).
    Zip,
    /// `.tar.gz` (Linux), executables in mode 0o755.
    TarGz,
}

/// A target that has a portable archive in every release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    triple: &'static str,
    format: ArchiveFormat,
    exe_suffix: &'static str,
}

/// Executables of the archive, without suffix, in replacement order.
pub(crate) const BINARIES: [&str; 3] = ["syncengine-desktop", "syncengine", "syncengine-agent"];
/// Redistribution texts shipped next to the executables.
pub(crate) const TEXTS: [&str; 2] = ["THIRD_PARTY_NOTICES.txt", "LICENSE.txt"];

impl Target {
    /// Windows x86_64 (MSVC), `.zip` archive.
    pub const WINDOWS_X64: Self = Self {
        triple: "x86_64-pc-windows-msvc",
        format: ArchiveFormat::Zip,
        exe_suffix: ".exe",
    };
    /// Linux x86_64 (GNU), `.tar.gz` archive.
    pub const LINUX_X64: Self = Self {
        triple: "x86_64-unknown-linux-gnu",
        format: ArchiveFormat::TarGz,
        exe_suffix: "",
    };

    /// Target of this build, `None` when no archive is published for it.
    #[must_use]
    pub fn current() -> Option<Self> {
        if cfg!(all(target_arch = "x86_64", windows, target_env = "msvc")) {
            Some(Self::WINDOWS_X64)
        } else if cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) {
            Some(Self::LINUX_X64)
        } else {
            None
        }
    }

    /// Rust target triple, as in the archive name.
    #[must_use]
    pub fn triple(self) -> &'static str {
        self.triple
    }

    /// Archive format of this target.
    #[must_use]
    pub fn format(self) -> ArchiveFormat {
        self.format
    }

    /// Release asset name for `version` (tag without `v`).
    #[must_use]
    pub fn asset_name(self, version: &str) -> String {
        let extension = match self.format {
            ArchiveFormat::Zip => ".zip",
            ArchiveFormat::TarGz => ".tar.gz",
        };
        format!("{}{extension}", self.archive_root(version))
    }

    /// Single top-level folder of the archive.
    #[must_use]
    pub fn archive_root(self, version: &str) -> String {
        format!("SyncEngine_{version}_{}", self.triple)
    }

    /// File name of the interface executable.
    #[must_use]
    pub fn desktop_name(self) -> String {
        format!("{}{}", BINARIES[0], self.exe_suffix)
    }

    /// Exact file names of the archive, in replacement order: executables
    /// first, then the redistribution texts.
    #[must_use]
    pub fn members(self) -> Vec<String> {
        BINARIES
            .iter()
            .map(|name| format!("{name}{}", self.exe_suffix))
            .chain(TEXTS.iter().map(|name| (*name).to_owned()))
            .collect()
    }

    /// Whether `member` is an executable (mode 0o755 under Unix).
    pub(crate) fn is_binary(self, member: &str) -> bool {
        BINARIES
            .iter()
            .any(|name| format!("{name}{}", self.exe_suffix) == member)
    }
}

/// Parses a release tag (`v1.2.3` or `1.2.3`) as semver.
pub(crate) fn parse_tag(tag: &str) -> Result<semver::Version, UpdateError> {
    let text = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(text).map_err(|e| {
        fail(
            ErrorKind::InvalidRelease,
            format!("étiquette de release « {} » invalide : {e}", short(tag)),
        )
    })
}

/// Whether the release `tag` is strictly newer than `current` (semver
/// ordering, never string comparison: `0.1.10` is newer than `0.1.9`).
///
/// # Errors
/// [`ErrorKind::InvalidRelease`] when either value is not a semver version.
pub fn is_newer(current: &str, tag: &str) -> Result<bool, UpdateError> {
    Ok(parse_tag(tag)? > parse_tag(current)?)
}

/// Bounded copy of untrusted text for messages.
pub(crate) fn short(text: &str) -> String {
    text.chars().take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering_is_numeric_not_textual() -> Result<(), UpdateError> {
        assert!(is_newer("0.1.9", "v0.1.10")?);
        assert!(is_newer("0.1.8", "0.2.0")?);
        assert!(!is_newer("0.1.8", "v0.1.8")?);
        assert!(!is_newer("0.2.0", "v0.1.99")?);
        assert!(!is_newer("1.0.0", "v1.0.0-rc.1")?);
        Ok(())
    }

    #[test]
    fn invalid_tags_are_refused() {
        for tag in ["", "v", "latest", "v1.2", "v1.2.3.4", "vv1.2.3", " 1.2.3"] {
            let error = is_newer("0.1.8", tag).err();
            assert_eq!(
                error.map(|e| e.kind()),
                Some(ErrorKind::InvalidRelease),
                "{tag:?}"
            );
        }
    }

    #[test]
    fn names_follow_the_release_script() {
        let windows = Target::WINDOWS_X64;
        assert_eq!(
            windows.asset_name("0.1.9"),
            "SyncEngine_0.1.9_x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(windows.members()[2], "syncengine-agent.exe");
        assert!(windows.is_binary("syncengine.exe"));
        assert!(!windows.is_binary("LICENSE.txt"));
        let linux = Target::LINUX_X64;
        assert_eq!(
            linux.asset_name("0.1.9"),
            "SyncEngine_0.1.9_x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(linux.desktop_name(), "syncengine-desktop");
    }
}
