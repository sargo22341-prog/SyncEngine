//! Tests d'intégration du binaire `syncengine`.
//!
//! Les commandes de découverte sont exercées sur des répertoires temporaires
//! isolés : connecteur local + scanner + filtre + comparaison, de bout en bout.

// Aides de test hors `#[test]` : `clippy.toml` n'y autorise pas `unwrap`,
// mais un échec y est bien un échec de test.
#![expect(
    clippy::unwrap_used,
    reason = "aides de test : un échec est un échec de test"
)]

mod common;

use std::fs;
use std::path::Path;

use common::{DenyListing, snapshot, stdout, syncengine, write};
use tempfile::TempDir;

#[test]
fn version_exits_successfully() {
    let output = syncengine(["--version"]);
    assert!(output.status.success());
    assert!(stdout(&output).starts_with(concat!("syncengine ", env!("CARGO_PKG_VERSION"))));
    assert!(output.stderr.is_empty());
}

#[test]
fn help_is_default() {
    let output = syncengine::<[&str; 0], &str>([]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("COMMANDES"));
    assert!(text.contains("compare"));
}

#[test]
fn unknown_command_is_usage_error() {
    let output = syncengine(["delete-everything"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("commande inconnue"));
}

/// Copie la date de modification de `from` vers `to`.
fn same_mtime(from: &Path, to: &Path) {
    let modified = fs::metadata(from).unwrap().modified().unwrap();
    fs::File::options()
        .write(true)
        .open(to)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

/// Deux arborescences : une identique, une modifiée, une ajoutée de chaque côté.
fn pair() -> (TempDir, TempDir) {
    let left = TempDir::new().unwrap();
    let right = TempDir::new().unwrap();
    write(left.path(), "commun.txt", b"identique");
    write(right.path(), "commun.txt", b"identique");
    same_mtime(
        &left.path().join("commun.txt"),
        &right.path().join("commun.txt"),
    );
    write(left.path(), "docs/rapport.md", b"version longue");
    write(right.path(), "docs/rapport.md", b"courte");
    write(left.path(), "nouveau.txt", b"n");
    write(right.path(), "docs/ancien.txt", b"a");
    write(left.path(), "cache/tmp.bin", b"x");
    write(right.path(), "cache/tmp.bin", b"y");
    (left, right)
}

#[test]
fn scan_lists_inventory_and_reports_completeness() {
    let (left, _right) = pair();
    let output = syncengine([Path::new("scan"), left.path()]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("docs/rapport.md"));
    assert!(text.contains("nouveau.txt"));
    assert!(text.contains("Inventaire COMPLET"));
}

#[test]
fn scan_with_hash_and_filters() {
    let (left, _right) = pair();
    let output = syncengine([
        "scan".as_ref(),
        left.path().as_os_str(),
        "--hash".as_ref(),
        "--exclude-dir".as_ref(),
        "cache".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(!text.contains("tmp.bin"));
    // Empreinte BLAKE3 (64 caractères hexadécimaux) de « identique ».
    let expected = blake3_hex(b"identique");
    assert!(text.contains(&expected), "{text}");
}

/// Empreinte de référence, calculée directement par la bibliothèque BLAKE3.
fn blake3_hex(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

#[test]
fn compare_reports_differences_without_modifying_anything() {
    let (left, right) = pair();
    let before = snapshot(left.path()) + &snapshot(right.path());
    let output = syncengine([
        "compare".as_ref(),
        left.path().as_os_str(),
        right.path().as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.contains("<   nouveau.txt  (gauche seulement)"),
        "{text}"
    );
    assert!(
        text.contains(">   docs/ancien.txt  (droite seulement)"),
        "{text}"
    );
    assert!(
        text.contains("!=  docs/rapport.md  (taille 14 ≠ 6"),
        "{text}"
    );
    assert!(!text.contains("commun.txt"), "équivalent masqué sans --all");
    assert!(text.contains("simulation, aucune modification"));
    assert_eq!(snapshot(left.path()) + &snapshot(right.path()), before);

    let output = syncengine([
        "compare".as_ref(),
        left.path().as_os_str(),
        right.path().as_os_str(),
        "--all".as_ref(),
        "--hash".as_ref(),
    ]);
    let text = stdout(&output);
    assert!(
        text.contains("=   commun.txt  (contenu identique)"),
        "{text}"
    );
    assert!(
        text.contains("!=  cache/tmp.bin  (contenu différent"),
        "{text}"
    );
}

#[test]
fn identical_folders_exit_zero() {
    let left = TempDir::new().unwrap();
    let right = TempDir::new().unwrap();
    write(left.path(), "a/b.txt", b"meme");
    write(right.path(), "a/b.txt", b"meme");
    same_mtime(&left.path().join("a/b.txt"), &right.path().join("a/b.txt"));
    let output = syncengine([
        "compare".as_ref(),
        left.path().as_os_str(),
        right.path().as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

#[test]
fn missing_folder_is_a_failure_not_an_empty_side() {
    let (left, _right) = pair();
    let missing = left.path().join("absent");
    let output = syncengine([
        "compare".as_ref(),
        left.path().as_os_str(),
        missing.as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "aucun constat ne doit être affiché"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("ce n'est PAS un dossier vide"), "{stderr}");
}

#[test]
fn invalid_filter_is_usage_error() {
    let (left, _right) = pair();
    let output = syncengine([
        "scan".as_ref(),
        left.path().as_os_str(),
        "--exclude".as_ref(),
        "/absolu".as_ref(),
    ]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn access_denied_source_folder_gives_uncertainty_never_right_only() {
    // Given : la source a un dossier illisible ; la destination a des
    //         fichiers dans le dossier correspondant.
    // When  : compare source destination.
    // Then  : ces fichiers sont « incertains », jamais « droite seulement »,
    //         et le code de sortie vaut 3.
    let (left, right) = pair();
    write(right.path(), "docs/prive/orphelin.txt", b"o");
    write(left.path(), "docs/prive/s.txt", b"s");
    let Some(_guard) = DenyListing::new(&left.path().join("docs").join("prive")) else {
        eprintln!("NON EXÉCUTÉ : permissions non applicables (root ou administrateur Windows)");
        return;
    };
    let output = syncengine([
        "compare".as_ref(),
        left.path().as_os_str(),
        right.path().as_os_str(),
    ]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.contains("??  docs/prive/orphelin.txt  (incertain : présence inconnue à gauche"),
        "{text}"
    );
    assert!(!text.contains(">   docs/prive"), "{text}");
    assert!(text.contains("dossier incomplet : docs/prive"), "{text}");
    assert!(
        text.contains("aucune suppression ne doit en être déduite"),
        "{text}"
    );

    let output = syncengine([Path::new("scan"), left.path()]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(stdout(&output).contains("Inventaire INCOMPLET"));
}
