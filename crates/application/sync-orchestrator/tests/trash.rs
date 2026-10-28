//! Per-profile trash and journal lifetime through the real local facade;
//! temporary directories only.
use std::fs;
use std::path::Path;
use sync_core::{CancellationToken, ProfileId};
use sync_filter::FilterRules;
use sync_orchestrator::{Application, Consent, TRASH_FOLDER};
use sync_plan::{Action, SyncMode};
use sync_profiles::{Profile, TrashPolicy};

type Failure = Box<dyn std::error::Error>;

/// Mirror profile with deletions allowed and the given trash; the
/// destination has one file to replace and one to delete.
fn fixture(trash: TrashPolicy) -> Result<(tempfile::TempDir, Application, Profile), Failure> {
    let temp = tempfile::tempdir()?;
    for directory in ["source", "destination", "state"] {
        fs::create_dir(temp.path().join(directory))?;
    }
    let app =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
    let profile = Profile {
        id: ProfileId::new("p")?,
        name: "Corbeille".into(),
        group: String::new(),
        mode: SyncMode::Mirror,
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        filter: FilterRules::new(),
        remote_source: None,
        remote_destination: None,
        allow_delete: true,
        compare: sync_profiles::CompareMethod::Content,
        trash,
    };
    app.profile_save(profile.clone())?;
    fs::write(profile.source.join("changed"), b"new content")?;
    fs::write(profile.destination.join("changed"), b"old content")?;
    fs::write(profile.destination.join("obsolete"), b"removed from source")?;
    Ok((temp, app, profile))
}

fn plan_id(preview: &serde_json::Value) -> Result<String, Failure> {
    Ok(preview["plan"]["id"].as_str().ok_or("plan id")?.to_owned())
}

fn journals(state: &Path) -> Result<Vec<String>, Failure> {
    let mut names = Vec::new();
    for entry in fs::read_dir(state)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name.ends_with(".jsonl") {
            names.push(name);
        }
    }
    Ok(names)
}

fn all_consents() -> Consent {
    Consent {
        writes: true,
        deletions: true,
        permanent_loss: false,
    }
}

#[test]
fn disabled_trash_makes_zero_copies_and_keeps_other_safeguards() -> Result<(), Failure> {
    // Given a profile whose trash is disabled,
    let (temp, app, p) = fixture(TrashPolicy::disabled())?;
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["trash"]["enabled"], false);
    assert_eq!(preview["summary"]["trash"]["overflow"], false);
    // the deletion confirmation is still required,
    assert!(
        app.execute(p.id.clone(), plan_id(&preview)?, true, false)
            .is_err()
    );
    // When the plan is executed with every confirmation,
    let preview = app.preview(p.id.clone())?;
    let report = app.execute_with(p.id.clone(), plan_id(&preview)?, all_consents())?;
    // Then the files are replaced and deleted without any copy.
    assert_eq!(report["status"], "succeeded");
    assert_eq!(fs::read(p.destination.join("changed"))?, b"new content");
    assert!(!p.destination.join("obsolete").exists());
    assert!(!temp.path().join("state").join(TRASH_FOLDER).exists());
    for outcome in report["outcomes"].as_array().ok_or("outcomes")? {
        assert!(outcome["status"]["version"].is_null(), "{outcome}");
    }
    assert!(app.trash_overview(&p.id)?.runs.is_empty());
    Ok(())
}

