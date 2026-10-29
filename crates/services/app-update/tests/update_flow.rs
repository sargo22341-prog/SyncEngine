//! End-to-end update flow against a local HTTP server, in isolated temporary
//! folders: no real network, no GitHub account, no personal folder.

use app_update::{Check, ErrorKind, Progress, Target, Update, UpdateError, UpdateSource};
use sha2::Digest;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CURRENT: &str = "0.1.8";
const NEW: &str = "0.2.0";

/// One canned answer per path: status, extra headers, body.
type Routes = Arc<Mutex<HashMap<String, (u16, Vec<(String, String)>, Vec<u8>)>>>;

/// Minimal HTTP/1.1 server: one answer per connection, then close.
struct Server {
    base: String,
    routes: Routes,
}

impl Server {
    fn start() -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base = format!("http://{}", listener.local_addr()?);
        let routes: Routes = Arc::default();
        let shared = Arc::clone(&routes);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                });
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = request.split(' ').nth(1).unwrap_or("/").to_owned();
                let answer = shared
                    .lock()
                    .ok()
                    .and_then(|r| r.get(&path).cloned())
                    .unwrap_or((404, Vec::new(), b"{}".to_vec()));
                let (status, headers, body) = answer;
                let mut head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (name, value) in headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                if stream.write_all(head.as_bytes()).is_ok() && stream.write_all(&body).is_ok() {
                    let _ignored = stream.flush();
                }
            }
        });
        Ok(Self { base, routes })
    }

    fn route(&self, path: &str, status: u16, headers: &[(&str, &str)], body: Vec<u8>) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.insert(
                path.to_owned(),
                (
                    status,
                    headers
                        .iter()
                        .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                        .collect(),
                    body,
                ),
            );
        }
    }

    fn source(&self) -> Result<UpdateSource, UpdateError> {
        UpdateSource::loopback(&self.base)
    }

    /// Publishes `release` JSON for a tag with one archive and the body.
    fn publish(&self, tag: &str, target: Target, archive: &[u8], body: &str) {
        let version = tag.trim_start_matches('v');
        let name = target.asset_name(version);
        let json = serde_json::json!({
            "tag_name": tag,
            "published_at": "2026-10-20T08:30:00Z",
            "draft": false,
            "prerelease": false,
            "body": body,
            "assets": [{
                "name": name,
                "browser_download_url": format!("{}/download/{tag}/{name}", self.base),
                "size": archive.len(),
            }],
        });
        self.route("/releases/latest", 200, &[], json.to_string().into_bytes());
        self.route(
            &format!("/download/{tag}/{name}"),
            200,
            &[],
            archive.to_vec(),
        );
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn body_for(target: Target, archive: &[u8]) -> String {
    format!(
        "- Nouveauté\n\n### Empreintes SHA-256\n\n```\n{}  {}\n```\n",
        hex(&sha2::Sha256::digest(archive)),
        target.asset_name(NEW)
    )
}

/// Entries `(name, content)` of an archive; `None` content is a directory.
type Entries = Vec<(String, Option<Vec<u8>>)>;

fn good_entries(target: Target) -> Entries {
    let root = target.archive_root(NEW);
    target
        .members()
        .into_iter()
        .map(|m| {
            (
                format!("{root}/{m}"),
                Some(format!("nouveau {m}").into_bytes()),
            )
        })
        .collect()
}

fn zip_archive(entries: &Entries) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in entries {
        match content {
            Some(bytes) => {
                writer.start_file(name.as_str(), options)?;
                writer.write_all(bytes)?;
            }
            None => writer.add_directory(name.as_str(), options)?,
        }
    }
    Ok(writer.finish()?.into_inner())
}

fn tar_archive(entries: &Entries) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    for (name, content) in entries {
        let mut header = tar::Header::new_gnu();
        let bytes = content.clone().unwrap_or_default();
        header.set_entry_type(if content.is_some() {
            tar::EntryType::Regular
        } else {
            tar::EntryType::Directory
        });
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        // Raw name: the builder's path helpers refuse `..`, a test needs it.
        let field = &mut header.as_old_mut().name;
        field.fill(0);
        field[..name.len()].copy_from_slice(name.as_bytes());
        header.set_cksum();
        builder.append(&header, bytes.as_slice())?;
    }
    Ok(builder.into_inner()?.finish()?)
}

