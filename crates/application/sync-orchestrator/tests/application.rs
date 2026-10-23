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
    let app =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
    let profile = Profile {
        id: ProfileId::new("p")?,
        name: "Photos été".into(),
        group: "Tests".into(),
        mode,
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        filter: FilterRules::new().exclude_file("*.tmp"),
        remote_source: None,
        remote_destination: None,
        allow_delete: false,
    };
    app.profile_save(profile.clone())?;
    Ok((temp, app, profile))
}

#[test]
fn maintenance_ownership_blocks_preferences_and_scheduler_initialization()
-> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, _) = fixture(SyncMode::Backup)?;
    let preferences = app.preferences()?;
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(temp.path().join("state/desktop.lock"))?;
    owner.try_lock()?;
    assert!(app.preferences_save(preferences.clone()).is_err());
    assert!(app.schedules().is_err());
    assert!(!temp.path().join("state/scheduler.sqlite").exists());
    drop(owner);
    app.preferences_save(preferences)?;
    assert!(app.schedules()?.is_empty());
    Ok(())
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
    let app =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
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
fn nested_state_is_refused_and_idle_desktop_allows_agent() -> Result<(), Box<dyn std::error::Error>>
{
    let (temp, app, mut p) = fixture(SyncMode::Backup)?;
    let other =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
    fs::write(p.source.join("agent-file"), b"agent")?;
    other.execute_scheduled(&p, false, &sync_core::CancellationToken::new())?;
    assert_eq!(fs::read(p.destination.join("agent-file"))?, b"agent");
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
    let app = Application::open_with_lock_root(state, temp.path().join("locks"))?;
    let rows = app.history(p.id)?;
    assert_eq!(rows[0].state, "running");
    assert!(rows[0].report.is_none());
    assert_eq!(rows[0].plan, preview["plan"]);
    Ok(())
}

#[test]
fn scheduled_authorization_checks_profile_deletions_and_report_status()
-> Result<(), Box<dyn std::error::Error>> {
    use sync_core::CancellationToken;
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.source.join("keep"), b"keep")?;
    fs::write(p.destination.join("keep"), b"keep")?;
    fs::write(p.destination.join("old"), b"old")?;
    let cancel = CancellationToken::new();
    // Given an explicitly destructive profile, When unattended deletion approval
    // is missing, Then old content remains and no application intent is begun.
    assert!(app.execute_scheduled(&p, false, &cancel).is_err());
    assert_eq!(fs::read(p.destination.join("old"))?, b"old");
    assert!(app.history(p.id.clone())?.is_empty());
    let mut stale = p.clone();
    stale.name = "stale authorization".into();
    assert_eq!(
        app.execute_scheduled(&stale, true, &cancel)
            .err()
            .ok_or("error")?
            .code,
        "stale"
    );
    let report = app.execute_scheduled(&p, true, &cancel)?;
    assert_eq!(report["status"], "succeeded");
    assert!(!p.destination.join("old").exists());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        app.execute_scheduled(&p, true, &cancelled)
            .err()
            .ok_or("error")?
            .code,
        "cancelled"
    );
    Ok(())
}
#[test]
fn scheduled_conflicts_are_attention_and_do_not_advance_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    use sync_scheduler::{JobStatus, MissedJobs, Schedule, ScheduledApplication, Trigger};
    let (_temp, app, p) = fixture(SyncMode::Bidirectional)?;
    fs::write(p.source.join("conflict"), b"source")?;
    fs::write(p.destination.join("conflict"), b"target")?;
    let schedule = Schedule {
        approved: p.clone(),
        trigger: Trigger::Interval { seconds: 60 },
        created_at: 0,
        enabled: true,
        allow_deletions: false,
        missed: MissedJobs::RunOnce,
    };
    let result = app.run(&schedule, &sync_core::CancellationToken::new());
    assert_eq!(result.status, JobStatus::Attention);
    assert_eq!(fs::read(p.source.join("conflict"))?, b"source");
    assert_eq!(fs::read(p.destination.join("conflict"))?, b"target");
    assert_eq!(
        app.history(p.id)?[0].report.as_ref().ok_or("report")?["baseline_updated"],
        false
    );
    Ok(())
}
#[test]
fn interrupted_scheduled_intent_requires_explicit_diagnosis_before_rescan()
-> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, p) = fixture(SyncMode::Backup)?;
    fs::write(p.source.join("file"), b"data")?;
    let preview = app.preview(p.id.clone())?;
    rusqlite::Connection::open(temp.path().join("state/desktop-runs.sqlite"))?.execute(
        "INSERT INTO runs VALUES('lost',?1,'0','running',?2,NULL)",
        rusqlite::params![p.id.as_str(), preview["plan"].to_string()],
    )?;
    let cancel = sync_core::CancellationToken::new();
    let plan_id = preview["plan"]["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(
        app.execute(p.id.clone(), plan_id, true, false)
            .err()
            .ok_or("manual interruption refusal")?
            .code,
        "interrupted"
    );
    assert_eq!(
        app.execute_scheduled(&p, false, &cancel)
            .err()
            .ok_or("error")?
            .code,
        "interrupted"
    );
    assert!(!p.destination.join("file").exists());
    assert!(app.acknowledge_interrupted(&p.id, false).is_err());
    app.acknowledge_interrupted(&p.id, true)?;
    assert_eq!(app.history(p.id.clone())?[0].state, "error");
    let report = app.execute_scheduled(&p, false, &cancel)?;
    assert_eq!(report["status"], "succeeded");
    assert_eq!(fs::read(p.destination.join("file"))?, b"data");
    Ok(())
}

