# Développement

Règles de contribution (humains et agents IA) : [`AGENTS.md`](../AGENTS.md).
Organisation du code : [Architecture](ARCHITECTURE.md).

## Prérequis

La chaîne Rust est épinglée dans `rust-toolchain.toml` (édition 2024) ;
`rustup` l'installe au premier appel de `cargo`.

- **Windows 10/11 x86_64** : `rustup` (cible MSVC) et Visual Studio Build Tools,
  charge « Développement Desktop en C++ » (compilateur C pour BLAKE3, SQLite et
  libssh2 ; `rc.exe` pour l'icône de l'exécutable).
- **Debian/Ubuntu x86_64** : `build-essential pkg-config libssl-dev zlib1g-dev`.
  À l'exécution : `ca-certificates`, OpenSSL, et X11 ou Wayland avec Vulkan/GL
  pour l'interface.

SQLite est compilé avec le projet (`rusqlite` *bundled*). FTP/FTPS utilise une
copie corrigée de SuppaFTP sous `vendor/suppaftp` (voir `SYNCENGINE-PATCH.md`) :
ne pas la remplacer par la version amont sans revérifier ce correctif.

## Lancer

```sh
cargo run -p syncengine-desktop                              # dossiers locaux
cargo run -p syncengine-desktop --features network,onedrive  # avec les connecteurs
cargo run -p syncengine-cli -- help
cargo build --release -p syncengine-desktop -p syncengine-cli -p syncengine-agent --features network,onedrive
```

Pour ne pas toucher à ses vrais profils, définir `SYNCENGINE_STATE_DIR` sur un
chemin absolu dédié.

## Contrôles

À passer avant chaque commit ; la CI les exécute sous Ubuntu 24.04 et
Windows et refuse tout warning :

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTFLAGS='-D warnings' cargo check --workspace --all-targets --all-features
```

Sous PowerShell : `$env:RUSTFLAGS='-D warnings'; cargo check --workspace --all-targets --all-features`.

Les tests de connecteurs réseau sont ignorés par `cargo test` : des scripts
lancent des serveurs éphémères en local (certificats et clés jetables sous
`target/`, rien n'est installé sur le système) :

```sh
python -m pip install paramiko==5.1.0 pyftpdlib==2.2.0 pyopenssl==26.4.0
python scripts/test_phase08_sftp.py
python scripts/test_phase08_webdav.py
python scripts/test_phase08_ftp.py
python scripts/test_phase09_onedrive.py
```

Autres scripts testés en CI : `test_release.py`, `test_versioning.py`,
`test_state_maintenance.py`, `test_state_lock.py --binary target/debug/syncengine`
et, sous Linux, `test_signals.py`. Aucun test ne doit dépendre d'un compte
réel ni toucher aux dossiers personnels : utiliser des dossiers temporaires et
le connecteur mémoire (`tests/support/connector-memory`).

## Versions et publication

Tout se fait dans `.github/workflows/ci.yml` :

- chaque pull request et chaque push sur `main` : contrôles ci-dessus, tests
  réseau et `cargo audit` (une vulnérabilité bloque ; les avertissements
  hérités d'iced sont signalés sans bloquer) ;
- chaque push sur `main` qui ne touche pas que la documentation (`*.md`,
  `docs/`, `LICENSE`) publie une version :
  1. `scripts/versioning.py` monte le patch de `[workspace.package].version`
     et crée le commit `Version X.Y.Z [skip ci]` ;
  2. un job par OS construit les trois exécutables et l'archive portable
     (`.zip` Windows, `.tar.gz` Linux) avec manifeste, notices tierces et
     `SHA256SUMS` (`scripts/release.py`) ;
  3. le commit et le tag `vX.Y.Z` sont poussés, puis la GitHub Release est
     créée ;
- **Actions › CI › Run workflow** permet de monter `minor` ou `major`.

Ne jamais modifier la version à la main. Chaque changement visible ajoute une
ligne `- …` sous `<!-- notes -->` dans `RELEASE_NOTES.md` : ce texte devient la
description de la release, puis la liste est vidée par le commit de version.
`CHANGELOG.md` garde le détail des changements de comportement. Après une
release, faire `git pull` avant de reprendre le travail.

Les binaires ne sont pas signés. Reproduire une archive en local :

```sh
python scripts/release.py prepare
python scripts/release.py portable --bundle target/release/bundle --target x86_64-pc-windows-msvc
python scripts/release.py collect --bundle target/release/bundle --output target/release-candidate --target x86_64-pc-windows-msvc --formats portable
```

(`x86_64-unknown-linux-gnu` sous Linux.)

## Icône

Source : `docs/icon/syncengine.png`. `python scripts/generate_icons.py docs/icon/syncengine.png`
(Pillow) régénère `apps/desktop/assets/` : `.ico` intégré à l'exécutable
Windows par `build.rs`, `.png` pour l'archive Linux, `.rgba` pour la fenêtre et
la zone de notification.

## Mesures

`python scripts/measure_scan.py --binary <chemin de syncengine> --files 10000`
mesure le temps de scan (métadonnées et BLAKE3) et la mémoire sur des fichiers
synthétiques en dossier temporaire.
