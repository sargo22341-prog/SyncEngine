//! Tests du connecteur local dans des répertoires temporaires isolés.
//!
//! Aucun test ne touche à un dossier personnel : tout est créé sous un
//! `tempfile::TempDir` supprimé à la fin.

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

use std::fs;
use std::path::Path;

use connector_api::{
    ListPage, ListingIssueKind, NameIssue, PageToken, StorageConnector, StorageErrorKind,
};
use connector_local::{LocalConnector, LocalOptions};
use sync_core::{CancellationToken, EntryKind, FileEntry, ResourcePath, TimestampPrecision};
use tempfile::TempDir;

fn p(value: &str) -> ResourcePath {
    ResourcePath::parse(value).unwrap()
}

fn connector(root: &Path) -> LocalConnector {
    LocalConnector::new(root, LocalOptions::new()).unwrap()
}

fn list(connector: &LocalConnector, dir: &str) -> ListPage {
    connector
        .list(&p(dir), None, &CancellationToken::new())
        .unwrap()
}

fn sorted(page: &ListPage) -> Vec<(String, EntryKind)> {
    let mut entries: Vec<_> = page
        .entries()
        .iter()
        .map(|e| (e.path().as_str().to_owned(), e.kind()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn tree() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::create_dir_all(root.join("docs").join("sub")).unwrap();
    fs::write(root.join("docs").join("b.md"), b"bravo!").unwrap();
    fs::write(root.join("docs").join("sub").join("c.bin"), [7u8; 3000]).unwrap();
    fs::create_dir(root.join("vide")).unwrap();
    fs::write(root.join("café.txt"), b"nfc").unwrap();
    fs::write(root.join("日本語.txt"), b"jp").unwrap();
    dir
}

#[test]
fn lists_children_with_metadata() {
    let dir = tree();
    let connector = connector(dir.path());
    let page = list(&connector, "");
    assert!(page.next_page().is_none());
    assert!(page.issues().is_empty(), "{:?}", page.issues());
    assert_eq!(
        sorted(&page),
        [
            ("a.txt".to_owned(), EntryKind::File),
            ("café.txt".to_owned(), EntryKind::File),
            ("docs".to_owned(), EntryKind::Directory),
            ("vide".to_owned(), EntryKind::Directory),
            ("日本語.txt".to_owned(), EntryKind::File),
        ]
    );
    let a = page
        .entries()
        .iter()
        .find(|e| e.path().as_str() == "a.txt")
        .unwrap();
    assert_eq!(a.metadata().size(), Some(5));
    assert!(a.metadata().modified().is_some());
    assert_ne!(
        a.metadata().modified_precision(),
        TimestampPrecision::Unknown
    );
    // Pas de taille rapportée pour un répertoire.
    let docs = page
        .entries()
        .iter()
        .find(|e| e.path().as_str() == "docs")
        .unwrap();
    assert_eq!(docs.metadata().size(), None);

    assert_eq!(
        sorted(&list(&connector, "docs")),
        [
            ("docs/b.md".to_owned(), EntryKind::File),
            ("docs/sub".to_owned(), EntryKind::Directory)
        ]
    );
    assert!(list(&connector, "vide").entries().is_empty());
}

#[test]
fn stat_and_missing_entries() {
    let dir = tree();
    let connector = connector(dir.path());
    let cancel = CancellationToken::new();
    let entry = connector.stat(&p("docs/sub/c.bin"), &cancel).unwrap();
    assert_eq!(entry.kind(), EntryKind::File);
    assert_eq!(entry.metadata().size(), Some(3000));
    assert_eq!(
        connector
            .stat(&ResourcePath::root(), &cancel)
            .unwrap()
            .kind(),
        EntryKind::Directory
    );
    let error = connector.stat(&p("absent.txt"), &cancel).unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotFound);
    assert_eq!(error.path(), Some(&p("absent.txt")));
    // Un ancêtre qui est un fichier : introuvable, jamais une lecture ailleurs.
    let error = connector.stat(&p("a.txt/x"), &cancel).unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotFound);
}