#[test]
fn gui_schedule_approval_is_bound_to_current_profile_and_idle_window_allows_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    use sync_scheduler::{MissedJobs, Scheduler, Trigger, Wake};
    let (temp, app, mut p) = fixture(SyncMode::Backup)?;
    let trigger = Trigger::Interval { seconds: 60 };
    assert!(
        app.schedule_save(
            p.id.clone(),
            trigger.clone(),
            MissedJobs::RunOnce,
            false,
            false
        )
        .is_err()
    );
    assert!(
        app.schedule_save(
            p.id.clone(),
            trigger.clone(),
            MissedJobs::RunOnce,
            true,
            true
        )
        .is_err()
    );
    app.schedule_save(p.id.clone(), trigger, MissedJobs::RunOnce, true, false)?;
    let states = app.schedules()?;
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].schedule.approved, p);
    p.name = "changed".into();
    app.profile_save(p.clone())?;
    let mut scheduler = Scheduler::open(temp.path().join("state/scheduler.sqlite"))?;
    let agent =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
    let receipts = scheduler.dispatch(
        sync_core::Timestamp::now()?.secs(),
        Wake::Clock,
        &agent,
        &sync_core::CancellationToken::new(),
    )?;
    assert_eq!(receipts[0].completion.code, "stale");
    app.schedule_remove(p.id)?;
    assert!(app.schedules()?.is_empty());
    Ok(())
}

#[test]
fn paged_preview_transports_only_the_inspected_slice_and_executes_the_whole_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, p) = fixture(SyncMode::Backup)?;
    for n in 0..205 {
        fs::write(p.source.join(format!("file-{n:03}")), b"x")?;
    }
    let preview = app.preview_paged(p.id.clone())?;
    assert_eq!(
        preview["plan"]["operations"]
            .as_array()
            .ok_or("operations")?
            .len(),
        100
    );
    let id = preview["plan"]["id"].as_str().ok_or("id")?;
    let page = app.plan_page(id, 200, "", "copy")?;
    assert_eq!(page["total"], 205);
    assert_eq!(page["operations"].as_array().ok_or("page")?.len(), 5);
    assert!(app.plan_page("stale", 0, "", "").is_err());
    app.execute(p.id.clone(), id.into(), true, false)?;
    assert_eq!(fs::read_dir(&p.destination)?.count(), 205);
    let overview = app.history_overview(p.id.clone())?;
    assert!(overview[0].plan.is_null());
    assert_eq!(
        overview[0].report.as_ref().ok_or("summary")?["status"],
        "succeeded"
    );
    let historical = app.history_page(p.id.clone(), &overview[0].execution, 200)?;
    assert_eq!(historical["total"], 205);
    assert_eq!(
        historical["operations"]
            .as_array()
            .ok_or("history page")?
            .len(),
        5
    );
    assert!(!historical["operations"][0]["outcome"].is_null());
    Ok(())
}
