//! Interrupted executions: diagnosis from the journal, acknowledgement and
//! resumption by a new preview, through the real local facade; temporary
//! directories only.
use std::fs;
use std::path::Path;
use sync_core::{ExecutionId, OperationId, PlanId, ProfileId, ResourcePath};
use sync_filter::FilterRules;
use sync_journal::{FileJournal, JournalRecord, JournalSink, OutcomeCode};
use sync_orchestrator::Application;
use sync_plan::{Action, SyncMode};
use sync_profiles::{Profile, TrashPolicy};

type Failure = Box<dyn std::error::Error>;

const STAGING: &str = ".syncengine-41-7-0.partial";

/// Backup profile with two source files; `done.txt` was already copied and
/// `big.bin` was being copied when the process stopped.
fn crashed() -> Result<(tempfile::TempDir, Application, Profile), Failure> {
    let temp = tempfile::tempdir()?;
    for directory in ["source", "destination", "state"] {
        fs::create_dir(temp.path().join(directory))?;
    }
    let state = temp.path().join("state");
    let app = Application::open_with_lock_root(&state, temp.path().join("locks"))?;
    let profile = Profile {
        id: ProfileId::new("p")?,
        name: "Interrompu".into(),
        group: String::new(),
        mode: SyncMode::Backup,
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        filter: FilterRules::new(),
        remote_source: None,
        remote_destination: None,
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
        trash: TrashPolicy::disabled(),
    };
    app.profile_save(profile.clone())?;
    fs::write(profile.source.join("done.txt"), b"copied before the crash")?;
    fs::write(profile.source.join("big.bin"), b"never finished")?;
    fs::write(
        profile.destination.join("done.txt"),
        b"copied before the crash",
    )?;
    let preview = app.preview(profile.id.clone())?;
    // What a killed process leaves: an attempt still running, its journal
    // and a temporary file at the destination.
    rusqlite::Connection::open(state.join("desktop-runs.sqlite"))?.execute(
        "INSERT INTO runs VALUES('run-crash',?1,'{\"secs\":1700000000,\"nanos\":0}','running',?2,NULL)",
        rusqlite::params![profile.id.as_str(), preview["plan"].to_string()],
    )?;
    let mut journal = FileJournal::create(state.join("run-crash.jsonl"))?;
    let op = |n: &str| OperationId::new(n);
    let path = |p: &str| ResourcePath::parse(p);
    journal.append(JournalRecord::ExecutionStarted {
        execution: ExecutionId::new("run-crash")?,
        plan: PlanId::new("plan")?,
        profile: Some(profile.id.clone()),
        operations: 2,
        mutations: 2,
        source: "source".into(),
        destination: "destination".into(),
        versions: None,
    })?;
    journal.append(JournalRecord::OperationStarted {
        operation: op("op-1")?,
        path: path("done.txt")?,
        action: Action::Copy,
    })?;
    journal.append(JournalRecord::OperationFinished {
        operation: op("op-1")?,
        path: path("done.txt")?,
        action: Action::Copy,
        outcome: OutcomeCode::Done,
        bytes: 23,
        error_class: None,
        detail: None,
    })?;
    journal.append(JournalRecord::OperationStarted {
        operation: op("op-2")?,
        path: path("big.bin")?,
        action: Action::Copy,
    })?;
    journal.append(JournalRecord::StagingCreated {
        operation: op("op-2")?,
        staging: path(STAGING)?,
    })?;
    fs::write(profile.destination.join(STAGING), b"never")?;
    Ok((temp, app, profile))
}

fn journal(temp: &tempfile::TempDir) -> std::path::PathBuf {
    temp.path().join("state").join("run-crash.jsonl")
}

