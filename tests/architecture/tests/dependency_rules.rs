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
    ("connector-local", &["sync-core", "connector-api"]),
    ("connector-memory", &["sync-core", "connector-api"]),
    ("sync-filter", &["sync-core"]),
    (
        "sync-scanner",
        &[
            "sync-core",
            "sync-errors",
            "sync-filter",
            "connector-api",
            "connector-memory",
        ],
    ),
    (
        "sync-compare",
        &[
            "sync-core",
            "sync-errors",
            "sync-scanner",
            "connector-api",
            "connector-memory",
            "sync-filter",
        ],
    ),
    // Contrat de plan : ne connaît ni la découverte ni les connecteurs, pour
    // que l'exécuteur (phase 04) et l'interface n'en dépendent pas.
    ("sync-plan", &["sync-core", "sync-errors", "sync-filter"]),
    (
        "sync-decisions",
        &[
            "sync-core",
            "sync-errors",
            "sync-plan",
            "sync-compare",
            // dev-dependencies (tests sur inventaires en mémoire)
            "connector-api",
            "connector-memory",
            "sync-filter",
            "sync-scanner",
        ],
    ),
    (
        "sync-backup",
        &[
            "sync-decisions",
            "sync-plan",
            // dev-dependencies
            "connector-memory",
            "sync-compare",
            "sync-core",
            "sync-filter",
            "sync-scanner",
        ],
    ),
    (
        "sync-mirror",
        &[
            "sync-core",
            "sync-decisions",
            "sync-plan",
            // dev-dependencies
            "connector-api",
            "connector-memory",
            "sync-compare",
            "sync-filter",
            "sync-scanner",
        ],
    ),
    (
        "syncengine-cli",
        &[
            "sync-core",
            "sync-errors",
            "sync-events",
            "connector-api",
            "connector-local",
            "sync-filter",
            "sync-scanner",
            "sync-compare",
            "sync-plan",
            "sync-decisions",
            "sync-backup",
            "sync-mirror",
        ],
    ),
    ("syncengine-architecture-tests", &[]),
];

/// Doubles de test : autorisés uniquement en dev-dependency.
const TEST_DOUBLES: &[&str] = &["connector-memory"];

/// Paquets de planification : purs, sans E/S. Ils ne dépendent d'aucune
/// implémentation de connecteur (seul le contrat `connector-api` est admis,
/// et seulement pour les tests).
const PLANNING: &[&str] = &["sync-plan", "sync-decisions", "sync-backup", "sync-mirror"];

/// Paquets de présentation : seuls autorisés à dépendre de Tauri.
const PRESENTATION: &[&str] = &[];

struct Package {
    name: String,
    description: Option<String>,
    /// Dépendances déclarées (toutes sections : normales, dev, build).
    dependencies: BTreeSet<String>,
    /// Dépendances hors dev-dependencies (normales et build).
    non_dev_dependencies: BTreeSet<String>,
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
            non_dev_dependencies: package["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|dep| dep["kind"].as_str() != Some("dev"))
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
fn test_doubles_are_dev_dependencies_only() {
    for package in workspace_packages() {
        for double in TEST_DOUBLES {
            assert!(
                !package.non_dev_dependencies.contains(*double),
                "{} dépend de {double} hors dev-dependencies : double de test réservé aux tests",
                package.name
            );
        }
    }
}

#[test]
fn planning_crates_never_depend_on_connector_implementations() {
    for package in workspace_packages() {
        if !PLANNING.contains(&package.name.as_str()) {
            continue;
        }
        for dep in &package.non_dev_dependencies {
            assert!(
                !dep.starts_with("connector-"),
                "{} dépend de {dep} : la planification ne fait aucune E/S",
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