/// Program folder with the current files of `target`.
fn program(target: Target) -> Result<tempfile::TempDir, std::io::Error> {
    let dir = tempfile::tempdir()?;
    for member in target.members() {
        std::fs::write(dir.path().join(&member), format!("ancien {member}"))?;
    }
    Ok(dir)
}

fn assert_untouched(dir: &Path, target: Target) -> TestResult {
    for member in target.members() {
        assert_eq!(
            std::fs::read_to_string(dir.join(&member))?,
            format!("ancien {member}")
        );
    }
    let mut names: Vec<String> = std::fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.sort();
    let mut expected = target.members();
    expected.sort();
    assert_eq!(
        names, expected,
        "aucun fichier temporaire ni .old ne doit rester"
    );
    Ok(())
}

fn available(check: Check) -> Result<Update, Box<dyn std::error::Error>> {
    match check {
        Check::Available(update) => Ok(*update),
        other => Err(format!("mise à jour attendue, obtenu {other:?}").into()),
    }
}

fn kind<T: std::fmt::Debug>(result: Result<T, UpdateError>) -> Option<ErrorKind> {
    result.err().map(|e| e.kind())
}

#[test]
fn versions_equal_older_newer_and_invalid() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    for (tag, newer) in [("v0.1.8", false), ("v0.1.7", false), ("v0.1.10", true)] {
        server.publish(tag, target, b"x", "");
        let check = app_update::check(&source, CURRENT, target)?;
        assert_eq!(matches!(check, Check::Available(_)), newer, "{tag}");
    }
    server.publish("version-3", target, b"x", "");
    assert_eq!(
        kind(app_update::check(&source, CURRENT, target)),
        Some(ErrorKind::InvalidRelease)
    );
    Ok(())
}

#[test]
fn release_metadata_and_platform_availability() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let archive = zip_archive(&good_entries(Target::WINDOWS_X64))?;
    server.publish(
        "v0.2.0",
        Target::WINDOWS_X64,
        &archive,
        &body_for(Target::WINDOWS_X64, &archive),
    );
    let update = available(app_update::check(&source, CURRENT, Target::WINDOWS_X64)?)?;
    assert_eq!(update.version, NEW);
    assert_eq!(update.notes, "- Nouveauté");
    assert_eq!(update.published_at.as_deref(), Some("2026-10-20T08:30:00Z"));
    assert!(update.page_url.ends_with("/releases/tag/v0.2.0"));
    assert!(update.checksum_problem().is_none());
    // Only the Windows archive is published: Linux gets « not available ».
    let linux = app_update::check(&source, CURRENT, Target::LINUX_X64)?;
    assert!(matches!(linux, Check::NoAsset { latest } if latest == NEW));
    Ok(())
}

/// Status, headers, body, expected class.
type HttpCase<'a> = (u16, &'a [(&'a str, &'a str)], &'a [u8], ErrorKind);

#[test]
fn http_failures_are_classified_and_never_up_to_date() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    let cases: [HttpCase<'_>; 5] = [
        (
            403,
            &[("x-ratelimit-remaining", "0")],
            b"{}",
            ErrorKind::RateLimited,
        ),
        (429, &[], b"{}", ErrorKind::RateLimited),
        (404, &[], b"{}", ErrorKind::NoRelease),
        (500, &[], b"{}", ErrorKind::Http),
        (200, &[], b"<html>", ErrorKind::InvalidRelease),
    ];
    for (status, headers, body, expected) in cases {
        server.route("/releases/latest", status, headers, body.to_vec());
        assert_eq!(
            kind(app_update::check(&source, CURRENT, target)),
            Some(expected),
            "{status}"
        );
    }
    let draft = serde_json::json!({"tag_name": "v9.0.0", "draft": true, "assets": []});
    server.route("/releases/latest", 200, &[], draft.to_string().into_bytes());
    assert_eq!(
        kind(app_update::check(&source, CURRENT, target)),
        Some(ErrorKind::InvalidRelease)
    );
    // Closed port: offline, distinct from « up to date ».
    let closed = TcpListener::bind("127.0.0.1:0")?;
    let address = closed.local_addr()?;
    drop(closed);
    let offline = UpdateSource::loopback(&format!("http://{address}"))?;
    assert_eq!(
        kind(app_update::check(&offline, CURRENT, target)),
        Some(ErrorKind::Offline)
    );
    assert_eq!(
        kind(UpdateSource::loopback("http://example.com")),
        Some(ErrorKind::Unsupported)
    );
    Ok(())
}

