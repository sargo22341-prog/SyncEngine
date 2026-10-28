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
        compare: sync_profiles::CompareMethod::Content,
        trash: sync_profiles::TrashPolicy::legacy(),
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
    assert!(app.snapshot()?.entries_scanned >= 1);
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
    // History keeps the preview's identity and its real changes only.
    assert_eq!(rows[0].plan["id"], preview["plan"]["id"]);
    assert_eq!(rows[0].plan["operations"], preview["plan"]["operations"]);
    assert_eq!(rows[0].plan["unchanged_omitted"], 0);
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
    let execution = report["execution"].as_str().ok_or("execution")?;
    let kept = temp
        .path()
        .join("state/Corbeille/p")
        .join(execution)
        .join("old");
    assert_eq!(fs::read(kept)?, b"old");
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
fn paused_scheduled_analysis_keeps_ownership_and_can_resume_or_cancel()
-> Result<(), Box<dyn std::error::Error>> {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    for cancel in [false, true] {
        let (_temp, app, profile) = fixture(SyncMode::Backup)?;
        fs::write(profile.source.join("file"), b"content")?;
        let token = sync_core::CancellationToken::new();
        token.set_paused(true);
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
            let worker = scope.spawn(|| {
                let result = app.execute_scheduled(&profile, false, &token);
                done_tx.send(result).unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !app.snapshot()?.active && !worker.is_finished() && Instant::now() < deadline {
                std::thread::yield_now();
            }
            let snapshot = app.snapshot()?;
            let blocked = done_rx.recv_timeout(Duration::from_millis(40)).is_err();
            let busy = app.profile_remove(profile.id.clone()).is_err();
            let untouched = !profile.destination.join("file").exists();
            if cancel {
                app.cancel()?;
            } else {
                app.set_paused(false)?;
            }
            let result = done_rx.recv_timeout(Duration::from_secs(5))?;
            worker.join().map_err(|_| "scheduled worker panicked")?;
            assert!(snapshot.active && snapshot.pause_requested);
            assert!(blocked && busy && untouched);
            assert!(!app.snapshot()?.active);
            if cancel {
                assert!(result.is_err());
                assert!(!profile.destination.join("file").exists());
            } else {
                assert_eq!(result?["status"], "succeeded");
                assert_eq!(fs::read(profile.destination.join("file"))?, b"content");
            }
            Ok(())
        })?;
    }
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
        show_window: false,
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
fn explicit_diagnosis_clears_stale_schedule_without_replay_and_preserves_files()
-> Result<(), Box<dyn std::error::Error>> {
    use sync_scheduler::{
        Completion, JobStatus, MissedJobs, Schedule, ScheduledApplication, Scheduler, Trigger, Wake,
    };
    struct Counter(std::cell::Cell<usize>);
    impl ScheduledApplication for Counter {
        fn run(&self, _: &Schedule, _: &sync_core::CancellationToken) -> Completion {
            self.0.set(self.0.get() + 1);
            Completion {
                status: JobStatus::Succeeded,
                code: "test".into(),
            }
        }
    }
    let (temp, app, p) = fixture(SyncMode::Mirror)?;
    fs::write(p.source.join("file"), b"source")?;
    fs::write(p.destination.join("keep"), b"destination")?;
    let scheduler_path = temp.path().join("state/scheduler.sqlite");
    Scheduler::open(&scheduler_path)?.save(&Schedule {
        approved: p.clone(),
        trigger: Trigger::Interval { seconds: 60 },
        created_at: 0,
        enabled: true,
        allow_deletions: false,
        missed: MissedJobs::RunOnce,
        show_window: false,
    })?;
    rusqlite::Connection::open(&scheduler_path)?.execute(
        "UPDATE schedules SET running_slot=60 WHERE profile=?1",
        [p.id.as_str()],
    )?;
    assert_eq!(
        app.schedule_remove(p.id.clone())
            .err()
            .ok_or("missing refusal")?
            .code,
        "interrupted"
    );
    assert_eq!(
        app.schedule_save(
            p.id.clone(),
            Trigger::Interval { seconds: 60 },
            MissedJobs::RunOnce,
            true,
            false,
            false
        )
        .err()
        .ok_or("missing save refusal")?
        .code,
        "interrupted"
    );
    assert!(app.acknowledge_interrupted(&p.id, false).is_err());
    let owner = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(temp.path().join("state/desktop.lock"))?;
    owner.try_lock()?;
    assert_eq!(
        app.acknowledge_interrupted(&p.id, true)
            .err()
            .ok_or("missing busy refusal")?
            .code,
        "busy"
    );
    assert_eq!(
        Scheduler::open(&scheduler_path)?.list()?[0].running_slot,
        Some(60)
    );
    drop(owner);
    let execution = sync_executor::execution_lock(&temp.path().join("locks"))?;
    assert!(app.acknowledge_interrupted(&p.id, true).is_err());
    drop(execution);
    app.acknowledge_interrupted(&p.id, true)?;
    app.acknowledge_interrupted(&p.id, true)?; // Retrying a partial diagnosis is safe.
    let mut scheduler = Scheduler::open(&scheduler_path)?;
    let row = scheduler.list()?.remove(0);
    assert_eq!(row.running_slot, None);
    assert_eq!(row.last_slot, Some(60));
    assert_eq!(
        row.result.ok_or("missing receipt")?.status,
        JobStatus::Failed
    );
    let counter = Counter(std::cell::Cell::new(0));
    scheduler.dispatch(
        60,
        Wake::Clock,
        &counter,
        &sync_core::CancellationToken::new(),
    )?;
    assert_eq!(counter.0.get(), 0);
    assert_eq!(fs::read(p.destination.join("keep"))?, b"destination");
    assert!(!p.destination.join("file").exists());
    app.schedule_remove(p.id.clone())?;
    app.profile_remove(p.id)?;
    assert!(app.profiles_list()?.is_empty());
    assert!(scheduler.list()?.is_empty());
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
            true,
            false
        )
        .is_err()
    );
    app.schedule_save(
        p.id.clone(),
        trigger,
        MissedJobs::RunOnce,
        true,
        false,
        true,
    )?;
    let states = app.schedules()?;
    assert_eq!(states.len(), 1);
    assert!(states[0].schedule.show_window);
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

