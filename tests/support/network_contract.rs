// Contrat commun compilé séparément par les tests FTP et FTPS.
use connector_api::{CommitMode, StorageConnector, StorageErrorKind as Kind, WriteOptions};
use sync_core::{CancellationToken, EntryKind, ResourcePath};
struct Capture(std::sync::Mutex<Vec<String>>);
static LOGS: Capture = Capture(std::sync::Mutex::new(Vec::new()));

// The Python fixture verifies this entire test used exactly one login.
fn exercise_listing_reuse(source: &dyn StorageConnector) -> Result<(), Box<dyn std::error::Error>> {
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    for _ in 0..2 {
        let mut total = 0;
        let mut token = None;
        loop {
            let page = source.list(&root, token.as_ref(), &cancel)?;
            total += page.entries().len();
            token = page.next_page().cloned();
            if token.is_none() { break; }
        }
        assert_eq!(total, 601);
    }
    Ok(())
}
impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        if let Ok(mut lines) = self.0.lock() {
            lines.push(format!("{}", record.args()));
        }
    }
    fn flush(&self) {}
}

fn exercise(
    source: &dyn StorageConnector,
    destination: &dyn StorageConnector,
) -> Result<(), Box<dyn std::error::Error>> {
    log::set_logger(&LOGS).map_err(|_| "logger already installed")?;
    log::set_max_level(log::LevelFilter::Trace);
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    assert!(source.capabilities().is_read_only());
    let first = source.list(&root, None, &cancel)?;
    let mut total = first.entries().len();
    let mut token = first.next_page().cloned();
    while let Some(t) = token {
        let next = source.list(&root, Some(&t), &cancel)?;
        total += next.entries().len();
        token = next.next_page().cloned();
    }
    assert_eq!(total, 601);
    assert_eq!(
        source
            .stat(&ResourcePath::parse("missing")?, &cancel)
            .err()
            .ok_or("absence lost")?
            .kind(),
        Kind::NotFound
    );
    let mut reader = source.open_read(&ResourcePath::parse("file")?, &cancel)?;
    let mut bytes = [0; 4];
    assert_eq!(reader.read(&mut bytes)?, 3);
    assert_eq!(&bytes[..3], b"abc");
    assert_eq!(reader.read(&mut bytes)?, 0);
    let target = ResourcePath::parse("new")?;
    let mut writer = destination.open_write(&target, &WriteOptions::new(), &cancel)?;
    writer.write(b"new-content")?;
    assert!(source.stat(&target, &cancel).is_err());
    writer.finish()?;
    let stage = writer.staging_path().ok_or("no stage")?.clone();
    assert_eq!(source.stat(&stage, &cancel)?.metadata().size(), Some(11));
    // Given another SyncEngine writer, When root lock is held, Then refuse it.
    assert!(
        destination
            .open_write(
                &ResourcePath::parse("competing")?,
                &WriteOptions::new(),
                &cancel
            )
            .is_err()
    );
    writer.commit(CommitMode::CreateNew)?;
    assert!(source.stat(&stage, &cancel).is_err());
    let mut conflict = destination.open_write(&target, &WriteOptions::new(), &cancel)?;
    conflict.write(b"conflict")?;
    assert!(conflict.commit(CommitMode::CreateNew).is_err());
    assert_eq!(source.stat(&target, &cancel)?.metadata().size(), Some(11));
    let mut replace = destination.open_write(&target, &WriteOptions::new(), &cancel)?;
    replace.write(b"replace")?;
    replace.commit(CommitMode::ReplaceExisting)?;
    assert_eq!(source.stat(&target, &cancel)?.metadata().size(), Some(7));
    let mut abort = destination.open_write(
        &ResourcePath::parse("aborted")?,
        &WriteOptions::new(),
        &cancel,
    )?;
    abort.write(b"partial")?;
    abort.finish()?;
    abort.abort()?;
    assert!(
        source
            .stat(&ResourcePath::parse("aborted")?, &cancel)
            .is_err()
    );
    let dir = ResourcePath::parse("directory")?;
    destination.create_directory(&dir, &cancel)?;
    let nested = dir.join("file")?;
    let mut nested_write = destination.open_write(&nested, &WriteOptions::new(), &cancel)?;
    nested_write.write(b"keep")?;
    nested_write.commit(CommitMode::CreateNew)?;
    assert!(
        destination
            .delete(&dir, EntryKind::Directory, &cancel)
            .is_err()
    );
    assert_eq!(source.stat(&nested, &cancel)?.metadata().size(), Some(4));
    destination.delete(&nested, EntryKind::File, &cancel)?;
    destination.delete(&dir, EntryKind::Directory, &cancel)?;
    destination.delete(&target, EntryKind::File, &cancel)?;
    // Server truncates this fixture AFTER MLSD observed 8 bytes, before RETR.
    // Even a successful 226 must not turn that partial content into success.
    let short = ResourcePath::parse("short")?;
    let mut upload = destination.open_write(&short, &WriteOptions::new(), &cancel)?;
    upload.write(b"12345678")?;
    upload.commit(CommitMode::CreateNew)?;
    let mut reader = source.open_read(&short, &cancel)?;
    assert_eq!(reader.read(&mut bytes)?, 2);
    assert_eq!(
        reader
            .read(&mut bytes)
            .err()
            .ok_or("short transfer accepted")?
            .kind(),
        Kind::Disconnected
    );
    drop(reader);
    destination.delete(&short, EntryKind::File, &cancel)?;
    let stopped = CancellationToken::new();
    stopped.cancel();
    assert_eq!(
        source
            .list(&root, None, &stopped)
            .err()
            .ok_or("cancel ignored")?
            .kind(),
        Kind::Cancelled
    );
    assert!(
        source
            .stat(&ResourcePath::parse("file\r\nDELE file")?, &cancel)
            .is_err()
    );
    let logs = LOGS.0.lock().map_err(|_| "log capture poisoned")?;
    assert!(!logs.is_empty());
    assert!(
        !logs.iter().any(|s| s.contains("fixture-only")),
        "credential leaked at TRACE"
    );
    Ok(())
}