#[test]
fn foreign_download_address_is_refused() -> TestResult {
    let server = Server::start()?;
    let target = Target::WINDOWS_X64;
    let json = serde_json::json!({
        "tag_name": "v0.2.0", "body": "",
        "assets": [{"name": target.asset_name(NEW), "browser_download_url": "http://127.0.0.1:1/x", "size": 1}],
    });
    server.route("/releases/latest", 200, &[], json.to_string().into_bytes());
    assert_eq!(
        kind(app_update::check(&server.source()?, CURRENT, target)),
        Some(ErrorKind::InvalidRelease)
    );
    Ok(())
}

fn full_update(target: Target, archive: Vec<u8>) -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let progress = Progress::default();
    let downloaded = app_update::download(&source, &update, dir.path(), &progress)?;
    assert_eq!(progress.done(), archive.len() as u64);
    assert_eq!(progress.total(), archive.len() as u64);
    let staged = app_update::stage(downloaded, &progress)?;
    assert!(!dir.path().join("syncengine-update.download").exists());
    let installed = app_update::install(staged)?;
    assert_eq!(installed.desktop, dir.path().join(target.desktop_name()));
    assert!(installed.warnings.is_empty());
    for member in target.members() {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&member))?,
            format!("nouveau {member}")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("{member}.old")))?,
            format!("ancien {member}")
        );
    }
    assert!(!dir.path().join("syncengine-update.staging").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |name: &str| -> std::io::Result<u32> {
            Ok(std::fs::metadata(dir.path().join(name))?
                .permissions()
                .mode()
                & 0o777)
        };
        assert_eq!(mode(&target.desktop_name())?, 0o755);
        assert_eq!(mode("LICENSE.txt")?, 0o644);
    }
    // Next start: only the leftovers are removed.
    let report = app_update::cleanup(dir.path(), target);
    assert_eq!(report.removed.len(), 5);
    assert!(report.failed.is_empty());
    let mut names: Vec<String> = std::fs::read_dir(dir.path())?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.sort();
    let mut expected = target.members();
    expected.sort();
    assert_eq!(names, expected);
    Ok(())
}

#[test]
fn zip_update_replaces_every_file_then_cleanup_removes_old_ones() -> TestResult {
    let target = Target::WINDOWS_X64;
    let mut entries = good_entries(target);
    entries.insert(0, (format!("{}/", target.archive_root(NEW)), None));
    full_update(target, zip_archive(&entries)?)
}

#[test]
fn tar_gz_update_replaces_every_file_with_modes() -> TestResult {
    let target = Target::LINUX_X64;
    full_update(target, tar_archive(&good_entries(target))?)
}

#[test]
fn wrong_checksum_replaces_nothing_and_removes_the_download() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    let archive = zip_archive(&good_entries(target))?;
    let mut tampered = archive.clone();
    if let Some(last) = tampered.last_mut() {
        *last ^= 0xff;
    }
    // Published checksum of the genuine archive, tampered bytes served.
    server.publish("v0.2.0", target, &tampered, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let result = app_update::download(&source, &update, dir.path(), &Progress::default());
    assert_eq!(kind(result), Some(ErrorKind::ChecksumMismatch));
    assert_untouched(dir.path(), target)
}

#[test]
fn missing_checksum_refuses_before_downloading() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    let archive = zip_archive(&good_entries(target))?;
    server.publish("v0.2.0", target, &archive, "- Notes sans empreintes");
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    assert!(update.checksum_problem().is_some());
    let dir = program(target)?;
    let result = app_update::download(&source, &update, dir.path(), &Progress::default());
    assert_eq!(kind(result), Some(ErrorKind::ChecksumMissing));
    assert_untouched(dir.path(), target)
}

