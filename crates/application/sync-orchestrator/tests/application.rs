//! Real local facade boundaries; no personal files or cloud accounts.
use std::fs;
use sync_core::ProfileId;
use sync_filter::FilterRules;
use sync_orchestrator::Application;
use sync_plan::SyncMode;
use sync_profiles::Profile;

fn fixture(
    mode: SyncMode,
) -> Result<(tempfile::TempDir, Application, Profile), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    for directory in ["source", "destination", "state"] {
        fs::create_dir(temp.path().join(directory))?;
    }
    let app = Application::open(temp.path().join("state"))?;
    let profile = Profile {
        id: ProfileId::new("p")?,
        name: "Photos été".into(),
        group: "Tests".into(),
        mode,
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        filter: FilterRules::new().exclude_file("*.tmp"),
        allow_delete: false,
    };
    app.profile_save(profile.clone())?;
    Ok((temp, app, profile))
}

#[test]
fn preview_is_read_only_exact_plan_consumed_once_and_history_survives()
-> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, p) = fixture(SyncMode::Backup)?;
    fs::write(p.source.join("été.txt"), b"data")?;
    fs::write(p.source.join("secret.tmp"), b"excluded")?;
    let preview = app.preview(p.id.clone())?;
    assert!(!p.destination.join("été.txt").exists());
    let id = preview["plan"]["id"]
        .as_str()
        .ok_or("plan id absent")?
        .to_owned();
    assert!(app.execute(p.id.clone(), id.clone(), false, false).is_err());
    let report = app.execute(p.id.clone(), id.clone(), true, false)?;
    assert_eq!(report["status"], "succeeded");
    assert_eq!(fs::read(p.destination.join("été.txt"))?, b"data");
    assert!(!p.destination.join("secret.tmp").exists());
    assert!(app.execute(p.id.clone(), id, true, false).is_err());
    let rows = app.history(p.id.clone())?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].plan, preview["plan"]);
    assert!(app.snapshot()?.bytes_done >= 4);
    drop(app);
    let app = Application::open(temp.path().join("state"))?;
    assert_eq!(app.history(p.id)?.len(), 1);
    Ok(())
}

#[test]
fn changed_profile_wrong_plan_and_failed_preview_invalidate_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, mut p) = fixture(SyncMode::Backup)?;
    fs::write(p.source.join("file"), b"data")?;
    let first = app.preview(p.id.clone())?;
    let first_id = first["plan"]["id"].as_str().ok_or("id")?.to_owned();
    assert!(
        app.execute(p.id.clone(), "forged".into(), true, false)
            .is_err()
    );
    assert!(
        app.execute(p.id.clone(), first_id.clone(), true, false)
            .is_err()
    );
    app.preview(p.id.clone())?;
    p.name = "Edited".into();
    app.profile_save(p.clone())?;
    assert!(app.execute(p.id.clone(), first_id, true, false).is_err());
    app.preview(p.id.clone())?;
    fs::remove_dir_all(&p.source)?;
    assert!(app.preview(p.id.clone()).is_err());
    assert!(app.execute(p.id, "anything".into(), true, false).is_err());
    assert!(fs::read_dir(&p.destination)?.next().is_none());
    Ok(())
}

#[test]
fn nested_state_and_second_desktop_are_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, mut p) = fixture(SyncMode::Backup)?;
    assert!(Application::open(temp.path().join("state")).is_err());
    p.destination = p.source.join("nested");
    fs::create_dir(&p.destination)?;
    assert!(app.profile_save(p.clone()).is_err());
    p.destination = temp.path().join("state");
    assert!(app.profile_save(p).is_err());
    Ok(())
}

#[test]
fn missing_mirror_source_means_zero_deletions() -> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.source.join("keep"), b"keep")?;
    fs::write(p.destination.join("keep"), b"keep")?;
    fs::write(p.destination.join("old"), b"do not lose")?;
    let preview = app.preview(p.id.clone())?;
    fs::rename(&p.source, p.source.with_file_name("missing"))?;
    assert!(
        app.execute(
            p.id,
            preview["plan"]["id"].as_str().ok_or("id")?.into(),
            true,
            true
        )
        .is_err()
    );
    assert_eq!(fs::read(p.destination.join("old"))?, b"do not lose");
    Ok(())
}