#[test]
fn listing_a_missing_directory_is_an_error_not_an_empty_page() {
    let dir = tree();
    let connector = connector(dir.path());
    let error = connector
        .list(&p("absent"), None, &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::NotFound);
    let error = connector
        .list(&p("a.txt"), None, &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Permanent);
}

#[test]
fn missing_root_is_disconnected() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("montage-absent");
    let connector = connector(&root);
    let error = connector
        .list(&ResourcePath::root(), None, &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Disconnected);
    let error = connector
        .stat(&p("x"), &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Disconnected);
}

#[test]
fn root_must_be_a_directory() {
    let dir = tree();
    let connector = connector(&dir.path().join("a.txt"));
    let error = connector
        .list(&ResourcePath::root(), None, &CancellationToken::new())
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Permanent);
}

#[test]
fn reads_content_in_chunks() {
    let dir = tree();
    let connector = connector(dir.path());
    let mut reader = connector
        .open_read(&p("docs/sub/c.bin"), &CancellationToken::new())
        .unwrap();
    let mut buf = [0u8; 1024];
    let mut total = Vec::new();
    loop {
        let read = reader.read(&mut buf).unwrap();
        if read == 0 {
            break;
        }
        assert!(read <= 1024);
        total.extend_from_slice(&buf[..read]);
    }
    assert_eq!(total, vec![7u8; 3000]);
}

#[test]
fn reading_a_directory_or_root_is_refused() {
    let dir = tree();
    let connector = connector(dir.path());
    let cancel = CancellationToken::new();
    assert!(connector.open_read(&p("docs"), &cancel).is_err());
    assert!(connector.open_read(&ResourcePath::root(), &cancel).is_err());
    assert_eq!(
        connector
            .open_read(&p("absent"), &cancel)
            .err()
            .unwrap()
            .kind(),
        StorageErrorKind::NotFound
    );
}

#[test]
fn cancellation_is_honoured() {
    let dir = tree();
    let connector = connector(dir.path());
    let cancel = CancellationToken::new();
    let mut reader = connector.open_read(&p("a.txt"), &cancel).unwrap();
    cancel.cancel();
    let mut buf = [0u8; 8];
    assert_eq!(
        reader.read(&mut buf).unwrap_err().kind(),
        StorageErrorKind::Cancelled
    );
    assert_eq!(
        connector
            .list(&ResourcePath::root(), None, &cancel)
            .unwrap_err()
            .kind(),
        StorageErrorKind::Cancelled
    );
}

#[test]
fn forged_page_tokens_are_refused() {
    let dir = tree();
    let connector = connector(dir.path());
    let error = connector
        .list(
            &ResourcePath::root(),
            Some(&PageToken::new("1")),
            &CancellationToken::new(),
        )
        .unwrap_err();
    assert_eq!(error.kind(), StorageErrorKind::Conflict);
}

#[test]
fn connector_never_mutates_the_tree() {
    let dir = tree();
    // Fichiers : nom, taille, date (lues par `fs::metadata`, sans cache).
    // Répertoires : nom seulement. Sous NTFS, la date d'un répertoire vue par
    // l'énumération de son parent est une copie mise à jour paresseusement :
    // elle peut changer après une simple lecture, sans aucune écriture
    // (constaté sous Windows 11 lors de la mise au point de ce test).
    let snapshot = |root: &Path| -> Vec<(String, Option<(u64, std::time::SystemTime)>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(current) = stack.pop() {
            for item in fs::read_dir(&current).unwrap() {
                let item = item.unwrap();
                let meta = fs::metadata(item.path()).unwrap();
                let observed = meta
                    .is_file()
                    .then(|| (meta.len(), meta.modified().unwrap()));
                out.push((item.path().display().to_string(), observed));
                if meta.is_dir() {
                    stack.push(item.path());
                }
            }
        }
        out.sort();
        out
    };
    let before = snapshot(dir.path());
    let connector = connector(dir.path());
    let cancel = CancellationToken::new();
    for d in ["", "docs", "docs/sub", "vide"] {
        let _page: ListPage = list(&connector, d);
    }
    let _entry: FileEntry = connector.stat(&p("a.txt"), &cancel).unwrap();
    let mut reader = connector.open_read(&p("docs/b.md"), &cancel).unwrap();
    let mut buf = [0u8; 64];
    while reader.read(&mut buf).unwrap() > 0 {}
    drop(reader);
    assert_eq!(snapshot(dir.path()), before);
}