#[test]
fn mirror_replicates_removal_after_initial_two_file_sync() -> Result<(), Box<dyn std::error::Error>>
{
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.source.join("keep.txt"), b"keep")?;
    fs::write(p.source.join("remove.txt"), b"preserve this version")?;
    let first = app.preview(p.id.clone())?;
    app.execute(
        p.id.clone(),
        first["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        false,
    )?;
    assert_eq!(
        fs::read(p.destination.join("remove.txt"))?,
        b"preserve this version"
    );
    // Given two files copied by a real first execution, When one source file
    // is removed, Then repeated previews remain read-only and plan one deletion.
    fs::remove_file(p.source.join("remove.txt"))?;
    for _ in 0..2 {
        let preview = app.preview(p.id.clone())?;
        assert_eq!(preview["summary"]["delete"], 1);
        assert_eq!(preview["summary"]["deletions_refused"], 0);
        assert!(p.destination.join("remove.txt").exists());
    }
    let preview = app.preview(p.id.clone())?;
    let report = app.execute(
        p.id.clone(),
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        true,
    )?;
    assert_eq!(report["status"], "succeeded");
    assert!(!p.destination.join("remove.txt").exists());
    assert_eq!(fs::read(p.destination.join("keep.txt"))?, b"keep");
    assert_eq!(app.preview(p.id)?["summary"]["delete"], 0);
    Ok(())
}
#[test]
fn history_records_only_real_changes_and_counts_unchanged_entries()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, p) = fixture(SyncMode::Mirror)?;
    for name in ["same-1", "same-2"] {
        fs::write(p.source.join(name), b"same")?;
        fs::write(p.destination.join(name), b"same")?;
    }
    fs::write(p.source.join("new"), b"new")?;
    // Given two identical files and one new file, When the mirror runs,
    // Then history stores only the copy, its outcome and the unchanged count.
    let preview = app.preview(p.id.clone())?;
    app.execute(
        p.id.clone(),
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        false,
    )?;
    let run = &app.history(p.id.clone())?[0];
    let operations = run.plan["operations"].as_array().ok_or("operations")?;
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["path"], "new");
    assert_eq!(run.plan["unchanged_omitted"], 2);
    let outcomes = &run.report.as_ref().ok_or("report")?["outcomes"];
    assert_eq!(outcomes.as_array().ok_or("outcomes")?.len(), 1);
    assert_eq!(outcomes[0]["path"], "new");
    let page = app.history_page(p.id.clone(), &run.execution, 0)?;
    assert_eq!(page["total"], 1);
    assert_eq!(page["operations"][0]["outcome"]["status"]["status"], "done");
    // Given nothing left to change, Then the run is recorded without operations.
    let preview = app.preview(p.id.clone())?;
    app.execute(
        p.id.clone(),
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        false,
    )?;
    let run = &app.history(p.id)?[0];
    assert_eq!(run.plan["operations"], serde_json::json!([]));
    assert_eq!(run.plan["unchanged_omitted"], 3);
    Ok(())
}
#[test]
fn mirror_without_deletion_consent_reports_refusal_instead_of_losing_data()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, p) = fixture(SyncMode::Mirror)?;
    fs::write(p.source.join("keep"), b"keep")?;
    fs::write(p.destination.join("keep"), b"keep")?;
    fs::write(p.destination.join("old"), b"old")?;
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["delete"], 0);
    assert_eq!(preview["summary"]["deletions_refused"], 1);
    app.execute(
        p.id,
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        false,
    )?;
    assert_eq!(fs::read(p.destination.join("old"))?, b"old");
    assert_eq!(
        app.history_overview(ProfileId::new("p")?)?[0]
            .report
            .as_ref()
            .ok_or("summary")?["deletions_refused"],
        1
    );
    Ok(())
}

