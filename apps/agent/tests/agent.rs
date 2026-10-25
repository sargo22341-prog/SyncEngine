//! Real headless processes, isolated roots, no GUI or personal files.
use std::path::Path;
use std::process::{Command, Output};
use sync_core::ProfileId;
use sync_orchestrator::Application;
use sync_profiles::Profile;
use sync_scheduler::{MissedJobs, Schedule, Scheduler, Trigger};
type Error = Box<dyn std::error::Error>;
/// Runs `action` on the facade of `root`, retrying briefly while `busy`.
/// Tests share one process: a child spawned by another test inherits the open
/// `desktop.lock` descriptor (and its `flock`) until its `exec`, which can
/// refuse this in-process command for a few milliseconds. Bounded to 5 s.
fn with_app<T>(
    root: &Path,
    action: impl Fn(&Application) -> Result<T, sync_orchestrator::AppError>,
) -> Result<T, Error> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let result =
            Application::open_with_lock_root(root.join("state"), root.join("SyncEngine/job-locks"))
                .and_then(|app| action(&app));
        match result {
            Err(error) if error.code == "busy" && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            other => return Ok(other?),
        }
    }
}
fn fixture() -> Result<(tempfile::TempDir, Profile), Error> {
    let temp = tempfile::tempdir()?;
    for name in ["state", "source", "destination"] {
        std::fs::create_dir(temp.path().join(name))?;
    }
    let p = Profile {
        id: ProfileId::new("p")?,
        name: "été backup".into(),
        group: String::new(),
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        mode: serde_json::from_str("\"backup\"")?,
        filter: Default::default(),
        remote_source: None,
        remote_destination: None,
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
    };
    with_app(temp.path(), |app| app.profile_save(p.clone()))?;
    Ok((temp, p))
}
fn command(root: &Path, action: &str, tail: &[&str]) -> Result<Output, Error> {
    Ok(Command::new(env!("CARGO_BIN_EXE_syncengine-agent"))
        .arg(action)
        .arg(root.join("state"))
        .args(tail)
        .env("LOCALAPPDATA", root)
        .env("HOME", root)
        .env("XDG_STATE_HOME", root)
        .output()?)
}
fn schedule(root: &Path, p: Profile, allow_deletions: bool) -> Result<(), Error> {
    Scheduler::open(root.join("state/scheduler.sqlite"))?.save(&Schedule {
        approved: p,
        trigger: Trigger::Interval { seconds: 60 },
        created_at: 0,
        enabled: true,
        allow_deletions,
        missed: MissedJobs::RunOnce,
        show_window: false,
    })?;
    Ok(())
}
#[test]
fn approved_profile_runs_in_headless_process_and_is_not_replayed() -> Result<(), Error> {
    let (temp, p) = fixture()?;
    std::fs::write(p.source.join("été"), b"safe")?;
    assert!(
        !command(temp.path(), "authorize", &["p", "interval", "60"])?
            .status
            .success()
    );
    let approved = command(
        temp.path(),
        "authorize",
        &["p", "interval", "60", "--approve-writes"],
    )?;
    assert!(
        approved.status.success(),
        "{}",
        String::from_utf8_lossy(&approved.stderr)
    );
    let run = command(temp.path(), "tick", &["clock"])?;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(std::fs::read(p.destination.join("été"))?, b"safe");
    let result: serde_json::Value = serde_json::from_slice(&run.stdout)?;
    assert_eq!(result[0]["status"], "succeeded");
    let replay = command(temp.path(), "tick", &["clock"])?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&replay.stdout)?,
        serde_json::json!([])
    );
    let history = with_app(temp.path(), |app| app.history(p.id.clone()))?;
    assert_eq!(history.len(), 1);
    // The finished job stays readable by a window; the idle replay tick did
    // not overwrite it.
    let live = with_app(temp.path(), Application::agent_live)?.ok_or("live snapshot")?;
    assert!(!live.snapshot.active);
    assert_eq!(live.snapshot.profile, Some(p.id.clone()));
    assert_eq!(live.snapshot.planned_bytes, 4);
    Ok(())
}
#[test]
fn changed_profile_or_missing_mirror_source_never_writes_or_deletes() -> Result<(), Error> {
    let (temp, mut p) = fixture()?;
    schedule(temp.path(), p.clone(), false)?;
    std::fs::write(p.source.join("new"), b"source")?;
    p.name = "edited".into();
    with_app(temp.path(), |app| app.profile_save(p.clone()))?;
    let run = command(temp.path(), "tick", &["clock"])?;
    assert_eq!(run.status.code(), Some(2));
    assert!(!p.destination.join("new").exists());
    p.mode = serde_json::from_str("\"mirror\"")?;
    p.allow_delete = true;
    with_app(temp.path(), |app| app.profile_save(p.clone()))?;
    // Separate fresh schedule to avoid intentionally retained completed watermark.
    let db = Scheduler::open(temp.path().join("state/scheduler.sqlite"))?;
    db.remove(&p.id)?;
    drop(db);
    schedule(temp.path(), p.clone(), true)?;
    std::fs::write(p.destination.join("keep"), b"destination")?;
    std::fs::rename(&p.source, temp.path().join("unavailable"))?;
    assert_eq!(
        command(temp.path(), "tick", &["clock"])?.status.code(),
        Some(2)
    );
    assert_eq!(std::fs::read(p.destination.join("keep"))?, b"destination");
    Ok(())
}
#[test]
fn global_os_lock_defers_across_state_directories_then_retries() -> Result<(), Error> {
    let (temp, p) = fixture()?;
    schedule(temp.path(), p.clone(), false)?;
    std::fs::write(p.source.join("file"), b"data")?;
    // Held by this process, observed by the separate headless process.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(temp.path().join("SyncEngine/job-locks/execution.lock"))?;
    file.try_lock()?;
    let busy = command(temp.path(), "tick", &["clock"])?;
    assert_eq!(busy.status.code(), Some(2));
    assert!(!p.destination.join("file").exists());
    assert!(
        Scheduler::open(temp.path().join("state/scheduler.sqlite"))?.list()?[0]
            .last_slot
            .is_none()
    );
    drop(file);
    assert!(command(temp.path(), "tick", &["clock"])?.status.success());
    assert_eq!(std::fs::read(p.destination.join("file"))?, b"data");
    Ok(())
}
#[test]
fn cancellation_marker_blocks_jobs_until_explicit_clear() -> Result<(), Error> {
    let (temp, p) = fixture()?;
    schedule(temp.path(), p.clone(), false)?;
    std::fs::write(p.source.join("file"), b"data")?;
    assert!(command(temp.path(), "cancel", &[])?.status.success());
    assert_eq!(
        command(temp.path(), "tick", &["clock"])?.status.code(),
        Some(2)
    );
    assert!(!p.destination.join("file").exists());
    assert!(command(temp.path(), "clear-cancel", &[])?.status.success());
    assert!(command(temp.path(), "tick", &["clock"])?.status.success());
    Ok(())
}
#[test]
fn export_is_inspectable_and_never_overwrites_existing_artifacts() -> Result<(), Error> {
    let (temp, _) = fixture()?;
    let out = temp.path().join("native");
    let out = out.to_str().ok_or("path")?;
    let result = command(
        temp.path(),
        "export",
        &[out, if cfg!(windows) { "windows" } else { "linux" }],
    )?;
    assert!(result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(value["installed"], false);
    assert!(
        !command(
            temp.path(),
            "export",
            &[out, if cfg!(windows) { "windows" } else { "linux" }]
        )?
        .status
        .success()
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_executable_uses_gui_subsystem_without_console_window() -> Result<(), Error> {
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_syncengine-agent"))?;
    let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into()?) as usize;
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    let subsystem = u16::from_le_bytes(bytes[pe + 24 + 68..pe + 24 + 70].try_into()?);
    assert_eq!(subsystem, 2); // IMAGE_SUBSYSTEM_WINDOWS_GUI
    Ok(())
}

struct Resident(std::process::Child);

#[test]
fn failed_resident_preserves_startup_cause_without_a_console() -> Result<(), Error> {
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    std::fs::create_dir(&state)?;
    std::fs::write(
        state.join("scheduler.sqlite"),
        b"invalid database for isolated test",
    )?;
    let output = command(temp.path(), "resident", &[])?;
    assert!(!output.status.success());
    let status = sync_orchestrator::resident_status(&state)?;
    assert!(!status.available);
    assert_eq!(status.phase, sync_orchestrator::ResidentPhase::Error);
    assert!(
        status
            .diagnostic
            .ok_or("missing startup diagnostic")?
            .contains("scheduler_open")
    );
    assert!(String::from_utf8(output.stderr)?.contains("scheduler_open"));
    assert_eq!(
        std::fs::read(state.join("scheduler.sqlite"))?,
        b"invalid database for isolated test"
    );
    Ok(())
}
impl Drop for Resident {
    fn drop(&mut self) {
        let _already_exited = self.0.kill().is_err();
        let _reap_failed = self.0.wait().is_err();
    }
}
fn start_resident(root: &Path) -> Result<Resident, Error> {
    Ok(Resident(
        Command::new(env!("CARGO_BIN_EXE_syncengine-agent"))
            .arg("resident")
            .arg(root.join("state"))
            .env("LOCALAPPDATA", root)
            .env("HOME", root)
            .env("XDG_STATE_HOME", root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?,
    ))
}

#[test]
fn resident_waits_for_a_foreground_command_and_can_stop_while_waiting() -> Result<(), Error> {
    let (temp, profile) = fixture()?;
    let state = temp.path().join("state");
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.join("desktop.lock"))?;
    owner.try_lock()?;
    let mut resident = start_resident(temp.path())?;
    wait_until(|| Ok(sync_orchestrator::resident_status(&state)?.available))?;
    // Longer than the native session query in ordinary operation. The command
    // remains locked throughout; neither profiles nor destinations may change.
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(resident.0.try_wait()?.is_none());
    assert_eq!(
        sync_orchestrator::resident_status(&state)?.phase,
        sync_orchestrator::ResidentPhase::Starting
    );
    assert!(std::fs::read_dir(&profile.destination)?.next().is_none());
    drop(owner);
    wait_until(|| {
        Ok(sync_orchestrator::resident_status(&state)?.phase
            == sync_orchestrator::ResidentPhase::Idle)
    })?;
    assert!(command(temp.path(), "stop", &[])?.status.success());
    wait_until(|| Ok(resident.0.try_wait()?.is_some()))?;
    assert!(resident.0.wait()?.success());
    std::fs::remove_file(state.join("agent-stop.request"))?;
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(state.join("desktop.lock"))?;
    owner.try_lock()?;
    let mut waiting = start_resident(temp.path())?;
    wait_until(|| Ok(sync_orchestrator::resident_status(&state)?.available))?;
    assert!(command(temp.path(), "stop", &[])?.status.success());
    wait_until(|| Ok(waiting.0.try_wait()?.is_some()))?;
    assert!(waiting.0.wait()?.success());
    assert!(std::fs::read_dir(&profile.destination)?.next().is_none());
    Ok(())
}
fn wait_until(mut condition: impl FnMut() -> Result<bool, Error>) -> Result<(), Error> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    loop {
        if condition()? {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("resident observation timed out".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
#[test]
fn resident_single_instance_live_edits_pause_stop_and_stale_presence() -> Result<(), Error> {
    let (temp, p) = fixture()?;
    let state = temp.path().join("state");
    let mut resident = start_resident(temp.path())?;
    wait_until(|| {
        Ok(sync_orchestrator::resident_status(&state)?.phase
            == sync_orchestrator::ResidentPhase::Idle)
    })?;
    let duplicate = command(temp.path(), "resident", &[])?;
    assert!(!duplicate.status.success());
    // The historical periodic dispatcher cannot race the resident either.
    assert!(!command(temp.path(), "tick", &["clock"])?.status.success());
    assert!(command(temp.path(), "cancel", &[])?.status.success());
    wait_until(|| {
        Ok(sync_orchestrator::resident_status(&state)?.phase
            == sync_orchestrator::ResidentPhase::Paused)
    })?;
    std::fs::write(p.source.join("resident-file"), b"autonomous")?;
    // Authorize through another process while the resident is already running.
    assert!(
        command(
            temp.path(),
            "authorize",
            &["p", "interval", "60", "--approve-writes"]
        )?
        .status
        .success()
    );
    assert!(!p.destination.join("resident-file").exists());
    assert!(command(temp.path(), "clear-cancel", &[])?.status.success());
    wait_until(|| Ok(p.destination.join("resident-file").exists()))?;
    wait_until(|| {
        Ok(Scheduler::open(state.join("scheduler.sqlite"))?.list()?[0]
            .running_slot
            .is_none())
    })?;
    assert!(command(temp.path(), "remove", &["p"])?.status.success());
    assert!(
        Scheduler::open(state.join("scheduler.sqlite"))?
            .list()?
            .is_empty()
    );
    assert!(command(temp.path(), "stop", &[])?.status.success());
    wait_until(|| Ok(resident.0.try_wait()?.is_some()))?;
    assert!(!sync_orchestrator::resident_status(&state)?.available);
    assert_eq!(
        sync_orchestrator::resident_status(&state)?.phase,
        sync_orchestrator::ResidentPhase::Stopped
    );
    // A pending explicit stop also prevents an automatic login restart.
    let mut stopped = start_resident(temp.path())?;
    wait_until(|| Ok(stopped.0.try_wait()?.is_some()))?;
    std::fs::remove_file(state.join("agent-stop.request"))?;
    let mut restarted = start_resident(temp.path())?;
    wait_until(|| {
        Ok(sync_orchestrator::resident_status(&state)?.phase
            == sync_orchestrator::ResidentPhase::Idle
            && sync_orchestrator::resident_status(&state)?.available)
    })?;
    restarted.0.kill()?;
    restarted.0.wait()?;
    let stale = sync_orchestrator::resident_status(&state)?;
    assert!(!stale.available);
    assert_eq!(stale.phase, sync_orchestrator::ResidentPhase::Idle); // a stale observation is never live presence
    Ok(())
}