/// Crée un lien vers un répertoire **hors** de la racine scannée.
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
fn symlinks_and_junctions_are_reported_and_never_followed() {
    let dir = tree();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), b"hors racine").unwrap();
    link_dir(outside.path(), &dir.path().join("lien"));

    let connector = connector(dir.path());
    let cancel = CancellationToken::new();
    let page = list(&connector, "");
    let link = page
        .entries()
        .iter()
        .find(|e| e.path().as_str() == "lien")
        .unwrap();
    assert_eq!(link.kind(), EntryKind::Symlink);
    assert_eq!(
        connector.stat(&p("lien"), &cancel).unwrap().kind(),
        EntryKind::Symlink
    );
    // Ni listing, ni stat, ni lecture à travers le lien.
    assert!(connector.list(&p("lien"), None, &cancel).is_err());
    assert!(connector.stat(&p("lien/secret.txt"), &cancel).is_err());
    assert!(connector.open_read(&p("lien/secret.txt"), &cancel).is_err());
    assert!(connector.open_read(&p("lien"), &cancel).is_err());
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_reported_not_dropped() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let dir = tree();
    fs::write(dir.path().join(OsStr::from_bytes(b"bad\xffname")), b"x").unwrap();
    let page = list(&connector(dir.path()), "");
    assert_eq!(page.issues().len(), 1);
    assert_eq!(
        page.issues()[0].kind(),
        &ListingIssueKind::UnrepresentableName(NameIssue::NonUtf8)
    );
    assert!(page.issues()[0].omits_entry());
}

#[cfg(windows)]
#[test]
fn non_utf8_names_are_reported_not_dropped() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    let dir = tree();
    // Surrogate UTF-16 isolé : nom NTFS valide, non représentable en UTF-8.
    let name = OsString::from_wide(&[0x0062, 0xD800, 0x0078]);
    fs::write(dir.path().join(name), b"x").unwrap();
    for connector in [connector(dir.path()), reusing(dir.path())] {
        let page = list(&connector, "");
        assert_eq!(page.issues().len(), 1);
        assert_eq!(
            page.issues()[0].kind(),
            &ListingIssueKind::UnrepresentableName(NameIssue::NonUtf8)
        );
    }
}

#[cfg(unix)]
#[test]
fn backslash_in_unix_name_is_reported() {
    let dir = tree();
    fs::write(dir.path().join("a\\b"), b"x").unwrap();
    let page = list(&connector(dir.path()), "");
    assert!(matches!(
        page.issues()[0].kind(),
        ListingIssueKind::UnrepresentableName(NameIssue::Invalid(_))
    ));
}

#[cfg(windows)]
#[test]
fn win32_ambiguous_names_are_reported_not_dropped() {
    let dir = tree();
    // Le préfixe verbatim contourne la normalisation Win32 et crée « fin. ».
    let verbatim = format!(r"\\?\{}\fin.", dir.path().display());
    fs::write(&verbatim, b"x").unwrap();
    for connector in [connector(dir.path()), reusing(dir.path())] {
        let page = list(&connector, "");
        assert_eq!(page.issues().len(), 1);
        assert_eq!(
            page.issues()[0].kind(),
            &ListingIssueKind::UnrepresentableName(NameIssue::PlatformAmbiguous)
        );
        // Win32 would normalize this logical name to a different name.
        assert_eq!(
            connector
                .stat(&p("fin."), &CancellationToken::new())
                .unwrap_err()
                .kind(),
            StorageErrorKind::NotSupported
        );
    }
    fs::remove_file(&verbatim).unwrap();
}

/// Retire temporairement le droit de lister un répertoire, rétabli au drop.
struct DenyListing {
    path: std::path::PathBuf,
}

