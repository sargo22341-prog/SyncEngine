//! Linux agent supervision through `systemd --user` (presentation side).
//!
//! The units are rendered by `platform-linux` (never by hand here), written
//! under `$XDG_CONFIG_HOME/systemd/user` with our exact names only, then
//! enabled with `systemctl --user`. The displayed agent state is read back
//! from the files and from `systemctl --user is-active`, never assumed.
//! Without systemd (or without a user manager), nothing is installed and the
//! interface says so; the documented fallback is cron (`docs/PLANIFICATION.md`).
//!
//! Order on removal (audit Linux M5): `disable --now` while the unit files
//! still exist, then the files and links, then `daemon-reload`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::Failure;

/// Unit files of the agent, with the text that proves they are ours.
pub(crate) const UNITS: [(&str, &str); 3] = [
    (
        "syncengine-clock.service",
        "Description=SyncEngine headless profiles",
    ),
    (
        "syncengine-session.service",
        "Description=SyncEngine headless profiles",
    ),
    (
        "syncengine-clock.timer",
        "Description=SyncEngine schedule dispatcher",
    ),
];
/// Links created by `systemctl --user enable` for those units.
pub(crate) const UNIT_LINKS: [&str; 3] = [
    "timers.target.wants/syncengine-clock.timer",
    "default.target.wants/syncengine-clock.service",
    "default.target.wants/syncengine-session.service",
];
const TIMER: &str = "syncengine-clock.timer";
const SESSION: &str = "syncengine-session.service";

/// Real state of the Linux agent, as observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentState {
    /// Units installed and the timer is active: schedules run.
    Active,
    /// Units installed but the timer is not active.
    Inactive,
    /// No (or not all) unit files: schedules never run.
    NotInstalled,
    /// `systemctl --user` is missing or has no user manager.
    Unavailable,
}

impl AgentState {
    /// Short label for the side panel.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Active => "Agent actif",
            Self::Inactive => "Agent inactif",
            Self::NotInstalled => "Agent non installé",
            Self::Unavailable => "systemd indisponible",
        }
    }

    /// One sentence for the side panel and the Schedules page.
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Active => "Minuteur systemd utilisateur actif",
            Self::Inactive => "Minuteur systemd installé mais arrêté",
            Self::NotInstalled => "Enregistrer une planification installe le minuteur systemd",
            Self::Unavailable => "Aucune planification ne se lance ; voir le guide (cron)",
        }
    }
}

/// `systemctl` runner; tests replace the lookup `PATH` of the child only.
pub(crate) struct Systemctl {
    path: Option<OsString>,
    env: Vec<(OsString, OsString)>,
}

impl Systemctl {
    /// The system `systemctl`, found through the process `PATH`.
    pub(crate) fn system() -> Self {
        Self {
            path: None,
            env: Vec::new(),
        }
    }