#[test]
fn mirror_revalidates_a_reappeared_source_before_deleting_destination()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.source.join("keep"), b"keep")?;
    fs::write(p.destination.join("keep"), b"keep")?;
    fs::write(p.destination.join("old"), b"destination must survive")?;
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["delete"], 1);
    // Given a deletion was previewed, When source content reappears, Then the
    // executor refuses deletion and both independent contents remain intact.
    fs::write(p.source.join("old"), b"source reappeared")?;
    let report = app.execute(
        p.id,
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        true,
    )?;
    assert_ne!(report["status"], "succeeded");
    assert_eq!(
        fs::read(p.destination.join("old"))?,
        b"destination must survive"
    );
    assert_eq!(fs::read(p.source.join("old"))?, b"source reappeared");
    Ok(())
}

/// Given a full source and a destination holding only one obsolete file (the
/// reported case: 42 000 photos → 1 old archive), When a mirror allowing
/// deletions is previewed, Then the file is planned for deletion: the relative
/// ceiling does not apply below its volume threshold.
#[test]
fn mirror_deletes_a_lone_obsolete_destination_file_by_default()
-> Result<(), Box<dyn std::error::Error>> {
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    for n in 0..20 {
        fs::write(p.source.join(format!("photo{n}.jpg")), b"photo")?;
    }
    fs::write(p.destination.join("old.zip"), b"old")?;
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["delete"], 1);
    assert_eq!(preview["summary"]["deletions_refused"], 0);
    assert!(preview["summary"]["deletion_refusal"].is_null());
    assert_eq!(preview["summary"]["ceiling_lifted"], false);
    let report = app.execute(
        p.id.clone(),
        preview["plan"]["id"].as_str().ok_or("id")?.into(),
        true,
        true,
    )?;
    assert_eq!(report["status"], "succeeded");
    assert!(!p.destination.join("old.zip").exists());
    assert_eq!(fs::read(p.destination.join("photo0.jpg"))?, b"photo");
    Ok(())
}