impl DenyListing {
    fn new(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
            // Sous root, les permissions ne s'appliquent pas : test sans objet.
            if fs::read_dir(path).is_ok() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
                return None;
            }
        }
        #[cfg(windows)]
        {
            // Refus explicite de « lister le dossier » pour Tout le monde (S-1-1-0).
            let output = std::process::Command::new("icacls")
                .arg(path)
                .args(["/deny", "*S-1-1-0:(RD)"])
                .output()
                .unwrap();
            assert!(output.status.success(), "icacls /deny : {output:?}");
            // Un administrateur disposant du privilège de sauvegarde (runners GitHub)
            // passe outre l'ACL : `FindFirstFileExW` ouvre avec l'intention de
            // sauvegarde. Le refus n'est alors pas observable : test sans objet.
            if fs::read_dir(path).is_ok() {
                restore_listing(path).unwrap();
                return None;
            }
        }
        Some(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for DenyListing {
    #[expect(
        clippy::print_stderr,
        reason = "un Drop ne peut pas renvoyer d'erreur : l'échec est signalé explicitement"
    )]
    fn drop(&mut self) {
        if let Err(error) = restore_listing(&self.path) {
            eprintln!(
                "ATTENTION : droits non rétablis sur {} : {error}",
                self.path.display()
            );
        }
    }
}

/// Rétablit le droit de lister `path`.
fn restore_listing(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("icacls")
            .arg(path)
            .args(["/remove:d", "*S-1-1-0"])
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "icacls : {:?}",
                output.status
            )));
        }
    }
    Ok(())
}

#[test]
fn access_denied_is_reported_as_such() {
    let dir = tree();
    let Some(_guard) = DenyListing::new(&dir.path().join("docs")) else {
        eprintln!(
            "NON EXÉCUTÉ : permissions non applicables (root, ou administrateur Windows \
             avec privilège de sauvegarde)"
        );
        return;
    };
    for connector in [connector(dir.path()), reusing(dir.path())] {
        let error = connector
            .list(&p("docs"), None, &CancellationToken::new())
            .unwrap_err();
        assert_eq!(error.kind(), StorageErrorKind::PermissionDenied);
        assert!(!list(&connector, "").entries().is_empty());
    }
}

#[test]
fn paginates_without_reusing_or_crossing_cursors() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    for n in 0..600 {
        fs::write(temp.path().join(format!("entry-{n}")), b"fixture")?;
    }
    let first = connector(temp.path());
    let other = connector(temp.path());
    let cancel = CancellationToken::new();
    let root = ResourcePath::root();
    let a = first.list(&root, None, &cancel)?;
    assert_eq!(a.entries().len(), 256);
    let token = a.next_page().ok_or("cursor missing")?.clone();
    assert!(other.list(&root, Some(&token), &cancel).is_err());
    let b = first.list(&root, Some(&token), &cancel)?;
    assert_eq!(b.entries().len(), 256);
    assert!(first.list(&root, Some(&token), &cancel).is_err());
    let c = first.list(&root, b.next_page(), &cancel)?;
    assert_eq!(c.entries().len(), 88);
    assert!(c.next_page().is_none());
    Ok(())
}

fn reusing(root: &Path) -> LocalConnector {
    LocalConnector::new(root, LocalOptions::new().with_handle_reuse()).unwrap()
}

fn read_all(connector: &LocalConnector, at: &str) -> Vec<u8> {
    let mut reader = connector
        .open_read(&p(at), &CancellationToken::new())
        .unwrap();
    let (mut content, mut buf) = (Vec::new(), [0u8; 512]);
    loop {
        let read = reader.read(&mut buf).unwrap();
        if read == 0 {
            return content;
        }
        content.extend_from_slice(&buf[..read]);
    }
}

