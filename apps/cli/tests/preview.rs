//! Tests de bout en bout de `syncengine preview` sur des répertoires
//! temporaires isolés : connecteur local → scanner → comparaison →
//! stratégie → plan affiché. Aucun fichier ne doit être modifié.
//!
//! Instantanés texte : `tests/snapshots/*.txt`, chemins temporaires
//! remplacés par `<SOURCE>` et `<DESTINATION>`. Régénération après un
//! changement voulu : `SYNCENGINE_UPDATE_SNAPSHOTS=1 cargo test -p syncengine-cli --test preview`
//! (relire le diff).

// Aides de test hors `#[test]` : un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "aides de test : un échec est un échec de test"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use common::{DenyListing, snapshot, stdout, syncengine, write};
use tempfile::TempDir;

const T1: u64 = 1_700_000_000;
const T2: u64 = 1_700_100_000;
const T3: u64 = 1_700_200_000;

/// Écrit un fichier et fixe sa date de modification (instantanés stables).
fn file_at(root: &Path, relative: &str, content: &[u8], secs: u64) {
    write(root, relative, content);
    fs::File::options()
        .write(true)
        .open(root.join(relative))
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

/// Source et destination couvrant copie, remplacement, conservation,
/// suppression (miroir) et conflit (sauvegarde).
fn fixture() -> (TempDir, TempDir) {
    let source = TempDir::new().unwrap();
    let destination = TempDir::new().unwrap();
    let (s, d) = (source.path(), destination.path());
    file_at(s, "commun.txt", b"identique", T1);
    file_at(d, "commun.txt", b"identique", T1);
    file_at(s, "docs/rapport.md", b"version longue", T2);
    file_at(d, "docs/rapport.md", b"courte", T1);
    file_at(d, "docs/notes.txt", b"n", T1);
    file_at(s, "recent.txt", b"source", T1);
    file_at(d, "recent.txt", b"modifie a la destination", T3);
    file_at(s, "nouveau/a.txt", b"a", T2);
    (source, destination)
}

fn preview(source: &Path, destination: &Path, extra: &[&str]) -> std::process::Output {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "preview".as_ref(),
        source.as_os_str(),
        destination.as_os_str(),
    ];
    args.extend(extra.iter().map(|arg| std::ffi::OsStr::new(*arg)));
    syncengine(args)
}

fn normalized(text: &str, source: &Path, destination: &Path) -> String {
    text.replace(&source.display().to_string(), "<SOURCE>")
        .replace(&destination.display().to_string(), "<DESTINATION>")
        .replace("\r\n", "\n")
}

fn assert_snapshot(name: &str, actual: &str) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "snapshots", name]
        .iter()
        .collect();
    if std::env::var_os("SYNCENGINE_UPDATE_SNAPSHOTS").is_some() {
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "instantané {} illisible ({error}) ; le créer avec SYNCENGINE_UPDATE_SNAPSHOTS=1",
            path.display()
        )
    });
    assert_eq!(
        expected.replace("\r\n", "\n"),
        actual,
        "instantané {name} différent"
    );
}

fn trees(source: &Path, destination: &Path) -> String {
    snapshot(source) + "\n--\n" + &snapshot(destination)
}

#[test]
fn backup_preview_shows_copy_replace_keep_and_conflict() {
    let (source, destination) = fixture();
    let (s, d) = (source.path(), destination.path());
    let before = trees(s, d);
    let output = preview(s, d, &[]);
    // Conflit présent ⇒ code 3.
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let text = normalized(&stdout(&output), s, d);
    assert!(
        text.contains("COPIER     nouveau/a.txt  (absent de la destination, 1 o)"),
        "{text}"
    );
    assert!(
        text.contains("REMPLACER  docs/rapport.md  (source plus récente, 14 o)"),
        "{text}"
    );
    assert!(text.contains("CONSERVER  docs/notes.txt"), "{text}");
    assert!(
        text.contains("CONFLIT    recent.txt  (destination plus récente ; rien n'est écrit)"),
        "{text}"
    );
    assert!(!text.contains("SUPPRIMER"), "{text}");
    assert!(
        !text.contains("commun.txt"),
        "équivalents masqués sans --all"
    );
    assert_snapshot("preview_backup.txt", &text);
    assert_eq!(trees(s, d), before, "la prévisualisation ne modifie rien");
}