/// Given a mirror whose deletions exceed the relative ceiling, When previewed
/// normally, Then every deletion is refused with its exact cause; When the user
/// lifts the ceiling for one preview, Then the deletions are planned, the lifted
/// guard is recorded in the plan, execution still needs deletion confirmation,
/// and the next ordinary preview is guarded again.
#[test]
fn mass_deletion_ceiling_is_lifted_only_for_one_explicit_preview()
-> Result<(), Box<dyn std::error::Error>> {
    use sync_orchestrator::DeletionCeiling;
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.source.join("keep"), b"keep")?;
    fs::write(p.destination.join("keep"), b"keep")?;
    for n in 0..12 {
        fs::write(p.destination.join(format!("old{n:02}")), b"old")?;
    }
    let guarded = app.preview_paged(p.id.clone())?;
    assert_eq!(guarded["summary"]["delete"], 0);
    assert_eq!(guarded["summary"]["deletions_refused"], 12);
    assert_eq!(
        guarded["summary"]["deletion_refusal"],
        serde_json::json!({"cause":"too_large","count":12,"total":13,"max_percent":50})
    );

    let lifted = app.preview_paged_with(p.id.clone(), DeletionCeiling::Lifted)?;
    assert_eq!(lifted["summary"]["delete"], 12);
    assert_eq!(lifted["summary"]["ceiling_lifted"], true);
    assert!(lifted["summary"]["deletion_refusal"].is_null());
    let plan = lifted["plan"]["id"].as_str().ok_or("id")?.to_owned();
    // Lifting the ceiling never replaces the per-execution confirmation.
    assert_eq!(
        app.execute(p.id.clone(), plan, true, false)
            .err()
            .ok_or("unconfirmed deletions accepted")?
            .code,
        "confirmation_required"
    );
    assert!(p.destination.join("old00").exists());

    let lifted = app.preview_paged_with(p.id.clone(), DeletionCeiling::Lifted)?;
    let plan = lifted["plan"]["id"].as_str().ok_or("id")?.to_owned();
    let report = app.execute(p.id.clone(), plan, true, true)?;
    assert_eq!(report["status"], "succeeded");
    assert!(!p.destination.join("old00").exists());
    assert_eq!(fs::read(p.destination.join("keep"))?, b"keep");
    // The recorded plan carries the lifted guard for audit.
    let run = &app.history(p.id.clone())?[0];
    let guard = run
        .plan
        .pointer("/policy/deletion/guard")
        .ok_or_else(|| format!("no guard in {}", run.plan))?;
    assert_eq!(
        guard,
        &serde_json::json!({"max_count":null,"max_percent":null,"allow_empty_source":false})
    );

    for n in 0..12 {
        fs::write(p.destination.join(format!("old{n:02}")), b"old")?;
    }
    let again = app.preview_paged(p.id)?;
    assert_eq!(again["summary"]["deletions_refused"], 12);
    assert_eq!(again["summary"]["ceiling_lifted"], false);
    Ok(())
}

/// Given an empty source, When the ceiling is lifted, Then deletions stay
/// refused: lifting never overrides the empty-source guard.
#[test]
fn lifting_the_ceiling_keeps_the_empty_source_guard() -> Result<(), Box<dyn std::error::Error>> {
    use sync_orchestrator::DeletionCeiling;
    let (_temp, app, mut p) = fixture(SyncMode::Mirror)?;
    p.allow_delete = true;
    app.profile_save(p.clone())?;
    fs::write(p.destination.join("precious"), b"precious")?;
    let lifted = app.preview_paged_with(p.id, DeletionCeiling::Lifted)?;
    assert_eq!(lifted["summary"]["delete"], 0);
    assert_eq!(
        lifted["summary"]["deletion_refusal"],
        serde_json::json!({"cause":"source_empty"})
    );
    assert!(p.destination.join("precious").exists());
    Ok(())
}