#[test]
fn cancelled_download_removes_the_temporary_file() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    let archive = zip_archive(&good_entries(target))?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let progress = Progress::default();
    progress.cancel();
    let result = app_update::download(&source, &update, dir.path(), &progress);
    assert_eq!(kind(result), Some(ErrorKind::Cancelled));
    assert_untouched(dir.path(), target)
}

#[test]
fn existing_temporary_file_means_busy_and_is_kept() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    let archive = zip_archive(&good_entries(target))?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let other = dir.path().join("syncengine-update.download");
    std::fs::write(&other, b"autre mise a jour")?;
    let result = app_update::download(&source, &update, dir.path(), &Progress::default());
    assert_eq!(kind(result), Some(ErrorKind::Busy));
    assert_eq!(std::fs::read(&other)?, b"autre mise a jour");
    Ok(())
}

/// Downloads `entries` as an archive and expects a total rejection.
fn rejected(target: Target, entries: &Entries, label: &str) -> TestResult {
    let archive = match target.format() {
        app_update::ArchiveFormat::Zip => zip_archive(entries)?,
        app_update::ArchiveFormat::TarGz => tar_archive(entries)?,
    };
    let server = Server::start()?;
    let source = server.source()?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let progress = Progress::default();
    let downloaded = app_update::download(&source, &update, dir.path(), &progress)?;
    let result = app_update::stage(downloaded, &progress);
    assert_eq!(kind(result), Some(ErrorKind::InvalidArchive), "{label}");
    assert_untouched(dir.path(), target)?;
    assert!(
        !dir.path()
            .parent()
            .unwrap_or(dir.path())
            .join("evil")
            .exists(),
        "{label}"
    );
    Ok(())
}

#[test]
fn malicious_or_incomplete_archives_are_rejected_entirely() -> TestResult {
    for target in [Target::WINDOWS_X64, Target::LINUX_X64] {
        let root = target.archive_root(NEW);
        let good = good_entries(target);
        let with = |extra: (String, Option<Vec<u8>>)| {
            let mut list = good.clone();
            list.push(extra);
            list
        };
        let cases: Vec<(&str, Entries)> = vec![
            (
                "parent",
                with((format!("{root}/../evil"), Some(b"x".to_vec()))),
            ),
            (
                "absolute",
                with(("/tmp/evil".to_owned(), Some(b"x".to_vec()))),
            ),
            (
                "extra",
                with((format!("{root}/autre.dll"), Some(b"x".to_vec()))),
            ),
            (
                "other root",
                with(("evil/syncengine".to_owned(), Some(b"x".to_vec()))),
            ),
            ("subfolder", with((format!("{root}/sous/"), None))),
            ("duplicate", with(good[1].clone())),
            ("missing", good[..4].to_vec()),
            ("backslash", {
                let mut list = good[..4].to_vec();
                list.push((format!("{root}\\LICENSE.txt"), Some(b"x".to_vec())));
                list
            }),
        ];
        for (label, entries) in cases {
            // The zip writer itself refuses to build a repeated name.
            if label == "duplicate" && target.format() == app_update::ArchiveFormat::Zip {
                continue;
            }
            rejected(target, &entries, &format!("{label} ({})", target.triple()))?;
        }
    }
    Ok(())
}

#[test]
fn symbolic_link_in_tar_is_rejected() -> TestResult {
    let target = Target::LINUX_X64;
    let root = target.archive_root(NEW);
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    for (name, content) in good_entries(target) {
        let bytes = content.unwrap_or_default();
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        builder.append_data(&mut header, name, bytes.as_slice())?;
    }
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    builder.append_link(&mut link, format!("{root}/lien"), "/etc/passwd")?;
    let archive = builder.into_inner()?.finish()?;
    let server = Server::start()?;
    let source = server.source()?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let progress = Progress::default();
    let downloaded = app_update::download(&source, &update, dir.path(), &progress)?;
    assert_eq!(
        kind(app_update::stage(downloaded, &progress)),
        Some(ErrorKind::InvalidArchive)
    );
    assert_untouched(dir.path(), target)
}