    /// A fake `systemctl` found first in `path` (tests).
    #[cfg(test)]
    pub(crate) fn with_path(path: &Path, env: &[(&str, &Path)]) -> Self {
        Self {
            path: Some(path.as_os_str().to_owned()),
            env: env
                .iter()
                .map(|(k, v)| (OsString::from(k), v.as_os_str().to_owned()))
                .collect(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("systemctl");
        if let Some(path) = &self.path {
            // Rust looks the program up in the child's PATH when it is set.
            command.env("PATH", path);
        }
        command.envs(self.env.iter().map(|(k, v)| (k, v)));
        command.args(args);
        command
    }

    /// Runs `systemctl args…`; `Err` holds a readable cause.
    pub(crate) fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = self
            .command(args)
            .output()
            .map_err(|e| format!("systemctl {} : {e}", args.join(" ")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "systemctl {} : {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// `$XDG_CONFIG_HOME/systemd/user`, or `~/.config/systemd/user`.
pub(crate) fn units_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .filter(|p| p.is_absolute())
        .map(|config| config.join("systemd/user"))
}

fn ours(dir: &Path, name: &str, marker: &str) -> bool {
    std::fs::read_to_string(dir.join(name)).is_ok_and(|text| text.contains(marker))
}

/// Observed state: files first, then the timer in the user manager.
pub(crate) fn state(systemctl: &Systemctl, dir: &Path) -> AgentState {
    if !UNITS.iter().all(|(name, marker)| ours(dir, name, marker)) {
        return AgentState::NotInstalled;
    }
    // `is-active` exits non-zero for an inactive unit: read its output.
    match systemctl.command(&["--user", "is-active", TIMER]).output() {
        Err(_) => AgentState::Unavailable,
        Ok(out) => match String::from_utf8_lossy(&out.stdout).trim() {
            "active" => AgentState::Active,
            "inactive" | "failed" | "activating" | "deactivating" => AgentState::Inactive,
            _ => AgentState::Unavailable,
        },
    }
}

/// Temporary file of a unit being written (left only by a crash before its
/// rename; inventoried by the purge).
pub(crate) fn temporary_name(unit: &str) -> String {
    format!(".{unit}.syncengine-tmp")
}

/// Writes one unit through a temporary file and a rename; refuses to replace
/// a file of the same name that is not ours.
fn write_unit(dir: &Path, name: &str, marker: &str, content: &str) -> Result<(), Failure> {
    let target = dir.join(name);
    match std::fs::read_to_string(&target) {
        Ok(existing) if existing == content => return Ok(()),
        Ok(existing) if !existing.contains(marker) => {
            return Err(Failure::new(
                "conflict",
                format!(
                    "{} existe et n’appartient pas à SyncEngine : rien n’a été remplacé",
                    target.display()
                ),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(Failure::new(
                "unavailable",
                format!("{} : {e}", target.display()),
            ));
        }
    }
    let temporary = dir.join(temporary_name(name));
    let io = |e: std::io::Error| Failure::new("unavailable", format!("{} : {e}", target.display()));
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(io)?;
        file.write_all(content.as_bytes()).map_err(io)?;
        file.sync_all().map_err(io)?;
    }
    std::fs::rename(&temporary, &target).map_err(io)
}

/// Installs (or refreshes) and enables the agent units for `state`.
/// Linux only: the units are rendered by `platform-linux`.
///
/// # Errors
/// `unavailable` without a units folder, an agent executable, or a working
/// `systemctl --user`; `conflict` when a same-named unit is not ours.
#[cfg(target_os = "linux")]
pub(crate) fn install(
    systemctl: &Systemctl,
    dir: &Path,
    agent: &Path,
    state: &Path,
) -> Result<(), Failure> {
    use platform_api::ScheduleAdapter;
    let invocation = platform_api::AgentInvocation {
        executable: agent.to_path_buf(),
        state: state.to_path_buf(),
    };
    let artifacts = platform_linux::LinuxScheduler
        .render(&invocation)
        .map_err(|e| Failure::new("invalid_input", e.to_string()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| Failure::new("unavailable", format!("{} : {e}", dir.display())))?;
    for artifact in &artifacts {
        let marker = UNITS
            .iter()
            .find(|(name, _)| *name == artifact.name)
            .map(|(_, marker)| *marker)
            .ok_or_else(|| Failure::new("invalid_input", "unité inattendue"))?;
        write_unit(dir, artifact.name, marker, &artifact.content)?;
    }
    let unavailable = |cause: String| {
        Failure::new(
            "unavailable",
            format!(
                "systemd utilisateur indisponible, les planifications ne se lanceront pas : {cause}"
            ),
        )
    };
    systemctl
        .run(&["--user", "daemon-reload"])
        .map_err(unavailable)?;
    systemctl
        .run(&["--user", "enable", "--now", TIMER])
        .map_err(unavailable)?;
    systemctl
        .run(&["--user", "enable", SESSION])
        .map_err(unavailable)?;
    Ok(())
}

/// Stops and removes the agent units in the safe order. Returns each failed
/// step with its cause; the files are removed even when `systemctl` fails.
pub(crate) fn uninstall(systemctl: &Systemctl, dir: &Path) -> Vec<(String, String)> {
    let mut failed = Vec::new();
    let present: Vec<&str> = UNITS
        .iter()
        .filter(|(name, marker)| ours(dir, name, marker))
        .map(|(name, _)| *name)
        .collect();
    if present.is_empty() {
        return failed;
    }
    let mut disable = vec!["--user", "disable", "--now"];
    disable.extend(present.iter().copied());
    if let Err(e) = systemctl.run(&disable) {
        failed.push((disable.join(" "), e));
    }
    for name in present {
        let path = dir.join(name);
        if let Err(e) = std::fs::remove_file(&path) {
            failed.push((path.display().to_string(), e.to_string()));
        }
    }
    for link in UNIT_LINKS {
        let path = dir.join(link);
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
            && let Err(e) = std::fs::remove_file(&path)
        {
            failed.push((path.display().to_string(), e.to_string()));
        }
    }
    if let Err(e) = systemctl.run(&["--user", "daemon-reload"]) {
        failed.push(("--user daemon-reload".into(), e));
    }
    failed
}

/// The agent executable shipped next to this interface.
#[cfg(target_os = "linux")]
pub(crate) fn agent_executable() -> Result<PathBuf, Failure> {
    let exe = std::env::current_exe()
        .map_err(|e| Failure::new("unavailable", e.to_string()))?
        .with_file_name("syncengine-agent");
    exe.canonicalize().map_err(|e| {
        Failure::new(
            "unavailable",
            format!("agent introuvable ({}) : {e}", exe.display()),
        )
    })
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;
    type Failure = Box<dyn std::error::Error>;

    /// A fake `systemctl` that logs each call with whether the timer unit
    /// file still exists at that moment, and answers `is-active`.
    pub(crate) fn fake_systemctl(
        bin: &Path,
        log: &Path,
        units: &Path,
        active: &str,
    ) -> Result<Systemctl, Failure> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(bin)?;
        let script = bin.join("systemctl");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ -e \"$UNITS/{TIMER}\" ]; then f=present; else f=absent; fi\necho \"$* [$f]\" >> \"$LOG\"\ncase \"$*\" in *is-active*) echo {active}; exit 3;; esac\nexit 0\n"
            ),
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        Ok(Systemctl::with_path(bin, &[("LOG", log), ("UNITS", units)]))
    }

    pub(crate) fn calls(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn install_writes_our_units_then_enables_the_timer() -> Result<(), Failure> {
        let _spawn = crate::test_support::spawn_guard();
        let temp = tempfile::tempdir()?;
        let units = temp.path().join("config/systemd/user");
        let log = temp.path().join("calls.log");
        let systemctl = fake_systemctl(&temp.path().join("bin"), &log, &units, "active")?;
        assert_eq!(state(&systemctl, &units), AgentState::NotInstalled);
        let agent = temp.path().join("syncengine-agent");
        let state_dir = temp.path().join("état");
        install(&systemctl, &units, &agent, &state_dir).map_err(|e| e.message)?;
        for (name, marker) in UNITS {
            assert!(ours(&units, name, marker), "{name}");
        }
        let service = std::fs::read_to_string(units.join("syncengine-clock.service"))?;
        assert!(service.contains("tick"));
        assert_eq!(
            calls(&log),
            [
                "--user daemon-reload [present]",
                "--user enable --now syncengine-clock.timer [present]",
                "--user enable syncengine-session.service [present]",
            ]
        );
        assert_eq!(state(&systemctl, &units), AgentState::Active);
        // Idempotent; a foreign unit of the same name is never replaced.
        install(&systemctl, &units, &agent, &state_dir).map_err(|e| e.message)?;
        std::fs::write(units.join("syncengine-clock.timer"), "[Timer]\n")?;
        assert!(install(&systemctl, &units, &agent, &state_dir).is_err());
        assert_eq!(
            std::fs::read_to_string(units.join("syncengine-clock.timer"))?,
            "[Timer]\n"
        );
        Ok(())
    }

    #[test]
    fn observed_state_distinguishes_inactive_and_unavailable() -> Result<(), Failure> {
        let _spawn = crate::test_support::spawn_guard();
        let temp = tempfile::tempdir()?;
        let units = temp.path().join("units");
        let log = temp.path().join("calls.log");
        let agent = temp.path().join("syncengine-agent");
        let inactive = fake_systemctl(&temp.path().join("bin1"), &log, &units, "inactive")?;
        install(&inactive, &units, &agent, temp.path()).map_err(|e| e.message)?;
        assert_eq!(state(&inactive, &units), AgentState::Inactive);
        let missing = Systemctl::with_path(&temp.path().join("vide"), &[]);
        assert_eq!(state(&missing, &units), AgentState::Unavailable);
        // Without systemctl, installing fails loudly instead of claiming success.
        let error = install(&missing, &units, &agent, temp.path())
            .err()
            .ok_or("install claimed success")?;
        assert_eq!(error.code, "unavailable");
        Ok(())
    }

    #[test]
    fn uninstall_disables_before_removing_then_reloads() -> Result<(), Failure> {
        let _spawn = crate::test_support::spawn_guard();
        // M5 — Étant donné des unités installées et actives, quand on les
        // retire, alors `disable --now` voit encore les fichiers, puis les
        // fichiers et liens disparaissent, puis `daemon-reload`.
        let temp = tempfile::tempdir()?;
        let units = temp.path().join("units");
        let log = temp.path().join("calls.log");
        let systemctl = fake_systemctl(&temp.path().join("bin"), &log, &units, "active")?;
        install(&systemctl, &units, &temp.path().join("agent"), temp.path())
            .map_err(|e| e.message)?;
        std::fs::create_dir_all(units.join("timers.target.wants"))?;
        std::os::unix::fs::symlink(
            units.join(TIMER),
            units.join("timers.target.wants").join(TIMER),
        )?;
        std::fs::remove_file(&log)?;
        assert!(uninstall(&systemctl, &units).is_empty());
        assert_eq!(
            calls(&log),
            [
                "--user disable --now syncengine-clock.service syncengine-session.service syncengine-clock.timer [present]",
                "--user daemon-reload [absent]",
            ]
        );
        assert!(std::fs::read_dir(&units)?.all(|e| e.is_ok_and(|e| e.path().is_dir())));
        assert!(!units.join("timers.target.wants").join(TIMER).exists());
        Ok(())
    }
}
