//! Exécution réelle sur le système de fichiers local, uniquement dans des
//! répertoires temporaires isolés (`tempfile::TempDir`).

#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::*;
use connector_local::{LocalConnector, LocalOptions, is_staging_name};
use sync_core::{CancellationToken, ExecutionId, ExecutionStatus, Timestamp};
use sync_executor::{ExecutionRequest, Executor, PlanExecutor, Refusal};
use sync_filter::{Filter, FilterRules};
use sync_journal::{FileJournal, MemoryJournal, read_journal};
use sync_plan::{ExecutionPlan, SyncMode};
use tempfile::TempDir;

fn write(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn read_only(root: &Path) -> LocalConnector {
    LocalConnector::new(root, LocalOptions::new()).unwrap()
}

fn writable(root: &Path) -> LocalConnector {
    LocalConnector::new(root, LocalOptions::new().with_mutations()).unwrap()
}

/// Arbre relatif (fichiers avec contenu, dossiers suffixés de `/`).
fn tree(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                out.push(format!("{relative}/"));
                walk(root, &path, out);
            } else {
                let content = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
                out.push(format!("{relative}={content}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// Plan avec empreintes BLAKE3, politique par défaut du mode (miroir :
/// suppressions autorisées sans plafond relatif).
fn plan_now(
    source: &LocalConnector,
    destination: &LocalConnector,
    mode: SyncMode,
    filter: &Filter,
) -> ExecutionPlan {
    let policy = match mode {
        SyncMode::Mirror => mirror_policy(),
        _ => sync_backup::BackupStrategy::default_policy(),
    };
    plan_with(source, destination, mode, &policy, filter, true)
}

fn run(
    plan: &ExecutionPlan,
    source: &LocalConnector,
    destination: &LocalConnector,
    journal: &mut dyn sync_journal::JournalSink,
) -> sync_executor::ExecutionReport {
    Executor::new()
        .execute(
            ExecutionRequest::new(
                plan,
                source,
                destination,
                ExecutionId::new("exec-local").unwrap(),
                journal,
                &CancellationToken::new(),
            )
            .with_options(confirmed())
            .at(now()),
        )
        .unwrap()
}

fn set_mtime(path: &Path, secs: u64) {
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn backup_on_disk_is_complete_verified_and_idempotent() {
    let source_dir = TempDir::new().unwrap();
    let destination_dir = TempDir::new().unwrap();
    write(source_dir.path(), "a.txt", b"alpha");
    write(source_dir.path(), "docs/sous/b.txt", b"bravo");
    write(source_dir.path(), "docs/café.md", "été".as_bytes());
    fs::create_dir(source_dir.path().join("vide")).unwrap();
    for file in ["a.txt", "docs/sous/b.txt", "docs/café.md"] {
        set_mtime(&source_dir.path().join(file), 1_700_000_000);
    }
    let source = read_only(source_dir.path());
    let destination = writable(destination_dir.path());
    let plan = plan_now(
        &source,
        &destination,
        SyncMode::Backup,
        &Filter::include_all(),
    );
    let journal_dir = TempDir::new().unwrap();
    let journal_path = journal_dir.path().join("exec.jsonl");
    let mut journal = FileJournal::create(&journal_path).unwrap();
    let report = run(&plan, &source, &destination, &mut journal);
    drop(journal);

    assert_eq!(report.status(), ExecutionStatus::Succeeded, "{report:#?}");
    assert_eq!(tree(destination_dir.path()), tree(source_dir.path()));
    assert_eq!(
        fs::metadata(destination_dir.path().join("docs/sous/b.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    );
    let recovery = read_journal(&journal_path).unwrap().recovery();
    assert_eq!(recovery.finished, Some(ExecutionStatus::Succeeded));
    assert_eq!(recovery.totals.done, report.totals().done);

    // Rejouer : empreintes et dates identiques, plus rien à faire.
    let again = plan_now(
        &source,
        &destination,
        SyncMode::Backup,
        &Filter::include_all(),
    );
    assert_eq!(again.summary().mutations(), 0);
    assert_eq!(again.summary().conflict, 0);
}

#[test]
fn file_modified_between_preview_and_execution_is_not_copied() {
    let source_dir = TempDir::new().unwrap();
    let destination_dir = TempDir::new().unwrap();
    write(source_dir.path(), "a.txt", b"version vue en simulation");
    write(source_dir.path(), "b.txt", b"b");
    let source = read_only(source_dir.path());
    let destination = writable(destination_dir.path());
    let plan = plan_now(
        &source,
        &destination,
        SyncMode::Backup,
        &Filter::include_all(),
    );
    write(source_dir.path(), "a.txt", b"modifie apres la simulation !");
    let report = run(&plan, &source, &destination, &mut MemoryJournal::new());
    assert!(matches!(
        refusal(&report, "a.txt"),
        Refusal::SourceChanged { .. }
    ));
    assert_eq!(tree(destination_dir.path()), ["b.txt=b"]);
}

#[test]
fn mirror_on_disk_replaces_deletes_and_keeps_excluded_content() {
    let source_dir = TempDir::new().unwrap();
    let destination_dir = TempDir::new().unwrap();
    write(source_dir.path(), "a.txt", b"nouveau");
    write(destination_dir.path(), "a.txt", b"ancien contenu");
    set_mtime(&destination_dir.path().join("a.txt"), 1_600_000_000);
    write(destination_dir.path(), "obsolete.txt", b"o");
    write(destination_dir.path(), "vieux/x.txt", b"x");
    write(destination_dir.path(), "vieux/notes.keep", b"k");
    write(destination_dir.path(), "parti/y.txt", b"y");
    let source = read_only(source_dir.path());
    let destination = writable(destination_dir.path());
    let filter = Filter::new(FilterRules::new().exclude_file("*.keep")).unwrap();
    let plan = plan_now(&source, &destination, SyncMode::Mirror, &filter);
    let report = run(&plan, &source, &destination, &mut MemoryJournal::new());

    assert_eq!(refusal(&report, "vieux"), Refusal::DirectoryNotEmpty);
    assert!(is_done(&report, "parti"));
    assert_eq!(
        tree(destination_dir.path()),
        ["a.txt=nouveau", "vieux/", "vieux/notes.keep=k"]
    );
    assert!(
        fs::read_dir(destination_dir.path())
            .unwrap()
            .all(|e| !is_staging_name(&e.unwrap().file_name().to_string_lossy()))
    );
    assert_eq!(report.status(), ExecutionStatus::SucceededWithWarnings);
}

#[test]
fn missing_source_root_at_execution_mutates_nothing() {
    // Given un plan miroir avec suppressions ; When le dossier source a
    // disparu (support débranché) ; Then refus avant toute mutation.
    let parent = TempDir::new().unwrap();
    let source_root = parent.path().join("source");
    fs::create_dir(&source_root).unwrap();
    write(&source_root, "a.txt", b"a");
    let destination_dir = TempDir::new().unwrap();
    write(destination_dir.path(), "obsolete.txt", b"o");
    let source = read_only(&source_root);
    let destination = writable(destination_dir.path());
    let plan = plan_now(
        &source,
        &destination,
        SyncMode::Mirror,
        &Filter::include_all(),
    );
    assert_eq!(plan.summary().delete, 1);
    // Windows directory handles now pin the granted root against replacement.
    // Model disconnection between preview and reopening for execution, as the
    // facade does; the unavailable-source deletion safeguard remains exercised.
    drop(source);
    fs::remove_dir_all(&source_root).unwrap();
    let source = read_only(&source_root);
    let result = Executor::new().execute(
        ExecutionRequest::new(
            &plan,
            &source,
            &destination,
            ExecutionId::new("exec-local").unwrap(),
            &mut MemoryJournal::new(),
            &CancellationToken::new(),
        )
        .with_options(confirmed())
        .at(now()),
    );
    assert!(matches!(
        result,
        Err(sync_executor::ExecutionError::EndpointUnavailable { .. })
    ));
    assert_eq!(tree(destination_dir.path()), ["obsolete.txt=o"]);
}

#[test]
fn interrupted_stages_are_neither_copied_nor_deleted_by_mirror() {
    let source_dir = TempDir::new().unwrap();
    let destination_dir = TempDir::new().unwrap();
    write(source_dir.path(), "data", b"valid");
    write(
        source_dir.path(),
        ".syncengine-source.partial",
        b"incomplete source",
    );
    write(
        destination_dir.path(),
        ".syncengine-recovery.partial/previous",
        b"recoverable original",
    );
    let source = read_only(source_dir.path());
    let destination = writable(destination_dir.path());
    let plan = plan_now(
        &source,
        &destination,
        SyncMode::Mirror,
        &Filter::include_all(),
    );
    assert_eq!(plan.summary().delete, 0);
    let report = Executor::new()
        .execute(
            ExecutionRequest::new(
                &plan,
                &source,
                &destination,
                ExecutionId::new("recovery-stage").unwrap(),
                &mut MemoryJournal::new(),
                &CancellationToken::new(),
            )
            .with_options(confirmed())
            .at(now()),
        )
        .unwrap();
    assert_eq!(report.status(), ExecutionStatus::Succeeded);
    assert!(
        !destination_dir
            .path()
            .join(".syncengine-source.partial")
            .exists()
    );
    assert_eq!(
        fs::read(
            destination_dir
                .path()
                .join(".syncengine-recovery.partial/previous")
        )
        .unwrap(),
        b"recoverable original"
    );
    assert_eq!(
        fs::read(destination_dir.path().join("data")).unwrap(),
        b"valid"
    );
}

#[test]
fn plan_age_is_checked_against_the_given_instant() {
    let source_dir = TempDir::new().unwrap();
    let destination_dir = TempDir::new().unwrap();
    write(source_dir.path(), "a.txt", b"a");
    let source = read_only(source_dir.path());
    let destination = writable(destination_dir.path());
    let plan = plan_now(
        &source,
        &destination,
        SyncMode::Backup,
        &Filter::include_all(),
    );
    let later = Timestamp::new(NOW + 3601, 0).unwrap();
    let result = Executor::new().execute(
        ExecutionRequest::new(
            &plan,
            &source,
            &destination,
            ExecutionId::new("exec-local").unwrap(),
            &mut MemoryJournal::new(),
            &CancellationToken::new(),
        )
        .at(later),
    );
    assert!(matches!(
        result,
        Err(sync_executor::ExecutionError::PlanExpired { .. })
    ));
    assert!(tree(destination_dir.path()).is_empty());
}

#[cfg(unix)]
mod permissions {
    //! M2 — permissions POSIX (sécurité : confidentialité des sauvegardes).
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
    }

    fn chmod(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn backup_keeps_private_files_private_and_scripts_executable() {
        // Étant donné un fichier source 0600, un script 0755 et un dossier
        // 0700, quand ils sont sauvegardés vers une destination locale,
        // alors leurs modes sont reproduits (jamais lisibles par le groupe ou
        // les autres pour les éléments privés).
        let source_dir = TempDir::new().unwrap();
        let destination_dir = TempDir::new().unwrap();
        write(source_dir.path(), "prive.md", b"secret");
        write(source_dir.path(), "script.sh", b"#!/bin/sh\n");
        write(source_dir.path(), "coffre/cle.txt", b"cle");
        write(source_dir.path(), "public/lisez.txt", b"ok");
        chmod(&source_dir.path().join("prive.md"), 0o600);
        chmod(&source_dir.path().join("script.sh"), 0o755);
        chmod(&source_dir.path().join("coffre/cle.txt"), 0o640);
        chmod(&source_dir.path().join("coffre"), 0o700);
        chmod(&source_dir.path().join("public"), 0o755);
        let source = read_only(source_dir.path());
        let destination = writable(destination_dir.path());
        let plan = plan_now(
            &source,
            &destination,
            SyncMode::Backup,
            &Filter::include_all(),
        );
        let report = run(&plan, &source, &destination, &mut MemoryJournal::new());
        assert_eq!(report.status(), ExecutionStatus::Succeeded);
        let out = destination_dir.path();
        assert_eq!(mode(&out.join("prive.md")), 0o600);
        assert_eq!(mode(&out.join("script.sh")), 0o755);
        assert_eq!(mode(&out.join("coffre")), 0o700);
        assert_eq!(mode(&out.join("coffre/cle.txt")), 0o640);
        assert_eq!(mode(&out.join("public")), 0o755);
    }

    #[test]
    fn replacing_never_widens_the_rights_of_the_existing_file() {
        // Étant donné une destination 0600 et une source 0644 plus récente,
        // quand la destination est remplacée, alors elle reste 0600.
        let source_dir = TempDir::new().unwrap();
        let destination_dir = TempDir::new().unwrap();
        write(source_dir.path(), "f.txt", b"nouveau contenu");
        write(destination_dir.path(), "f.txt", b"ancien");
        set_mtime(&destination_dir.path().join("f.txt"), 1_600_000_000);
        set_mtime(&source_dir.path().join("f.txt"), 1_700_000_000);
        chmod(&source_dir.path().join("f.txt"), 0o644);
        chmod(&destination_dir.path().join("f.txt"), 0o600);
        let source = read_only(source_dir.path());
        let destination = writable(destination_dir.path());
        let plan = plan_now(
            &source,
            &destination,
            SyncMode::Mirror,
            &Filter::include_all(),
        );
        let report = run(&plan, &source, &destination, &mut MemoryJournal::new());
        assert_eq!(report.status(), ExecutionStatus::Succeeded, "{report:?}");
        let published = destination_dir.path().join("f.txt");
        assert_eq!(fs::read(&published).unwrap(), b"nouveau contenu");
        assert_eq!(mode(&published), 0o600);
    }

    #[test]
    fn the_preserved_version_keeps_the_private_mode() {
        // Étant donné une destination 0600 remplacée avec conservation de
        // version, alors la copie archivée reste 0600 (jamais 0644).
        let source_dir = TempDir::new().unwrap();
        let destination_dir = TempDir::new().unwrap();
        let versions_dir = TempDir::new().unwrap();
        write(source_dir.path(), "f.txt", b"nouveau contenu");
        write(destination_dir.path(), "f.txt", b"ancien");
        set_mtime(&destination_dir.path().join("f.txt"), 1_600_000_000);
        set_mtime(&source_dir.path().join("f.txt"), 1_700_000_000);
        chmod(&destination_dir.path().join("f.txt"), 0o600);
        let source = read_only(source_dir.path());
        let destination = writable(destination_dir.path());
        let versions = writable(versions_dir.path());
        let store = sync_versioning::VersionStore::new(&versions, p("exec")).unwrap();
        let plan = plan_now(
            &source,
            &destination,
            SyncMode::Mirror,
            &Filter::include_all(),
        );
        let report = execute_with_versions(
            &plan,
            &source,
            &destination,
            &store,
            &mut MemoryJournal::new(),
        )
        .unwrap();
        assert_eq!(report.status(), ExecutionStatus::Succeeded, "{report:?}");
        let archived = versions_dir.path().join("exec/f.txt");
        assert_eq!(fs::read(&archived).unwrap(), b"ancien");
        assert_eq!(mode(&archived), 0o600);
    }
}