/// Given a single large file being copied, When the snapshot is polled during
/// the execution, Then transferred bytes advance block by block before the
/// file is validated (the live throughput no longer jumps from 0 to the whole
/// file size at the end of each file), and both counters agree at the end.
#[test]
fn snapshot_reports_block_level_transfer_progress() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    const SIZE: u64 = 64 * 1024 * 1024;
    let (_temp, app, p) = fixture(SyncMode::Backup)?;
    fs::File::create(p.source.join("large"))?.set_len(SIZE)?;
    let preview = app.preview(p.id.clone())?;
    let plan = preview["plan"]["id"].as_str().ok_or("id")?.to_owned();
    let app = Arc::new(app);
    let worker_app = app.clone();
    let id = p.id.clone();
    let worker = std::thread::spawn(move || worker_app.execute(id, plan, true, false));
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut mid_file = false;
    while !worker.is_finished() && Instant::now() < deadline {
        let s = app.snapshot()?;
        assert!(s.bytes_transferred >= s.bytes_done);
        assert!(s.bytes_transferred <= SIZE);
        if s.bytes_done == 0 && s.bytes_transferred > 0 {
            mid_file = true;
        }
        std::thread::yield_now();
    }
    let report = worker.join().map_err(|_| "execution worker panicked")??;
    assert_eq!(report["status"], "succeeded");
    assert!(
        mid_file,
        "no block-level progress observed before validation"
    );
    let end = app.snapshot()?;
    assert_eq!(end.bytes_done, SIZE);
    assert_eq!(end.bytes_transferred, SIZE);
    Ok(())
}

/// Given an agent instance, when it is idle, then it publishes nothing; when a
/// scheduled job ran, then a window reads the final snapshot with the planned
/// totals, and an old active snapshot is not presented as running.
#[test]
fn agent_live_snapshot_is_published_only_for_a_started_job()
-> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, p) = fixture(SyncMode::Backup)?;
    fs::write(p.source.join("file"), b"data")?;
    assert!(!app.publish_live()?);
    assert!(app.agent_live()?.is_none());
    app.execute_scheduled(&p, false, &sync_core::CancellationToken::new())?;
    assert!(app.publish_live()?);
    let window =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
    let live = window.agent_live()?.ok_or("published snapshot")?;
    assert_eq!(live.format, sync_orchestrator::LIVE_FORMAT);
    assert!(!live.snapshot.active);
    assert_eq!(live.snapshot.profile.as_ref(), Some(&p.id));
    assert_eq!(live.snapshot.planned_operations, 1);
    assert_eq!(live.snapshot.planned_bytes, 4);
    assert!(!live.is_live(live.updated_at, 10), "finished job");
    let mut running = live.clone();
    running.snapshot.active = true;
    assert!(running.is_live(running.updated_at + 10, 10));
    assert!(!running.is_live(running.updated_at + 11, 10), "agent gone");
    fs::write(temp.path().join("state/agent-live.json"), b"{")?;
    assert_eq!(window.agent_live().err().ok_or("malformed")?.code, "format");
    Ok(())
}

#[test]
fn agent_stop_request_and_window_presence_are_explicit() -> Result<(), Box<dyn std::error::Error>> {
    let (temp, app, _) = fixture(SyncMode::Backup)?;
    let state = temp.path().join("state");
    assert!(!app.agent_cancel_pending());
    app.agent_cancel_request()?;
    assert!(app.agent_cancel_pending());
    assert!(sync_orchestrator::agent_cancel_marker(&state).exists());
    app.agent_cancel_clear()?;
    app.agent_cancel_clear()?;
    assert!(!app.agent_cancel_pending());
    assert!(!sync_orchestrator::desktop_present(&state)?);
    let held = sync_orchestrator::hold_desktop_presence(&state)?.ok_or("first window")?;
    assert!(sync_orchestrator::desktop_present(&state)?);
    assert!(sync_orchestrator::hold_desktop_presence(&state)?.is_none());
    drop(held);
    assert!(!sync_orchestrator::desktop_present(&state)?);
    Ok(())
}
