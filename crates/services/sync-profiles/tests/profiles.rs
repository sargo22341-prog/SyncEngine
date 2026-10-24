//! Configuration persistence and migration boundaries; temporary stores only.
use sync_core::ProfileId;
use sync_filter::FilterRules;
use sync_plan::SyncMode;
use sync_profiles::{CompareMethod, Preferences, Profile, Profiles};

#[test]
fn migration_round_trip_edit_remove_and_preferences() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("profiles.sqlite");
    let store = Profiles::open(&path)?;
    let mut profile = Profile {
        id: ProfileId::new("p")?,
        name: "Été & photos".into(),
        group: "Archives".into(),
        mode: SyncMode::Backup,
        source: temp.path().join("source"),
        destination: temp.path().join("destination"),
        filter: FilterRules::new().exclude_file("*.tmp"),
        remote_source: None,
        remote_destination: None,
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
    };
    store.save(&profile)?;
    assert_eq!(store.list()?, vec![profile.clone()]);
    profile.name = "Autre nom".into();
    store.save(&profile)?;
    store.save_preferences(&Preferences { compact: true })?;
    drop(store);
    let store = Profiles::open(&path)?;
    assert_eq!(store.list()?, vec![profile.clone()]);
    assert!(store.preferences()?.compact);
    store.remove(&profile.id)?;
    assert!(store.list()?.is_empty());
    Ok(())
}

#[test]
fn invalid_edits_keep_previous_row() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let store = Profiles::open(temp.path().join("profiles.sqlite"))?;
    let profile = Profile {
        id: ProfileId::new("p")?,
        name: "Original".into(),
        group: String::new(),
        mode: SyncMode::Backup,
        source: temp.path().join("a"),
        destination: temp.path().join("b"),
        filter: FilterRules::new(),
        remote_source: None,
        remote_destination: None,
        allow_delete: false,
        compare: sync_profiles::CompareMethod::Content,
    };
    store.save(&profile)?;
    let mut invalid = profile.clone();
    invalid.allow_delete = true;
    assert!(store.save(&invalid).is_err());
    invalid = profile.clone();
    invalid.source = "relative".into();
    assert!(store.save(&invalid).is_err());
    invalid = profile.clone();
    invalid.filter = FilterRules::new().exclude_file("[");
    assert!(store.save(&invalid).is_err());
    assert_eq!(store.list()?, vec![profile]);
    Ok(())
}

#[test]
fn future_foreign_corrupt_stores_are_not_reset() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let future = temp.path().join("future.sqlite");
    rusqlite::Connection::open(&future)?.execute_batch("PRAGMA user_version=99;")?;
    assert!(Profiles::open(&future).is_err());
    let foreign = temp.path().join("foreign.sqlite");
    rusqlite::Connection::open(&foreign)?.execute_batch("CREATE TABLE personal(data TEXT);")?;
    assert!(Profiles::open(&foreign).is_err());
    let corrupt = temp.path().join("corrupt.sqlite");
    std::fs::write(&corrupt, b"not sqlite")?;
    assert!(Profiles::open(&corrupt).is_err());
    assert_eq!(std::fs::read(corrupt)?, b"not sqlite");
    Ok(())
}

#[test]
fn migrates_v1_without_rewriting_local_configuration_and_rejects_secret_urls()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("profiles.sqlite");
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch(include_str!("../migrations/001_initial.sql"))?;
    let legacy = serde_json::json!({"id":"old","name":"Legacy","group":"","mode":"backup",
        "source":temp.path().join("a"),"destination":temp.path().join("b"),"filter":FilterRules::new(),"allow_delete":false}).to_string();
    db.execute("INSERT INTO profiles VALUES('old',?1)", [&legacy])?;
    drop(db);
    let store = Profiles::open(&path)?;
    let mut p = store.list()?.remove(0);
    assert!(p.remote_source.is_none());
    let db = rusqlite::Connection::open(&path)?;
    assert_eq!(
        db.query_row("SELECT config FROM profiles", [], |r| r.get::<_, String>(0))?,
        legacy
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
        3
    );
    p.remote_source = Some(sync_profiles::RemoteEndpoint::Webdav {
        root: "https://user:secret@example.org/".into(),
        credential_ref: "DAV".into(),
    });
    assert!(store.save(&p).is_err());
    p.remote_source = Some(sync_profiles::RemoteEndpoint::Onedrive {
        drive_id: "fixture-drive".into(),
        download_hosts: vec![],
        credential_ref: "DRIVE".into(),
    });
    store.save(&p)?;
    assert_eq!(store.list()?, vec![p.clone()]);
    p.remote_destination = p.remote_source.clone();
    assert!(store.save(&p).is_err());
    p.remote_destination = None;
    p.mode = SyncMode::Bidirectional;
    assert!(store.save(&p).is_err());
    Ok(())
}

#[test]
fn compare_method_is_additive_and_bidirectional_keeps_content()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("profiles.sqlite");
    // Version 2 store written by the previous release.
    let db = rusqlite::Connection::open(&path)?;
    db.execute_batch(include_str!("../migrations/001_initial.sql"))?;
    db.execute_batch("PRAGMA user_version=2;")?;
    let legacy = serde_json::json!({"id":"old","name":"Legacy","group":"","mode":"mirror",
        "source":temp.path().join("a"),"destination":temp.path().join("b"),"filter":FilterRules::new(),"allow_delete":true}).to_string();
    db.execute("INSERT INTO profiles VALUES('old',?1)", [&legacy])?;
    drop(db);
    let store = Profiles::open(&path)?;
    let mut p = store.list()?.remove(0);
    // Absent field: previous behavior (content hashes), never weakened silently.
    assert_eq!(p.compare, CompareMethod::Content);
    let db = rusqlite::Connection::open(&path)?;
    assert_eq!(
        db.query_row("SELECT config FROM profiles", [], |r| r.get::<_, String>(0))?,
        legacy
    );
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?,
        3
    );
    // Saving a content profile keeps the version 2 JSON shape.
    store.save(&p)?;
    let stored: String = db.query_row("SELECT config FROM profiles", [], |r| r.get(0))?;
    assert!(!stored.contains("compare"));
    p.compare = CompareMethod::SizeAndDate;
    store.save(&p)?;
    let stored: String = db.query_row("SELECT config FROM profiles", [], |r| r.get(0))?;
    assert!(stored.contains(r#""compare":"size_and_date""#));
    drop(store);
    assert_eq!(Profiles::open(&path)?.list()?, vec![p.clone()]);
    // Bidirectional needs hashes of every file for its reference state.
    p.mode = SyncMode::Bidirectional;
    p.allow_delete = false;
    assert!(p.validate().is_err());
    p.compare = CompareMethod::Content;
    p.validate()?;
    Ok(())
}
