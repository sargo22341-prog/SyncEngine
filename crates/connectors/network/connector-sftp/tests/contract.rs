//! Contrat SFTP et test réseau isolé piloté par scripts/test_phase08_sftp.py.
#![cfg(feature = "sftp")]

use connector_api::{StorageConnector, StorageErrorKind as Kind, StorageFeature, StorageOperation};
use connector_sftp::{SftpAuthentication, SftpConnector, SftpOptions};
use std::path::PathBuf;
use sync_core::{CancellationToken, EntryKind, ResourcePath};

fn options() -> SftpOptions {
    SftpOptions {
        address: ([127, 0, 0, 1], 1).into(),
        host: "fixture.test".into(),
        username: "fixture".into(),
        known_hosts: PathBuf::from("unused"),
        root: "/root".into(),
    }
}

#[test]
fn configuration_capabilities_cancel_and_mutation_refusals()
-> Result<(), Box<dyn std::error::Error>> {
    for root in [
        "relative",
        "/root/../outside",
        "/root/./child",
        "/root//child",
        "/root/",
        "/root\\child",
        "/root\0child",
    ] {
        let mut opts = options();
        opts.root = root.into();
        assert!(SftpConnector::new(opts).is_err());
    }
    let connector = SftpConnector::new(options())?;
    assert!(connector.capabilities().is_read_only());
    assert!(
        connector
            .capabilities()
            .supports(StorageFeature::StreamingRead)
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let root = ResourcePath::root();
    assert_eq!(
        connector
            .list(&root, None, &cancel)
            .err()
            .ok_or("no cancellation")?
            .kind(),
        Kind::Cancelled
    );
    assert_eq!(
        connector
            .stat(&root, &cancel)
            .err()
            .ok_or("no cancellation")?
            .kind(),
        Kind::Cancelled
    );
    assert_eq!(
        connector
            .open_read(&root, &cancel)
            .err()
            .ok_or("no cancellation")?
            .kind(),
        Kind::Cancelled
    );
    assert_eq!(
        connector
            .delete(&root, EntryKind::Directory, &cancel)
            .err()
            .ok_or("no refusal")?
            .kind(),
        Kind::NotSupported
    );
    assert_eq!(
        connector
            .create_directory(&root, &cancel)
            .err()
            .ok_or("no refusal")?
            .kind(),
        Kind::NotSupported
    );
    Ok(())
}

struct FixtureAuthentication;
impl SftpAuthentication for FixtureAuthentication {
    fn authenticate(&self, session: &ssh2::Session, username: &str) -> Result<(), ssh2::Error> {
        session.userauth_password(username, "fixture-only")
    }
}

struct CountAuthentication(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl SftpAuthentication for CountAuthentication {
    fn authenticate(&self, session: &ssh2::Session, username: &str) -> Result<(), ssh2::Error> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        session.userauth_password(username, "fixture-only")
    }
}

#[test]
#[ignore = "serveur Paramiko éphémère requis : scripts/test_phase08_sftp.py"]
fn isolated_sftp_contract() -> Result<(), Box<dyn std::error::Error>> {
    let mut opts = options();
    opts.address = std::env::var("SYNCENGINE_SFTP_ADDR")?.parse()?;
    opts.known_hosts = std::env::var_os("SYNCENGINE_SFTP_HOSTS")
        .ok_or("missing fixture")?
        .into();
    let authentications = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let connector = SftpConnector::with_authentication(
        opts,
        Box::new(CountAuthentication(authentications.clone())),
    )?;
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    let first = connector.list(&root, None, &cancel)?;
    assert_eq!(first.entries().len(), 256);
    let mut count = first.entries().len();
    let mut token = first.next_page().cloned();
    while let Some(page) = token {
        let next = connector.list(&root, Some(&page), &cancel)?;
        count += next.entries().len();
        token = next.next_page().cloned();
    }
    assert_eq!(count, 605);
    assert!(connector.list(&root, None, &cancel)?.next_page().is_some());
    assert!(
        connector
            .list(&ResourcePath::parse("empty")?, None, &cancel)?
            .entries()
            .is_empty()
    );
    assert_eq!(
        authentications.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "successive directory reads must reuse their authenticated session"
    );
    assert!(
        connector
            .list(&ResourcePath::parse("denied")?, None, &cancel)
            .is_err()
    );
    assert!(
        connector
            .list(&ResourcePath::parse("empty")?, None, &cancel)?
            .entries()
            .is_empty()
    );
    assert_eq!(
        authentications.load(std::sync::atomic::Ordering::Relaxed),
        2,
        "an errored listing must discard its session before the next read"
    );
    assert!(
        connector
            .list(&ResourcePath::parse("transient")?, None, &cancel)?
            .entries()
            .is_empty()
    );
    assert_eq!(
        authentications.load(std::sync::atomic::Ordering::Relaxed),
        3,
        "transient listing failure must reconnect once and restart the entire directory"
    );
    let mut destination_options = options();
    destination_options.address = std::env::var("SYNCENGINE_SFTP_ADDR")?.parse()?;
    destination_options.known_hosts = std::env::var_os("SYNCENGINE_SFTP_HOSTS")
        .ok_or("missing fixture")?
        .into();
    let writable =
        SftpConnector::with_authentication(destination_options, Box::new(FixtureAuthentication))?
            .with_mutations();
    let target = ResourcePath::parse("new-file")?;
    let mut writer = writable.open_write(&target, &connector_api::WriteOptions::new(), &cancel)?;
    writer.write(b"new-data")?;
    assert_eq!(
        writable
            .stat(&target, &cancel)
            .err()
            .ok_or("published too soon")?
            .kind(),
        Kind::NotFound
    );
    writer.finish()?;
    let stage = writer.staging_path().ok_or("no staging path")?.clone();
    assert_eq!(writable.stat(&stage, &cancel)?.metadata().size(), Some(8));
    assert_eq!(
        writer
            .commit(connector_api::CommitMode::CreateNew)?
            .metadata()
            .size(),
        Some(8)
    );
    assert!(writable.stat(&stage, &cancel).is_err());
    let mut conflicting =
        writable.open_write(&target, &connector_api::WriteOptions::new(), &cancel)?;
    conflicting.write(b"must-not-overwrite")?;
    assert!(
        conflicting
            .commit(connector_api::CommitMode::CreateNew)
            .is_err()
    );
    assert_eq!(writable.stat(&target, &cancel)?.metadata().size(), Some(8));
    let mut replace = writable.open_write(&target, &connector_api::WriteOptions::new(), &cancel)?;
    replace.write(b"replaced")?;
    replace.commit(connector_api::CommitMode::ReplaceExisting)?;
    let abandoned = ResourcePath::parse("abandoned")?;
    let mut aborted =
        writable.open_write(&abandoned, &connector_api::WriteOptions::new(), &cancel)?;
    aborted.write(b"partial")?;
    aborted.finish()?;
    aborted.abort()?;
    assert!(writable.stat(&abandoned, &cancel).is_err());
    writable.delete(&target, EntryKind::File, &cancel)?;
    // Given failure after preserving the old target, When publication fails,
    // Then the original remains in the diagnostic staging directory.
    let failing = ResourcePath::parse("fail-publish")?;
    let mut original =
        writable.open_write(&failing, &connector_api::WriteOptions::new(), &cancel)?;
    original.write(b"original")?;
    original.commit(connector_api::CommitMode::CreateNew)?;
    let mut interrupted =
        writable.open_write(&failing, &connector_api::WriteOptions::new(), &cancel)?;
    interrupted.write(b"new")?;
    interrupted.finish()?;
    let previous = interrupted
        .staging_path()
        .and_then(ResourcePath::parent)
        .ok_or("missing reservation")?
        .join("previous")?;
    assert!(
        interrupted
            .commit(connector_api::CommitMode::ReplaceExisting)
            .is_err()
    );
    let mut saved = writable.open_read(&previous, &cancel)?;
    let mut original_bytes = [0; 8];
    assert_eq!(saved.read(&mut original_bytes)?, 8);
    assert_eq!(&original_bytes, b"original");
    let dir = ResourcePath::parse("new-dir")?;
    writable.create_directory(&dir, &cancel)?;
    writable.delete(&dir, EntryKind::Directory, &cancel)?;
    assert!(
        writable
            .delete(&root, EntryKind::Directory, &cancel)
            .is_err()
    );
    assert_eq!(
        connector
            .stat(&ResourcePath::parse("link")?, &cancel)?
            .kind(),
        EntryKind::Symlink
    );
    assert_eq!(
        connector
            .open_read(&ResourcePath::parse("link")?, &cancel)
            .err()
            .ok_or("link followed")?
            .kind(),
        Kind::NotSupported
    );
    assert!(
        connector
            .list(&ResourcePath::parse("link/child")?, None, &cancel)
            .is_err()
    );
    let err = connector
        .stat(&ResourcePath::parse("missing")?, &cancel)
        .err()
        .ok_or("missing accepted")?;
    assert_eq!(err.kind(), Kind::NotFound);
    assert_eq!(err.operation(), StorageOperation::Stat);
    assert_eq!(
        connector
            .list(&ResourcePath::parse("denied")?, None, &cancel)
            .err()
            .ok_or("permission ignored")?
            .kind(),
        Kind::PermissionDenied
    );
    assert!(
        connector
            .list(&ResourcePath::parse("empty")?, None, &cancel)?
            .entries()
            .is_empty()
    );
    // Given an unrepresentable name, When listing, Then fail the entire listing.
    assert_eq!(
        connector
            .list(&ResourcePath::parse("nonutf8")?, None, &cancel)
            .err()
            .ok_or("non-UTF8 name hidden")?
            .kind(),
        Kind::NotSupported
    );
    let path = ResourcePath::parse("file")?;
    let mut reader = connector.open_read(&path, &cancel)?;
    let mut bytes = [0; 2];
    assert_eq!(reader.read(&mut bytes)?, 2);
    assert_eq!(&bytes, b"ab");
    assert_eq!(reader.read(&mut bytes)?, 1);
    assert_eq!(bytes[0], b'c');
    assert_eq!(reader.read(&mut bytes)?, 0);
    let interrupted = CancellationToken::new();
    let mut reader = connector.open_read(&path, &interrupted)?;
    interrupted.cancel();
    assert_eq!(
        reader
            .read(&mut bytes)
            .err()
            .ok_or("cancel ignored")?
            .kind(),
        Kind::Cancelled
    );
    let mut broken = connector.open_read(&ResourcePath::parse("broken")?, &cancel)?;
    assert_eq!(broken.read(&mut bytes)?, 2);
    assert_eq!(
        broken
            .read(&mut bytes)
            .err()
            .ok_or("truncation hidden")?
            .kind(),
        Kind::Disconnected
    );
    // Given an untrusted host, When opening, Then credentials are never requested.
    struct NeverAuthenticate;
    impl SftpAuthentication for NeverAuthenticate {
        fn authenticate(&self, _: &ssh2::Session, _: &str) -> Result<(), ssh2::Error> {
            Err(ssh2::Error::new(
                ssh2::ErrorCode::Session(-1),
                "AUTHENTICATION WAS CALLED",
            ))
        }
    }
    let mut bad = options();
    bad.address = std::env::var("SYNCENGINE_SFTP_ADDR")?.parse()?;
    bad.known_hosts = std::env::var_os("SYNCENGINE_SFTP_BAD_HOSTS")
        .ok_or("missing fixture")?
        .into();
    let connector = SftpConnector::with_authentication(bad, Box::new(NeverAuthenticate))?;
    let err = connector
        .stat(&root, &cancel)
        .err()
        .ok_or("untrusted host accepted")?;
    assert_eq!(err.kind(), Kind::Unauthenticated);
    assert!(err.to_string().contains("clé hôte"));
    assert!(!format!("{err:?}").contains("fixture-only"));
    Ok(())
}