#[test]
fn enabled_trash_keeps_listable_copies_until_emptied_or_profile_removed() -> Result<(), Failure> {
    let (temp, app, p) = fixture(TrashPolicy::legacy())?;
    let state = temp.path().join("state");
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["trash"]["needed"], 30);
    let report = app.execute_with(p.id.clone(), plan_id(&preview)?, all_consents())?;
    assert_eq!(report["status"], "succeeded");
    let execution = report["execution"].as_str().ok_or("execution")?;
    // Copies are stored under Corbeille/<profile id>/<execution>/.
    let folder = state.join(TRASH_FOLDER).join("p").join(execution);
    assert_eq!(fs::read(folder.join("changed"))?, b"old content");
    assert_eq!(fs::read(folder.join("obsolete"))?, b"removed from source");
    // The journal goes once the final report is committed.
    assert!(journals(&state)?.is_empty());
    let overview = app.trash_overview(&p.id)?;
    assert_eq!(
        (overview.used, overview.limit),
        (30, TrashPolicy::DEFAULT_MAX_BYTES)
    );
    assert_eq!(overview.runs.len(), 1);
    assert_eq!((overview.runs[0].files, overview.runs[0].bytes), (2, 30));
    // The listing comes from the history (status.version).
    let page = app.trash_page(&p.id, execution, 0)?;
    assert_eq!(page.total, 2);
    let mut kinds: Vec<_> = page
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.action, f.bytes))
        .collect();
    kinds.sort_by_key(|k| k.0);
    assert_eq!(
        kinds,
        [
            ("changed", Some(Action::Replace), Some(11)),
            ("obsolete", Some(Action::Delete), Some(19)),
        ]
    );
    // « Enregistrer sous… » copies a kept file without changing the trash.
    let saved = temp.path().join("restored.txt");
    let obsolete = page
        .files
        .iter()
        .find(|f| f.path == "obsolete")
        .ok_or("file")?;
    app.trash_save_as(&p.id, execution, &obsolete.location, &saved)?;
    assert_eq!(fs::read(&saved)?, b"removed from source");
    assert!(
        app.trash_save_as(&p.id, execution, "../../escape", &saved)
            .is_err()
    );
    // Emptying removes the copies only.
    app.trash_empty(&p.id)?;
    assert!(!folder.exists());
    assert!(app.trash_overview(&p.id)?.runs.is_empty());
    assert_eq!(fs::read(p.destination.join("changed"))?, b"new content");
    // A new copy, then removing the profile removes its whole trash.
    fs::write(p.source.join("changed"), b"third")?;
    let preview = app.preview(p.id.clone())?;
    app.execute_with(p.id.clone(), plan_id(&preview)?, all_consents())?;
    assert!(state.join(TRASH_FOLDER).join("p").is_dir());
    app.profile_remove(p.id.clone())?;
    assert!(!state.join(TRASH_FOLDER).join("p").exists());
    assert_eq!(fs::read(p.destination.join("changed"))?, b"third");
    Ok(())
}

#[test]
fn overflowing_trash_warns_needs_manual_consent_and_blocks_scheduled_runs() -> Result<(), Failure> {
    // Given a trash smaller than the files to replace or delete,
    let (temp, app, p) = fixture(TrashPolicy {
        enabled: true,
        max_bytes: 10,
    })?;
    let state = temp.path().join("state");
    // When previewing, Then the overflow is reported with its sizes.
    let preview = app.preview(p.id.clone())?;
    let trash = &preview["summary"]["trash"];
    assert_eq!(
        (trash["needed"].as_u64(), trash["limit"].as_u64()),
        (Some(30), Some(10))
    );
    assert_eq!(trash["overflow"], true);
    // A manual run without the explicit consent is refused, nothing written.
    let refused = app
        .execute_with(p.id.clone(), plan_id(&preview)?, all_consents())
        .err()
        .ok_or("overflow accepted")?;
    assert_eq!(refused.code, "trash_insufficient");
    assert!(refused.message.contains("lancez-la manuellement"));
    assert_eq!(fs::read(p.destination.join("changed"))?, b"old content");
    // A scheduled run is blocked the same way.
    let blocked = app
        .execute_scheduled(&p, true, &CancellationToken::new())
        .err()
        .ok_or("scheduled overflow accepted")?;
    assert_eq!(blocked.code, "trash_insufficient");
    assert!(p.destination.join("obsolete").exists());
    // With the consent, the run proceeds and keeps nothing (all or nothing).
    let preview = app.preview(p.id.clone())?;
    let report = app.execute_with(
        p.id.clone(),
        plan_id(&preview)?,
        Consent {
            permanent_loss: true,
            ..all_consents()
        },
    )?;
    assert_eq!(report["status"], "succeeded");
    assert!(!p.destination.join("obsolete").exists());
    assert!(!state.join(TRASH_FOLDER).exists());
    Ok(())
}

#[test]
fn retention_follows_the_size_limit_oldest_synchronization_first() -> Result<(), Failure> {
    // Given a 40-byte trash and two synchronizations of 30 then 11 bytes,
    let (temp, app, mut p) = fixture(TrashPolicy {
        enabled: true,
        max_bytes: 40,
    })?;
    let first = app.preview(p.id.clone())?;
    let first = app.execute_with(p.id.clone(), plan_id(&first)?, all_consents())?;
    let first = first["execution"].as_str().ok_or("execution")?.to_owned();
    fs::write(p.source.join("changed"), b"newest")?;
    p.name = "Corbeille modifiée".into();
    app.profile_save(p.clone())?;
    let second = app.preview(p.id.clone())?;
    let second = app.execute_with(p.id.clone(), plan_id(&second)?, all_consents())?;
    // When the second one exceeds the limit, Then the oldest folder goes.
    let overview = app.trash_overview(&p.id)?;
    assert_eq!(overview.runs.len(), 1);
    assert_eq!(overview.runs[0].execution, second["execution"]);
    assert_eq!(overview.used, 11);
    assert!(
        !temp
            .path()
            .join("state")
            .join(TRASH_FOLDER)
            .join("p")
            .join(first)
            .exists()
    );
    Ok(())
}
