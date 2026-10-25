# Développement

Règles de contribution (humains et agents IA) : [`AGENTS.md`](../AGENTS.md).
Organisation du code : [Architecture](ARCHITECTURE.md).

Le lecteur d'inventaire Windows vendored possède des tests séparés du workspace :
`cargo test -p cap-primitives --lib large_read_dir --locked` (également en CI).
Exécuter aussi `cargo test -p cap-primitives --lib handle_read_dir --locked`.
Mesure locale reproductible, sans NAS :
`cargo run --release -p sync-orchestrator --example scan_handles_timing --locked`.
Méthode et limites : [audit des inventaires réseau](AUDIT_PERFORMANCE_RESEAU.md).
Le benchmark accepte aussi `-- 64 32 64` pour 133 184 entrées temporaires ;
le [complément SMB](AUDIT_SMB_205S.md) donne la comparaison avec la version précédente.
La [mesure sur NAS réel](AUDIT_NAS_REEL.md) décrit les commandes de simulation
sans mutation, le diagnostic du repli et les limites des mesures.
Test de volume synthétique du scanner (environ un million d'entrées, en RAM) :
`cargo test --release -p sync-scanner --test parallel_listing --locked -- --ignored --nocapture`.

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
ne pas la remplacer par la version amont sans revérifier ce correctif. SFTP
utilise une copie corrigée de `libssh2-sys` sous `vendor/libssh2-sys` (échange
de clés WinCNG sous Windows, libssh2#2583 non encore publié ; voir son
`SYNCENGINE-PATCH.md`). `cargo audit` ne contrôle pas ces copies locales :
surveiller à la main les avis visant `suppaftp`, `libssh2-sys` et `ssh2`.

Les chemins de handles Windows utilisent aussi un correctif local de
`cap-primitives 4.0.3` : conversion des racines UNC étendues sans les transformer
en chemins relatifs (voir [provenance et tests](../vendor/cap-primitives/SYNCENGINE-PATCH.md)).
Ses licences sont incluses ; surveiller également les avis `cap-primitives` et
`cap-std`. Les tests de conversion sont exécutés dans le workspace Windows,
sans aucun accès à un NAS :

```sh
cargo test -p connector-local --lib unc_path_regression
```

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

Les tests de refus d'accès (`DenyListing`) retirent le droit de lister un
dossier ; sous root (Linux) ou sous un administrateur Windows disposant du
privilège de sauvegarde (runners GitHub), l'OS ignore ce refus : le test
affiche `NON EXÉCUTÉ` et ne vérifie rien. Il s'exécute réellement sous Ubuntu
en CI (utilisateur non root) et sur un poste Windows non administrateur.

### Avis RustSec tolérés

`cargo audit` (vulnérabilités) ne tolère aucun avis. `cargo audit --deny
warnings` bloque aussi tout nouvel avertissement, sauf les avis ci-dessous :
tous transitifs, sans version corrigée compatible publiée. Aucun n'est une
vulnérabilité connue. Les retirer de `.github/workflows/ci.yml` dès que leur
condition de retrait est remplie (une étape non bloquante les rappelle à chaque
run).

| Avis | Crate | Origine | Condition de retrait |
|---|---|---|---|
| RUSTSEC-2026-0370, -0371, -0372 | `zvariant`, `zbus_names`, `zvariant_derive` (non maintenus : fusionnés dans `zbus` 6) | `zbus` 5 via `ksni` (icône de notification Linux, `tray-icon`) et `mundy` (thème système, `iced_winit`) | `ksni` et `mundy` publiés sur `zbus` 6 (portage en cours : iovxw/ksni#44, #45), puis `cargo update` |
| RUSTSEC-2026-0253 | `lru` 0.16 (« unsound » : `LruCache::pop()` si le `Drop` d'une clé panique sous `catch_unwind`) | `cryoglyph` 0.1 (rendu du texte d'`iced_wgpu` 0.14) | iced publié avec `cryoglyph` sur `lru` ≥ 0.18.2 (déjà le cas sur la branche principale d'iced) |
| RUSTSEC-2026-0192 | `ttf-parser` (non maintenu) | `fontdb` 0.23 / `cosmic-text` 0.15 (iced 0.14), `ab_glyph` (décorations Wayland de `winit`) | iced publié sur `cosmic-text`/`fontdb` 0.24 (`skrifa`) |
| RUSTSEC-2024-0436 | `paste` (non maintenu) | `metal` 0.32 via `wgpu` 27 ; **macOS uniquement**, jamais compilé pour Windows ou Linux | iced publié sur `wgpu` ≥ 30 (`objc2-metal`) |

La branche principale d'iced (0.15 en développement) règle `lru`, `ttf-parser`
et `paste`, mais refond l'API des widgets et n'est pas publiée : la migration
attend une version publiée d'iced.

## Versions et publication

Tout se fait dans `.github/workflows/ci.yml` :

- chaque pull request et chaque push sur `main` : contrôles ci-dessus, tests
  réseau et `cargo audit` (une vulnérabilité ou un nouvel avertissement
  bloque ; seuls les [avis tolérés](#avis-rustsec-tolérés) passent) ;
- chaque push sur `main` qui ne touche pas que la documentation (`*.md`,
  `docs/`, `LICENSE`) publie une version :
  1. `scripts/versioning.py` monte le patch de `[workspace.package].version`
     et crée le commit `Version X.Y.Z [skip ci]` ;
  2. un job par OS construit les trois exécutables et l'archive portable
     (`.zip` Windows, `.tar.gz` Linux, `scripts/release.py`). L'archive ne
     contient que le strict nécessaire : `syncengine-desktop`, `syncengine`,
     `syncengine-agent`, `LICENSE.txt` et `THIRD_PARTY_NOTICES.txt` (textes
     exigés par la redistribution). Manifeste, inventaires de sources et de
     licences et `SHA256SUMS` servent à la vérification et restent dans les
     artefacts du run, jamais publiés ;
  3. le commit et le tag `vX.Y.Z` sont poussés, puis la GitHub Release est
     créée avec **les deux archives seulement** ; leurs empreintes SHA-256
     sont ajoutées à la fin de la description ;
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
Windows par `build.rs`, `.rgba` intégré pour la fenêtre et la zone de
notification. Aucune image n'est livrée à côté des exécutables.

## Mesures

`python scripts/measure_scan.py --binary <chemin de syncengine> --files 10000`
mesure le temps de scan (métadonnées et BLAKE3) et la mémoire sur des fichiers
synthétiques en dossier temporaire.