#[test]
fn mirror_preview_deletes_only_when_explicitly_allowed() {
    let (source, destination) = fixture();
    let (s, d) = (source.path(), destination.path());
    let before = trees(s, d);

    let output = preview(s, d, &["--mode", "mirror"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = stdout(&output);
    assert!(!text.contains("SUPPRIMER"), "{text}");
    assert!(
        text.contains("CONSERVER  docs/notes.txt  (absent de la source ; suppressions non autorisées (--allow-delete))"),
        "{text}"
    );
    assert!(
        text.contains(
            "REMPLACER  recent.txt  (destination plus récente, ÉCRASÉE par la source, 6 o)"
        ),
        "{text}"
    );

    let output = preview(s, d, &["--mode", "mirror", "--allow-delete", "--all"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = normalized(&stdout(&output), s, d);
    assert!(
        text.contains("SUPPRIMER  docs/notes.txt  (absent de la source)"),
        "{text}"
    );
    assert!(
        text.contains("CONSERVER  commun.txt"),
        "--all montre les équivalents"
    );
    assert_snapshot("preview_mirror_delete.txt", &text);
    assert_eq!(trees(s, d), before, "la prévisualisation ne modifie rien");
}

#[test]
fn mirror_with_unreadable_source_folder_plans_zero_deletion() {
    // Given : miroir avec suppressions autorisées ; un dossier source est
    //         illisible ; la destination a des fichiers dedans et ailleurs.
    // When  : syncengine preview --mode mirror --allow-delete.
    // Then  : aucune ligne SUPPRIMER, refus motivés, code 3.
    let (source, destination) = fixture();
    let (s, d) = (source.path(), destination.path());
    file_at(s, "prive/s.txt", b"s", T1);
    file_at(d, "prive/orphelin.txt", b"o", T1);
    let Some(_guard) = DenyListing::new(&s.join("prive")) else {
        eprintln!("NON EXÉCUTÉ : permissions non applicables (root ou administrateur Windows)");
        return;
    };
    let output = preview(s, d, &["--mode", "mirror", "--allow-delete"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let text = stdout(&output);
    assert!(!text.contains("SUPPRIMER"), "{text}");
    assert!(
        text.contains("IGNORER    prive/orphelin.txt  (incertain : présence inconnue à la source)"),
        "{text}"
    );
    assert!(
        text.contains(
            "IGNORER    docs/notes.txt  (suppression refusée : inventaire de la source incomplet)"
        ),
        "{text}"
    );
    assert!(text.contains("SUPPRESSIONS REFUSÉES"), "{text}");
    assert!(text.contains("source dossier incomplet : prive"), "{text}");
}

#[test]
fn empty_source_never_empties_the_destination() {
    let (_source, destination) = fixture();
    let empty = TempDir::new().unwrap();
    let output = preview(
        empty.path(),
        destination.path(),
        &[
            "--mode",
            "mirror",
            "--allow-delete",
            "--max-delete-percent",
            "100",
        ],
    );
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let text = stdout(&output);
    assert!(!text.contains("SUPPRIMER"), "{text}");
    assert!(
        text.contains("la source est vide (montage absent ?)"),
        "{text}"
    );
}

#[test]
fn missing_source_produces_no_plan() {
    let (source, destination) = fixture();
    let missing = source.path().join("absent");
    let output = preview(
        &missing,
        destination.path(),
        &["--mode", "mirror", "--allow-delete"],
    );
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(output.stdout.is_empty(), "aucun plan ne doit être affiché");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("ce n'est PAS un dossier vide"), "{stderr}");
}

#[test]
fn nested_destination_is_refused() {
    let (source, _destination) = fixture();
    let inner = source.path().join("docs");
    let output = preview(source.path(), &inner, &["--mode", "mirror"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("la destination est située dans la source"),
        "{stderr}"
    );
}

#[test]
fn backup_refuses_deletion_options() {
    let (source, destination) = fixture();
    let output = preview(source.path(), destination.path(), &["--allow-delete"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn json_output_is_the_versioned_plan() {
    let (source, destination) = fixture();
    let output = preview(
        source.path(),
        destination.path(),
        &["--mode", "mirror", "--allow-delete", "--json"],
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["mode"], "mirror");
    assert_eq!(plan["source"]["complete"], true);
    let actions: Vec<(&str, &str)> = plan["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| (op["path"].as_str().unwrap(), op["action"].as_str().unwrap()))
        .collect();
    assert_eq!(
        actions,
        [
            ("commun.txt", "keep"),
            ("docs", "keep"),
            ("docs/notes.txt", "delete"),
            ("docs/rapport.md", "replace"),
            ("nouveau", "copy"),
            ("nouveau/a.txt", "copy"),
            ("recent.txt", "replace"),
        ]
    );
}