#[test]
fn failure_on_second_binary_restores_every_file() -> TestResult {
    let server = Server::start()?;
    let source = server.source()?;
    let target = Target::WINDOWS_X64;
    let archive = zip_archive(&good_entries(target))?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    let progress = Progress::default();
    let staged = app_update::stage(
        app_update::download(&source, &update, dir.path(), &progress)?,
        &progress,
    )?;
    // The second executable of the replacement order disappears from staging:
    // its rename into place fails after the first one was replaced.
    let second = &target.members()[1];
    std::fs::remove_file(dir.path().join("syncengine-update.staging").join(second))?;
    let result = app_update::install(staged);
    assert_eq!(kind(result), Some(ErrorKind::ReplaceFailed));
    assert_untouched(dir.path(), target)
}

#[test]
fn leftover_old_file_is_replaced_by_the_next_update() -> TestResult {
    let target = Target::WINDOWS_X64;
    let archive = zip_archive(&good_entries(target))?;
    let server = Server::start()?;
    let source = server.source()?;
    server.publish("v0.2.0", target, &archive, &body_for(target, &archive));
    let update = available(app_update::check(&source, CURRENT, target)?)?;
    let dir = program(target)?;
    std::fs::write(dir.path().join("syncengine.exe.old"), b"tres ancien")?;
    let progress = Progress::default();
    let staged = app_update::stage(
        app_update::download(&source, &update, dir.path(), &progress)?,
        &progress,
    )?;
    app_update::install(staged)?;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("syncengine.exe.old"))?,
        "ancien syncengine.exe"
    );
    Ok(())
}

#[test]
fn cleanup_touches_exact_names_only() -> TestResult {
    let target = Target::WINDOWS_X64;
    let dir = program(target)?;
    let keep = ["notes.old", "syncengine-update.download.bak", "profil.json"];
    for name in keep {
        std::fs::write(dir.path().join(name), b"x")?;
    }
    std::fs::write(dir.path().join("syncengine-desktop.exe.old"), b"x")?;
    std::fs::write(dir.path().join("syncengine-update.download"), b"x")?;
    let staging = dir.path().join("syncengine-update.staging");
    std::fs::create_dir(&staging)?;
    std::fs::write(staging.join("LICENSE.txt"), b"x")?;
    let report = app_update::cleanup(dir.path(), target);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.removed.len(), 3);
    for name in keep {
        assert!(dir.path().join(name).exists(), "{name}");
    }
    // A foreign file inside the staging folder keeps it (never recursive).
    std::fs::create_dir(&staging)?;
    std::fs::write(staging.join("etranger.txt"), b"x")?;
    let report = app_update::cleanup(dir.path(), target);
    assert_eq!(report.failed.len(), 1);
    assert!(staging.join("etranger.txt").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn cleanup_never_follows_links() -> TestResult {
    let target = Target::LINUX_X64;
    let dir = program(target)?;
    let outside = tempfile::tempdir()?;
    std::fs::write(outside.path().join("LICENSE.txt"), b"precieux")?;
    std::os::unix::fs::symlink(outside.path(), dir.path().join("syncengine-update.staging"))?;
    std::os::unix::fs::symlink(
        outside.path().join("LICENSE.txt"),
        dir.path().join("LICENSE.txt.old"),
    )?;
    let report = app_update::cleanup(dir.path(), target);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(
        std::fs::read(outside.path().join("LICENSE.txt"))?,
        b"precieux"
    );
    assert!(std::fs::symlink_metadata(dir.path().join("syncengine-update.staging")).is_err());
    Ok(())
}

#[test]
fn renamed_executable_is_not_supported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let target = Target::WINDOWS_X64;
    let good = dir.path().join("syncengine-desktop.exe");
    assert_eq!(app_update::program_dir(&good, target)?, dir.path());
    let renamed = dir.path().join("SyncEngine-portable.exe");
    assert_eq!(
        kind(app_update::program_dir(&renamed, target)),
        Some(ErrorKind::Unsupported)
    );
    Ok(())
}
