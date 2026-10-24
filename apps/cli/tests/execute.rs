//! Tests de bout en bout de `syncengine execute` et `syncengine journal`,
//! uniquement dans des répertoires temporaires isolés.

#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

mod common;

#[test]
fn facade_lock_blocks_direct_cli_before_journal_or_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(source.join("precious"), b"fixture").unwrap();
    let root = temp.path().join("SyncEngine/job-locks");
    let _owner = sync_executor::execution_lock(&root).unwrap();
    let journal = temp.path().join("blocked.jsonl");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_syncengine"))
        .env("LOCALAPPDATA", temp.path())
        .env("XDG_STATE_HOME", temp.path())
        .arg("execute")
        .arg(&source)
        .arg(&destination)
        .arg("--confirm")
        .arg("--journal")
        .arg(&journal)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("another SyncEngine execution"));
    assert!(!journal.exists());
    assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
}

use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Output;

use common::{DenyListing, snapshot, stdout, syncengine, write};
use tempfile::TempDir;

fn execute(source: &Path, destination: &Path, journal: &Path, extra: &[&str]) -> Output {
    let mut args: Vec<&OsStr> = vec![
        "execute".as_ref(),
        source.as_os_str(),
        destination.as_os_str(),
        "--confirm".as_ref(),
        "--journal".as_ref(),
        journal.as_os_str(),
    ];
    args.extend(extra.iter().map(OsStr::new));
    syncengine(args)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read(path: &Path) -> String {
    String::from_utf8(fs::read(path).unwrap()).unwrap()
}

#[test]
fn execute_without_confirmation_is_a_usage_error_and_touches_nothing() {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    write(source.path(), "a.txt", b"a");
    let output = syncengine([
        OsStr::new("execute"),
        source.path().as_os_str(),
        destination.path().as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--confirm"));
    assert!(fs::read_dir(destination.path()).unwrap().next().is_none());
}

#[test]
fn backup_executes_and_journal_reports_completion() {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    write(source.path(), "a.txt", b"alpha");
    write(source.path(), "docs/b.txt", b"bravo");
    let journal = state.path().join("exec.jsonl");

    let output = execute(source.path(), destination.path(), &journal, &["--progress"]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}\n{}", stderr(&output));
    assert!(text.starts_with("EXÉCUTION RÉELLE"), "{text}");
    assert!(text.contains("COPIÉ      a.txt  (5 o)"), "{text}");
    assert!(text.contains("CRÉÉ       docs/"), "{text}");
    assert!(text.contains("Statut : réussie"), "{text}");
    assert!(stderr(&output).contains("progression : 3/3 opération(s), 10/10 o"));
    assert_eq!(read(&destination.path().join("docs/b.txt")), "bravo");

    let report = syncengine([OsStr::new("journal"), journal.as_os_str()]);
    assert_eq!(report.status.code(), Some(0), "{}", stdout(&report));
    assert!(stdout(&report).contains("terminée — réussie"));

    // Le même journal n'est jamais écrasé.
    let again = execute(source.path(), destination.path(), &journal, &[]);
    assert_eq!(again.status.code(), Some(4));
    assert!(
        stderr(&again).contains("jamais écrasé"),
        "{}",
        stderr(&again)
    );

    // Rejouer avec un nouveau journal : rien à faire (dates conservées).
    let second = state.path().join("exec2.jsonl");
    let output = execute(source.path(), destination.path(), &second, &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["totals"]["done"], 0);
    assert_eq!(json["status"], "succeeded");
}

#[test]
fn mirror_deletions_need_allow_and_confirm_and_keep_versions() {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    let versions = TempDir::new().unwrap();
    write(source.path(), "garde.txt", b"g");
    write(destination.path(), "garde.txt", b"ancienne version");
    write(destination.path(), "obsolete.txt", b"o");
    write(destination.path(), "autre.txt", b"x");
    write(destination.path(), "encore.txt", b"y");

    // Suppressions prévues mais non confirmées : refus, rien n'est modifié.
    let before = snapshot(destination.path());
    let output = execute(
        source.path(),
        destination.path(),
        &state.path().join("1.jsonl"),
        &[
            "--mode",
            "mirror",
            "--allow-delete",
            "--max-delete-percent",
            "100",
        ],
    );
    assert_eq!(output.status.code(), Some(4), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("confirmation explicite"),
        "{}",
        stderr(&output)
    );
    assert_eq!(snapshot(destination.path()), before);
    // Le journal vide de la tentative refusée a été retiré.
    assert!(!state.path().join("1.jsonl").exists());

    let output = execute(
        source.path(),
        destination.path(),
        &state.path().join("2.jsonl"),
        &[
            "--mode",
            "mirror",
            "--allow-delete",
            "--max-delete-percent",
            "100",
            "--confirm-delete",
            "--versions-dir",
            versions.path().to_str().unwrap(),
        ],
    );
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(0), "{text}\n{}", stderr(&output));
    assert!(
        text.contains("SUPPRIMÉ   obsolete.txt  [version conservée"),
        "{text}"
    );
    assert!(
        text.contains("REMPLACÉ   garde.txt  (1 o)  [version conservée"),
        "{text}"
    );
    let remaining: Vec<String> = fs::read_dir(destination.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(remaining, ["garde.txt"]);
    // Versions : un sous-dossier par exécution.
    let runs: Vec<_> = fs::read_dir(versions.path()).unwrap().collect();
    assert_eq!(runs.len(), 1);
    let run = runs.into_iter().next().unwrap().unwrap().path();
    assert_eq!(read(&run.join("garde.txt")), "ancienne version");
    assert_eq!(read(&run.join("obsolete.txt")), "o");
}

#[test]
fn journal_and_versions_inside_an_endpoint_are_refused() {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    write(source.path(), "a.txt", b"a");
    let output = execute(
        source.path(),
        destination.path(),
        &destination.path().join("exec.jsonl"),
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("le journal"),
        "{}",
        stderr(&output)
    );
    let state = TempDir::new().unwrap();
    let output = execute(
        source.path(),
        destination.path(),
        &state.path().join("exec.jsonl"),
        &["--versions-dir", source.path().to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(fs::read_dir(destination.path()).unwrap().next().is_none());

    // Un journal à côté de la destination (dossier parent) est admis.
    let parent = TempDir::new().unwrap();
    let nested_destination = parent.path().join("destination");
    fs::create_dir(&nested_destination).unwrap();
    let output = execute(
        source.path(),
        &nested_destination,
        &parent.path().join("exec.jsonl"),
        &[],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(read(&nested_destination.join("a.txt")), "a");
}

#[test]
fn saved_plan_is_executed_only_for_the_folders_it_names() {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    write(source.path(), "a.txt", b"a");
    let preview = syncengine([
        OsStr::new("preview"),
        source.path().as_os_str(),
        destination.path().as_os_str(),
        OsStr::new("--json"),
    ]);
    let plan = state.path().join("plan.json");
    fs::write(&plan, preview.stdout).unwrap();

    let output = execute(
        source.path(),
        other.path(),
        &state.path().join("1.jsonl"),
        &["--plan", plan.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(4));
    assert!(
        stderr(&output).contains("le plan désigne"),
        "{}",
        stderr(&output)
    );
    assert!(fs::read_dir(other.path()).unwrap().next().is_none());

    let output = execute(
        source.path(),
        destination.path(),
        &state.path().join("2.jsonl"),
        &["--plan", plan.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(read(&destination.path().join("a.txt")), "a");
}

#[test]
fn unreadable_source_folder_means_zero_deletions_on_disk() {
    // Given un sous-dossier source illisible ; When un miroir avec
    // suppressions confirmées est exécuté ; Then aucune suppression.
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    let state = TempDir::new().unwrap();
    write(source.path(), "a.txt", b"a");
    write(source.path(), "prive/secret.txt", b"s");
    write(destination.path(), "prive/secret.txt", b"s");
    write(destination.path(), "prive/ancien.txt", b"vieux");
    write(destination.path(), "obsolete.txt", b"o");
    let Some(_deny) = DenyListing::new(&source.path().join("prive")) else {
        return; // retrait de droits impossible ici (root ou admin Windows) : non exécuté
    };
    let output = execute(
        source.path(),
        destination.path(),
        &state.path().join("exec.jsonl"),
        &[
            "--mode",
            "mirror",
            "--allow-delete",
            "--max-delete-percent",
            "100",
            "--confirm-delete",
        ],
    );
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(3), "{text}\n{}", stderr(&output));
    assert!(!text.contains("SUPPRIMÉ"), "{text}");
    assert!(destination.path().join("obsolete.txt").exists());
    assert!(destination.path().join("prive/ancien.txt").exists());
    assert_eq!(read(&destination.path().join("a.txt")), "a");
}

#[test]
fn interrupted_journal_is_diagnosed() {
    let state = TempDir::new().unwrap();
    let journal = state.path().join("exec.jsonl");
    let lines = [
        r#"{"format_version":1,"sequence":0,"at":{"secs":1,"nanos":0},"record":{"type":"execution_started","execution":"exec-1","plan":"plan-1","profile":null,"operations":2,"mutations":2,"source":"S","destination":"D","versions":null}}"#,
        r#"{"format_version":1,"sequence":1,"at":{"secs":2,"nanos":0},"record":{"type":"operation_started","operation":"op-000001","path":"gros.bin","action":"copy"}}"#,
        r#"{"format_version":1,"sequence":2,"at":{"secs":3,"nanos":0},"record":{"type":"staging_created","operation":"op-000001","staging":".syncengine-1-2-3.partial"}}"#,
    ];
    fs::write(&journal, lines.join("\n") + "\n{\"format_version\":1,\"seq").unwrap();
    let output = syncengine([OsStr::new("journal"), journal.as_os_str()]);
    let text = stdout(&output);
    assert_eq!(output.status.code(), Some(3), "{text}");
    assert!(text.contains("INTERROMPUE"), "{text}");
    assert!(text.contains("EN COURS     COPIER gros.bin"), "{text}");
    assert!(text.contains(".syncengine-1-2-3.partial"), "{text}");
    assert!(text.contains("dernière ligne incomplète"), "{text}");

    fs::write(&journal, "pas du json\n").unwrap();
    let output = syncengine([OsStr::new("journal"), journal.as_os_str()]);
    assert_eq!(output.status.code(), Some(4));
}