#[test]
fn handle_reuse_observes_exactly_what_a_full_walk_observes() {
    let dir = tree();
    let (plain, reuse) = (connector(dir.path()), reusing(dir.path()));
    // Twice: the second pass goes through cached handles.
    for _ in 0..2 {
        for at in ["", "docs", "docs/sub", "vide", "docs/sub", "docs"] {
            assert_eq!(sorted(&list(&plain, at)), sorted(&list(&reuse, at)), "{at}");
        }
        for at in ["a.txt", "docs/b.md", "docs/sub/c.bin", "docs/sub", "vide"] {
            let cancel = CancellationToken::new();
            assert_eq!(
                plain.stat(&p(at), &cancel).unwrap(),
                reuse.stat(&p(at), &cancel).unwrap()
            );
        }
        assert_eq!(read_all(&reuse, "docs/sub/c.bin"), [7u8; 3000]);
        assert_eq!(read_all(&reuse, "docs/b.md"), b"bravo!");
    }
    let cancel = CancellationToken::new();
    for missing in ["absent.txt", "docs/absent", "absent/x"] {
        assert_eq!(
            reuse.stat(&p(missing), &cancel).unwrap_err().kind(),
            StorageErrorKind::NotFound,
            "{missing}"
        );
    }
    // A file is never a directory, cached or not.
    assert_eq!(
        reuse.list(&p("a.txt"), None, &cancel).unwrap_err().kind(),
        StorageErrorKind::Permanent
    );
}

#[test]
fn handle_reuse_never_traverses_a_link() {
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), b"hors racine").unwrap();
    let dir = tree();
    link_dir(outside.path(), &dir.path().join("lien"));
    let reuse = reusing(dir.path());
    let cancel = CancellationToken::new();
    let root: Vec<_> = sorted(&list(&reuse, ""));
    assert!(root.contains(&("lien".to_owned(), EntryKind::Symlink)));
    for _ in 0..2 {
        assert_eq!(
            reuse.list(&p("lien"), None, &cancel).unwrap_err().kind(),
            StorageErrorKind::Permanent
        );
        assert!(reuse.stat(&p("lien/secret.txt"), &cancel).is_err());
        assert!(reuse.open_read(&p("lien/secret.txt"), &cancel).is_err());
    }
}

#[test]
fn handle_reuse_is_ignored_when_mutations_are_enabled() {
    let dir = tree();
    let writable = LocalConnector::new(
        dir.path(),
        LocalOptions::new().with_mutations().with_handle_reuse(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    list(&writable, "docs/sub");
    assert!(writable.stat(&p("docs/b.md"), &cancel).is_ok());
    // No handle is kept: the directory can be renamed (Windows refuses to
    // rename a directory with an open handle) and the old path is gone.
    fs::rename(dir.path().join("docs"), dir.path().join("moved")).unwrap();
    assert_eq!(
        writable.stat(&p("docs/b.md"), &cancel).unwrap_err().kind(),
        StorageErrorKind::NotFound
    );
    assert!(writable.stat(&p("moved/b.md"), &cancel).is_ok());
}

#[cfg(windows)]
#[test]
fn bulk_enumeration_preserves_cached_file_facts_after_the_name_disappears()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    let file = temp.path().join("été.txt");
    fs::write(&file, b"fixture")?;
    let dir = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())?;
    let mut reader = cap_primitives::windows_listing::HandleReadDir::new(dir.into_std_file())?;
    assert!(
        reader.uses_handle_batches(),
        "fixture must exercise the handle API"
    );
    let entry = reader.next().ok_or("missing entry")??;
    fs::remove_file(file)?;
    assert_eq!(entry.file_name(), std::ffi::OsStr::new("été.txt"));
    assert_eq!(entry.metadata().len(), 7);
    assert!(entry.metadata().is_file());
    assert!(reader.next().is_none());
    Ok(())
}

#[cfg(windows)]
#[test]
fn handle_batches_span_multiple_native_buffers_and_restart()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = TempDir::new()?;
    for n in 0_i32..1500 {
        fs::write(
            temp.path().join(format!("f{n:04}-été.txt")),
            n.to_le_bytes(),
        )?;
    }
    let dir = cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())?;
    for _ in 0..2 {
        let reader =
            cap_primitives::windows_listing::HandleReadDir::new(dir.try_clone()?.into_std_file())?;
        assert!(reader.uses_handle_batches());
        let entries = reader.collect::<Result<Vec<_>, _>>()?;
        assert_eq!(entries.len(), 1500);
        let mut names = std::collections::BTreeSet::new();
        for entry in entries {
            assert!(names.insert(entry.file_name()));
            assert_eq!(entry.metadata().len(), std::mem::size_of::<i32>() as u64);
        }
    }
    Ok(())
}
