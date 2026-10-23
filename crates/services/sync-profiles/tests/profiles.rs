//! Configuration persistence and migration boundaries; temporary stores only.
use sync_core::ProfileId;
use sync_filter::FilterRules;
use sync_plan::SyncMode;
use sync_profiles::{Preferences, Profile, Profiles};

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
        allow_delete: false,
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
        allow_delete: false,
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
