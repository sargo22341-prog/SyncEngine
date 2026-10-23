//! Vérifie les règles de couplage de `docs/CONTRATS.md` sur le graphe réel
//! produit par `cargo metadata`.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use serde_json::Value;

/// Dépendances internes autorisées pour chaque paquet du workspace.
///
/// Tout nouveau paquet doit être ajouté ici : un paquet absent fait échouer
/// le test, ce qui force à décider explicitement de sa place dans le graphe.
const ALLOWED_INTERNAL: &[(&str, &[&str])] = &[
    ("sync-core", &[]),
    ("sync-errors", &["sync-core"]),
    ("sync-events", &["sync-core", "sync-errors"]),
    ("connector-api", &["sync-core", "sync-errors"]),
    (
        "syncengine-cli",
        &["sync-core", "sync-errors", "sync-events", "connector-api"],
    ),
    ("syncengine-architecture-tests", &[]),
];

/// Paquets de présentation : seuls autorisés à dépendre de Tauri.
const PRESENTATION: &[&str] = &[];

struct Package {
    name: String,
    description: Option<String>,
    /// Dépendances déclarées (toutes sections : normales, dev, build).
    dependencies: BTreeSet<String>,
}

// Exception locale : fonction d'aide de test hors `#[test]`, un JSON
// inattendu de `cargo metadata` doit faire échouer le test immédiatement.
#[expect(
    clippy::unwrap_used,
    reason = "aide de test : une structure cargo metadata inattendue est un échec de test"
)]
fn workspace_packages() -> Vec<Package> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo metadata a échoué : {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| Package {
            name: package["name"].as_str().unwrap().to_owned(),
            description: package["description"].as_str().map(str::to_owned),
            dependencies: package["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dep| dep["name"].as_str().unwrap().to_owned())
                .collect(),
        })
        .collect()
}

#[test]
fn internal_dependencies_follow_declared_direction() {
    let packages = workspace_packages();
    let internal: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let allowed: BTreeMap<&str, BTreeSet<&str>> = ALLOWED_INTERNAL
        .iter()
        .map(|(name, deps)| (*name, deps.iter().copied().collect()))
        .collect();

    for package in &packages {
        let Some(allowed_deps) = allowed.get(package.name.as_str()) else {
            panic!(
                "paquet {} absent de ALLOWED_INTERNAL : déclarer ses dépendances internes",
                package.name
            );
        };
        for dep in &package.dependencies {
            if internal.contains(dep.as_str()) {
                assert!(
                    allowed_deps.contains(dep.as_str()),
                    "{} ne doit pas dépendre de {dep}",
                    package.name
                );
            }
        }
    }
}

#[test]
fn engines_do_not_depend_on_presentation_layer() {
    for package in workspace_packages() {
        if PRESENTATION.contains(&package.name.as_str()) {
            continue;
        }
        for dep in &package.dependencies {
            assert!(
                !dep.starts_with("tauri"),
                "{} dépend de {dep} : Tauri est réservé à la couche de présentation",
                package.name
            );
        }
    }
}

#[test]
fn sync_core_has_no_internal_dependency() {
    let packages = workspace_packages();
    let internal: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();
    let core = packages.iter().find(|p| p.name == "sync-core").unwrap();
    let internal_deps: Vec<&String> = core
        .dependencies
        .iter()
        .filter(|dep| internal.contains(dep.as_str()))
        .collect();
    assert!(internal_deps.is_empty(), "{internal_deps:?}");
}

#[test]
fn every_package_describes_its_responsibility() {
    for package in workspace_packages() {
        let description = package.description.unwrap_or_default();
        assert!(
            description.trim().len() >= 20,
            "{} doit décrire sa responsabilité dans Cargo.toml",
            package.name
        );
    }
}
