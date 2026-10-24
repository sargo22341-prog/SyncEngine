//! Mutations du connecteur local, dans des répertoires temporaires isolés.
//!
//! Aucun test ne touche à un dossier personnel : tout est créé sous un
//! `tempfile::TempDir` supprimé à la fin.

#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

use std::fs;
use std::path::Path;

use connector_api::{CommitMode, StorageConnector, StorageErrorKind, WriteOptions};
use connector_local::{LocalConnector, LocalOptions, is_staging_name};
use sync_core::{CancellationToken, EntryKind, ResourcePath, Timestamp};
use tempfile::TempDir;

fn p(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap()
}

fn writable(root: &Path) -> LocalConnector {
    LocalConnector::new(root, LocalOptions::new().with_mutations()).unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn write_file(
    connector: &LocalConnector,
    at: &str,
    content: &[u8],
    mode: CommitMode,
    options: WriteOptions,
) -> Result<sync_core::FileEntry, connector_api::StorageError> {
    let cancel = CancellationToken::new();
    let mut writer = connector.open_write(&p(at), &options, &cancel)?;
    for chunk in content.chunks(3) {
        writer.write(chunk)?;
    }
    writer.commit(mode)
}

#[test]
fn mutations_are_refused_unless_enabled() {
    let dir = TempDir::new().unwrap();
    let connector = LocalConnector::new(dir.path(), LocalOptions::new()).unwrap();
    let cancel = CancellationToken::new();
    assert!(connector.capabilities().is_read_only());
    let error = connector.create_directory(&p("d"), &cancel).unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotSupported);
    let error = connector
        .open_write(&p("f"), &WriteOptions::new(), &cancel)
        .err()
        .unwrap();
    assert_eq!(error.kind(), StorageErrorKind::NotSupported);
    fs::write(dir.path().join("x"), b"x").unwrap();
    let error = connector
        .delete(&p("x"), EntryKind::File, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotSupported);
    assert_eq!(names(dir.path()), ["x"]);
    assert!(!writable(dir.path()).capabilities().is_read_only());
}

#[test]
fn staged_write_is_invisible_until_commit_and_keeps_the_date() {
    let dir = TempDir::new().unwrap();
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();
    let modified = Timestamp::new(1_700_000_000, 500_000_000).unwrap();
    let options = WriteOptions::new()
        .with_modified(Some(modified))
        .with_size_hint(Some(6));
    let mut writer = connector
        .open_write(&p("doc.txt"), &options, &cancel)
        .unwrap();
    writer.write(b"bon").unwrap();
    writer.write(b"jour").unwrap();
    let staging = writer.staging_path().unwrap().clone();
    assert!(is_staging_name(staging.file_name().unwrap()));
    // Avant validation : seul le fichier temporaire existe.
    assert_eq!(names(dir.path()), [staging.as_str().to_owned()]);
    let entry = writer.commit(CommitMode::CreateNew).unwrap();
    assert_eq!(names(dir.path()), ["doc.txt"]);
    assert_eq!(fs::read(dir.path().join("doc.txt")).unwrap(), b"bonjour");
    assert_eq!(entry.kind(), EntryKind::File);
    assert_eq!(entry.metadata().size(), Some(7));
    assert_eq!(entry.metadata().modified(), Some(modified));
}

#[test]
fn create_new_never_overwrites() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("existant.txt"), b"precieux").unwrap();
    let connector = writable(dir.path());
    let error = write_file(
        &connector,
        "existant.txt",
        b"nouveau",
        CommitMode::CreateNew,
        WriteOptions::new(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Conflict);
    assert_eq!(
        fs::read(dir.path().join("existant.txt")).unwrap(),
        b"precieux"
    );
    // Le fichier temporaire est retiré.
    assert_eq!(names(dir.path()), ["existant.txt"]);
}

#[test]
fn replace_requires_an_existing_regular_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("f.txt"), b"ancien").unwrap();
    fs::create_dir(dir.path().join("dossier")).unwrap();
    let connector = writable(dir.path());
    write_file(
        &connector,
        "f.txt",
        b"remplace",
        CommitMode::ReplaceExisting,
        WriteOptions::new(),
    )
    .unwrap();
    assert_eq!(fs::read(dir.path().join("f.txt")).unwrap(), b"remplace");

    for target in ["absent.txt", "dossier"] {
        let error = write_file(
            &connector,
            target,
            b"x",
            CommitMode::ReplaceExisting,
            WriteOptions::new(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::Conflict, "{target}");
    }
    assert!(dir.path().join("dossier").is_dir());
    assert_eq!(names(dir.path()), ["dossier", "f.txt"]);
}