#[test]
fn a_crash_is_reported_with_its_unfinished_files_and_keeps_its_journal() -> Result<(), Failure> {
    // Given an execution stopped by a crash,
    let (temp, app, p) = crashed()?;
    // When the application lists interruptions (and opens again),
    let list = app.interruptions()?;
    drop(app);
    let app =
        Application::open_with_lock_root(temp.path().join("state"), temp.path().join("locks"))?;
    // Then it is reported with its counts and the journal stays for diagnosis.
    assert_eq!(list.len(), 1);
    let interruption = &list[0];
    assert_eq!(interruption.profile, p.id);
    assert_eq!(interruption.started, Some(1_700_000_000));
    assert_eq!(
        (
            interruption.planned,
            interruption.finished,
            interruption.in_flight
        ),
        (2, 1, 1)
    );
    assert!(interruption.journal_error.is_none());
    assert!(journal(&temp).exists());
    // The detail highlights the file started but never finished.
    let detail = app.interruption_detail(&p.id, "run-crash")?;
    assert_eq!(detail.in_flight.len(), 1);
    assert_eq!(detail.in_flight[0].path, "big.bin");
    assert_eq!(detail.in_flight[0].staging.as_deref(), Some(STAGING));
    assert_eq!(detail.recent.len(), 1);
    assert_eq!(detail.totals.done, 1);
    assert!(app.interruption_detail(&p.id, "run-other").is_err());
    Ok(())
}

#[test]
fn ignoring_acknowledges_first_then_removes_the_journal() -> Result<(), Failure> {
    // Given a reported interruption,
    let (temp, app, p) = crashed()?;
    // When the user has checked and ignores it,
    let warnings = app.acknowledge_interrupted(&p.id, true)?;
    // Then the attempt is a failure, nothing is reported any more, the
    // journal is gone and the temporary file is left as it was.
    assert!(warnings.is_empty());
    assert_eq!(app.history(p.id.clone())?[0].state, "error");
    assert!(app.interruptions()?.is_empty());
    assert!(!journal(&temp).exists());
    assert!(p.destination.join(STAGING).exists());
    Ok(())
}

#[test]
fn resuming_cleans_temporaries_and_starts_from_a_new_preview() -> Result<(), Failure> {
    // Given an interruption, a recorded staging path that is not a staging
    // name, and a user file that merely looks like one,
    let (temp, app, p) = crashed()?;
    fs::write(p.destination.join(".syncengine-user.partial"), b"user")?;
    // When the user resumes,
    let resumed = app.resume_interrupted(&p.id)?;
    // Then only the recorded temporary file is removed, after acknowledgement,
    // and the journal goes; nothing is replayed (big.bin is not copied).
    assert_eq!(resumed.removed, [STAGING]);
    assert!(resumed.leftovers.is_empty());
    assert!(resumed.warnings.is_empty());
    assert!(!p.destination.join(STAGING).exists());
    assert!(p.destination.join(".syncengine-user.partial").exists());
    assert!(!p.destination.join("big.bin").exists());
    assert!(!journal(&temp).exists());
    assert!(app.interruptions()?.is_empty());
    // A new full preview skips the file already copied and copies the rest.
    let preview = app.preview(p.id.clone())?;
    assert_eq!(preview["summary"]["copy"], 1);
    let copy = preview["plan"]["operations"]
        .as_array()
        .ok_or("operations")?
        .iter()
        .find(|op| op["action"] == "copy")
        .ok_or("copy")?;
    assert_eq!(copy["path"], "big.bin");
    Ok(())
}

#[test]
fn a_recorded_path_without_the_staging_name_is_never_removed() -> Result<(), Failure> {
    // Given a journal whose staging record names a user file,
    let (temp, app, p) = crashed()?;
    let path = journal(&temp);
    let text = fs::read_to_string(&path)?.replace(STAGING, "done.txt");
    fs::write(&path, text)?;
    // When the user resumes, Then that file is listed to check, not removed.
    let resumed = app.resume_interrupted(&p.id)?;
    assert!(resumed.removed.is_empty());
    assert_eq!(resumed.leftovers.len(), 1);
    assert!(Path::new(&p.destination.join("done.txt")).exists());
    Ok(())
}