#[test]
fn deletion_requires_confirmation_and_preserves_version() -> Result<(), Box<dyn std::error::Error>>
{
    let (temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.source.join("keep"), b"keep")?;
    fs::write(p.destination.join("keep"), b"keep")?;
    fs::write(p.destination.join("old"), b"old")?;
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["delete"], 1);
    assert!(
        app.execute(
            p.id.clone(),
            preview["plan"]["id"].as_str().ok_or("id")?.into(),
            true,
            false
        )
        .is_err()
    );
    assert!(p.destination.join("old").exists());
    let preview = app.preview(p.id.clone())?;
    let report = app.execute(
        p.id.clone(),
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        true,
    )?;
    assert_eq!(report["status"], "succeeded");
    assert!(!p.destination.join("old").exists());
    assert!(temp.path().join("state/versions").is_dir());
    Ok(())
}

#[test]
fn bidirectional_uses_persistent_reference_and_reports_conflicts()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, p) = fixture(SyncMode::Bidirectional)?;
    fs::write(p.source.join("left"), b"left")?;
    fs::write(p.destination.join("right"), b"right")?;
    let preview = app.preview(p.id.clone())?;
    let report = app.execute(
        p.id.clone(),
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        false,
    )?;
    assert_eq!(report["baseline_updated"], true);
    fs::write(p.source.join("left"), b"aaa")?;
    fs::write(p.destination.join("left"), b"bbb")?;
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["conflict"], 1);
    let result = app.execute(
        p.id,
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        false,
    )?;
    assert_eq!(result["baseline_updated"], false);
    assert_eq!(fs::read(p.source.join("left"))?, b"aaa");
    assert_eq!(fs::read(p.destination.join("left"))?, b"bbb");
    Ok(())
}

#[test]
fn cancellation_and_busy_commands_keep_destination_intact() -> Result<(), Box<dyn std::error::Error>>
{
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let (_temp, app, p) = fixture(SyncMode::Backup)?;
    // Sparse file keeps fixture allocation bounded while hashing/transfer give
    // the observer a real cancellable operation. No sleeps inside the engine.
    fs::File::create(p.source.join("large"))?.set_len(64 * 1024 * 1024)?;
    let app = Arc::new(app);
    let worker_app = app.clone();
    let id = p.id.clone();
    let worker = std::thread::spawn(move || worker_app.preview(id));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.snapshot()?.active && !worker.is_finished() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(app.snapshot()?.active, "preview must expose active state");
    assert!(app.profile_remove(p.id.clone()).is_err());
    app.cancel()?;
    assert!(
        worker
            .join()
            .map_err(|_| "preview worker panicked")?
            .is_err()
    );
    assert!(!app.snapshot()?.active);
    assert!(!p.destination.join("large").exists());
    // A new preview gets a fresh cancellation token, then real execution can
    // independently be cancelled. No subsequent operation may publish.
    let preview = app.preview(p.id.clone())?;
    let plan = preview["plan"]["id"].as_str().ok_or("id")?.to_owned();
    let worker_app = app.clone();
    let id = p.id.clone();
    let worker = std::thread::spawn(move || worker_app.execute(id, plan, true, false));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.snapshot()?.active && !worker.is_finished() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(app.snapshot()?.active, "execution must expose active state");
    app.cancel()?;
    let result = worker.join().map_err(|_| "execution worker panicked")?;
    if let Ok(report) = result {
        assert_ne!(report["status"], "succeeded");
    }
    assert!(!p.destination.join("large").exists());
    assert!(!app.snapshot()?.active);
    assert_eq!(app.history(p.id)?.len(), 1);
    Ok(())
}

#[test]
fn unfinished_intent_remains_visible_after_restart() -> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, p) = fixture(SyncMode::Backup)?;
    let preview = app.preview(p.id.clone())?;
    let state = temp.path().join("state");
    let db = rusqlite::Connection::open(state.join("desktop-runs.sqlite"))?;
    db.execute(
        "INSERT INTO runs VALUES('interrupted',?1,?2,'running',?3,NULL)",
        rusqlite::params![
            p.id.as_str(),
            r#"{"secs":1,"nanos":0}"#,
            serde_json::to_string(&preview["plan"])?
        ],
    )?;
    drop(db);
    drop(app);
    let app = Application::open(state)?;
    let rows = app.history(p.id)?;
    assert_eq!(rows[0].state, "running");
    assert!(rows[0].report.is_none());
    assert_eq!(rows[0].plan, preview["plan"]);
    Ok(())
}