/// Un antivirus ou un indexeur ouvre brièvement un fichier fraîchement écrit
/// sans `FILE_SHARE_DELETE` : le remplacement attend sa libération au lieu
/// d'échouer avec « accès refusé ».
#[cfg(windows)]
#[test]
fn replace_waits_for_a_brief_foreign_lock() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("f.txt");
    fs::write(&target, b"ancien").unwrap();
    let holder = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&target)
        .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(60));
        drop(holder);
    });
    let connector = writable(dir.path());
    write_file(
        &connector,
        "f.txt",
        b"remplace",
        CommitMode::ReplaceExisting,
        WriteOptions::new(),
    )
    .unwrap();
    release.join().unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"remplace");
    assert_eq!(names(dir.path()), ["f.txt"]);
}

#[test]
fn abort_and_drop_remove_the_staging_file() {
    let dir = TempDir::new().unwrap();
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();
    let mut writer = connector
        .open_write(&p("a.txt"), &WriteOptions::new(), &cancel)
        .unwrap();
    writer.write(b"partiel").unwrap();
    writer.abort().unwrap();
    assert!(names(dir.path()).is_empty());

    let mut writer = connector
        .open_write(&p("b.txt"), &WriteOptions::new(), &cancel)
        .unwrap();
    writer.write(b"partiel").unwrap();
    drop(writer);
    assert!(names(dir.path()).is_empty());
}

#[test]
fn cancellation_stops_writing_and_publishing() {
    let dir = TempDir::new().unwrap();
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();
    let mut writer = connector
        .open_write(&p("a.txt"), &WriteOptions::new(), &cancel)
        .unwrap();
    writer.write(b"debut").unwrap();
    cancel.cancel();
    assert_eq!(
        writer.write(b"suite").unwrap_err().kind(),
        StorageErrorKind::Cancelled
    );
    assert_eq!(
        writer.commit(CommitMode::CreateNew).unwrap_err().kind(),
        StorageErrorKind::Cancelled
    );
    assert!(names(dir.path()).is_empty());
}

#[test]
fn missing_parent_is_not_created_implicitly() {
    let dir = TempDir::new().unwrap();
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();
    let error = connector
        .open_write(&p("absent/f.txt"), &WriteOptions::new(), &cancel)
        .err()
        .unwrap();
    assert_eq!(error.kind(), StorageErrorKind::NotFound);
    let error = connector
        .create_directory(&p("absent/sous"), &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotFound);
    assert!(names(dir.path()).is_empty());
}

#[test]
fn create_directory_once() {
    let dir = TempDir::new().unwrap();
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();
    let entry = connector.create_directory(&p("d"), &cancel).unwrap();
    assert_eq!(entry.kind(), EntryKind::Directory);
    connector.create_directory(&p("d/e"), &cancel).unwrap();
    assert!(dir.path().join("d").join("e").is_dir());
    let error = connector.create_directory(&p("d"), &cancel).unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Conflict);
    let error = connector
        .create_directory(&ResourcePath::root(), &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Permanent);
}

#[test]
fn delete_is_never_recursive_and_checks_the_kind() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("plein")).unwrap();
    fs::write(root.join("plein").join("garde.txt"), b"g").unwrap();
    fs::create_dir(root.join("vide")).unwrap();
    fs::write(root.join("f.txt"), b"f").unwrap();
    let connector = writable(root);
    let cancel = CancellationToken::new();

    let error = connector
        .delete(&p("plein"), EntryKind::Directory, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Conflict);
    assert!(root.join("plein").join("garde.txt").exists());

    let error = connector
        .delete(&p("f.txt"), EntryKind::Directory, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Conflict);
    let error = connector
        .delete(&p("vide"), EntryKind::File, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Conflict);
    let error = connector
        .delete(&p("absent"), EntryKind::File, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotFound);

    connector
        .delete(&p("f.txt"), EntryKind::File, &cancel)
        .unwrap();
    connector
        .delete(&p("vide"), EntryKind::Directory, &cancel)
        .unwrap();
    assert_eq!(names(root), ["plein"]);
}

#[test]
fn missing_root_is_disconnected_for_mutations() {
    let dir = TempDir::new().unwrap();
    let connector = writable(&dir.path().join("absent"));
    let error = connector
        .create_directory(&p("d"), &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Disconnected);
}

/// Crée un lien vers un répertoire **hors** de la racine.
/// Windows : jonction (`mklink /J`, sans privilège administrateur).
fn link_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J a échoué : {status:?}");
    }
}

#[test]
fn mutations_never_go_through_a_link() {
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("cible.txt"), b"hors racine").unwrap();
    let dir = TempDir::new().unwrap();
    link_dir(outside.path(), &dir.path().join("lien"));
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();

    let error = connector
        .open_write(&p("lien/nouveau.txt"), &WriteOptions::new(), &cancel)
        .err()
        .unwrap();
    assert_eq!(error.kind(), StorageErrorKind::Permanent);
    let error = connector
        .delete(&p("lien/cible.txt"), EntryKind::File, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Permanent);
    let error = connector
        .create_directory(&p("lien/d"), &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Permanent);
    // Le lien lui-même n'est jamais supprimé.
    let error = connector
        .delete(&p("lien"), EntryKind::Directory, &cancel)
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotSupported);

    assert_eq!(names(outside.path()), ["cible.txt"]);
    assert_eq!(
        fs::read(outside.path().join("cible.txt")).unwrap(),
        b"hors racine"
    );
}

#[test]
fn reserved_parent_cannot_redirect_publication_or_cleanup_outside_root() {
    // Given a reserved writer, When an adversary replaces its logical parent,
    // Then publication stays in the granted directory and outside data survives.
    let root = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("new.txt"), b"precious").unwrap();
    fs::create_dir(root.path().join("parent")).unwrap();
    let connector = writable(root.path());
    let mut writer = connector
        .open_write(
            &p("parent/new.txt"),
            &WriteOptions::new(),
            &CancellationToken::new(),
        )
        .unwrap();
    writer.write(b"verified").unwrap();
    let moved = fs::rename(root.path().join("parent"), root.path().join("moved"));
    #[cfg(unix)]
    assert!(moved.is_ok());
    if moved.is_ok() {
        link_dir(outside.path(), &root.path().join("parent"));
    }
    writer.commit(CommitMode::CreateNew).unwrap();
    assert_eq!(
        fs::read(outside.path().join("new.txt")).unwrap(),
        b"precious"
    );
    let parent = if moved.is_ok() { "moved" } else { "parent" };
    assert_eq!(
        fs::read(root.path().join(parent).join("new.txt")).unwrap(),
        b"verified"
    );
}

#[test]
fn staged_content_can_be_reread_through_the_writer_before_commit() {
    let dir = TempDir::new().unwrap();
    let connector = writable(dir.path());
    let cancel = CancellationToken::new();
    let mut writer = connector
        .open_write(&p("f.bin"), &WriteOptions::new(), &cancel)
        .unwrap();
    let content: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    writer.write(&content[..7000]).unwrap();
    writer.write(&content[7000..]).unwrap();
    writer.finish().unwrap();
    let mut reread = Vec::new();
    let mut buffer = [0u8; 4096];
    assert!(
        writer
            .read_back(&mut buffer, &mut |chunk| reread.extend_from_slice(chunk))
            .unwrap()
    );
    assert_eq!(reread, content);
    // Reading back publishes nothing; the commit still does.
    assert!(!dir.path().join("f.bin").exists());
    writer.commit(CommitMode::CreateNew).unwrap();
    assert_eq!(fs::read(dir.path().join("f.bin")).unwrap(), content);
    assert_eq!(names(dir.path()), ["f.bin"]);

    // Cancellation is honoured while rereading.
    let token = CancellationToken::new();
    let mut writer = connector
        .open_write(&p("g.bin"), &WriteOptions::new(), &token)
        .unwrap();
    writer.write(b"abc").unwrap();
    token.cancel();
    let error = writer.read_back(&mut buffer, &mut |_| {}).unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Cancelled);
    writer.abort().unwrap();
    assert_eq!(names(dir.path()), ["f.bin"]);
}
