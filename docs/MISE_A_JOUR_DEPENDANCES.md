# Actualisation des dépendances — 28 octobre 2026

## Périmètre et résultat

Inventaire des manifestes du workspace, de Cargo.lock, des trois copies
vendored, des scripts Python, des outils de génération et de publication,
des images de runners et des actions GitHub. Les versions stables publiées
ont été recherchées, y compris les versions majeures (`cargo upgrade
--incompatible`). Les versions de développement ne sont pas introduites.
La version de l'application n'est pas modifiée.

### Changements

| Dépendance | Avant | Après |
|---|---|---|
| rusqlite | 0.40.2 | 0.41.1 |
| libsqlite3-sys | 0.38.2 | 0.39.1 |
| globset | 0.4.20 | 0.4.21 |
| libz-sys | 1.1.30 | 1.1.31 |
| openssl-probe | 0.2.1 | 0.2.2 |
| ordered-float | 5.5.0 | 5.5.1 |
| rsqlite-vfs | 0.1.1 | 0.2.0 |
| rustix | 1.1.5 | 1.1.6 |
| sqlite-wasm-rs | 0.5.5 | 0.6.2 |
| unicode-width | 0.2.2 | 0.2.3 |
| Python CI | 3.12 | 3.15 (test local : 3.15.2) |
| Ubuntu contrôlé en CI | 24.04 | 24.04 et 26.04 |
| Ubuntu audit/publication | 24.04 | 26.04 |

`Connection::backup` accepte désormais un callback générique : le type de
l'absence de callback est explicité, sans changer le contrat de sauvegarde.
Le test existant de sauvegarde, réouverture et conservation des tombstones
passe. Les changements de format des bases sont couverts par les tests de
migration existants ; aucune migration applicative supplémentaire n'est requise.

Les archives Linux restent construites sous 24.04 : utiliser 26.04 pour les
binaires pourrait relever leur exigence glibc. Windows conserve `windows-2025`,
dernière génération publiée. Les paquets APT sont installés depuis les dépôts
de chaque runner, sans versions anciennes figées.

### Dépendances directes déjà aux dernières versions publiées

Versions confirmées via l'API crates.io : blake3 1.8.7, cap-fs-ext/cap-std/
cap-primitives 4.0.3, chrono 0.4.45, ctrlc 3.5.2, embed-resource 3.0.12,
iced 0.14.0, log 0.4.34 (contrainte `0.4`, dev-dependency), percent-encoding
2.3.2, reqwest 0.13.5 (épinglage exact conservé), rfd 0.17.2, roxmltree
0.21.2, serde 1.0.229, serde_json 1.0.154, ssh2 0.9.6, suppaftp 12.2.1,
tempfile 3.27.0, thiserror 2.0.21, tray-icon 0.26.1, url 2.5.8, winreg 0.56.0.
Après mise à jour, `cargo upgrade --incompatible --dry-run` ne propose plus
de changement. Les dépendances transitives restent dans les contrats semver
de leurs parents ; leurs versions majeures incompatibles ne se substituent
pas arbitrairement aux versions demandées par ces parents.

Rust 1.99.1 et cargo-audit 0.22.2 sont déjà à jour. Les actions suivent leurs
tags majeurs, déjà à jour : checkout v7 (dernière release 7.0.1), setup-python
v7 (7.1.0), upload-artifact v7 (7.0.2), download-artifact v8 (8.0.2).

Paramiko 5.1.0, pyftpdlib 2.2.0 et pyOpenSSL 26.4.0 sont déjà à jour.
`scripts/requirements-network.txt` centralise ces versions et épingle aussi
cryptography 50.0.2, importé directement par les fixtures HTTPS.
Pillow 13.0.0, dernière version, est documenté pour la génération des icônes ;
les ressources ne sont pas régénérées sans changement visuel demandé.

Les copies suppaftp 12.2.1, libssh2-sys 0.3.3 et cap-primitives 4.0.3 sont
aux dernières versions publiées. Leurs correctifs locaux restent nécessaires
et conservés (voir chaque `SYNCENGINE-PATCH.md`). Les workflows et références
Docker sous `vendor/libssh2-sys/libssh2` sont ceux de la source tierce : ils
ne sont pas exécutés par SyncEngine et ne sont pas réécrits. Aucun Dockerfile,
Compose, frontend npm ni autre gestionnaire de dépendances applicatif n'est présent.

Sources officielles : [crates.io](https://crates.io/),
[canal Rust stable](https://static.rust-lang.org/dist/channel-rust-stable.toml),
[Python 3.15.2](https://www.python.org/downloads/release/python-3152/),
[runners GitHub](https://github.com/actions/runner-images),
[versions Python des runners](https://github.com/actions/python-versions),
[PyPI](https://pypi.org/).

## Vérifications locales

Plateforme exécutée : Windows x86_64 MSVC, Rust 1.99.1.
Python 3.15.2 et ses bibliothèques sont installés uniquement sous `target/`
pour cette validation, sans modifier l'interpréteur personnel.
Les commandes Cargo ci-dessous utilisent `RUSTFLAGS=-D warnings` et les
doctests/documentations `RUSTDOCFLAGS=-D warnings`. Les scripts Python utilisent
`PYTHONWARNINGS=error`. La CI applique également ces variables globalement.

Commandes exécutées avec succès :

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --no-deps --all-features --locked
cargo test -p cap-primitives --lib large_read_dir --locked
cargo test -p cap-primitives --lib handle_read_dir --locked
cargo check -p syncengine-cli -p syncengine-agent -p syncengine-desktop --no-default-features --locked
cargo check -p sync-orchestrator --no-default-features --features onedrive --locked
cargo test -p connector-registry --no-default-features --locked
cargo build -p syncengine-cli --all-features --locked
cargo test -p sync-scanner --test parallel_listing --locked -- --ignored
cargo test -p cap-primitives --lib handle_read_dir::tests::million_records_in_bounded_batches --locked -- --ignored
```

Suite workspace : 593 tests réussis, aucun échec, 10 ignorés par défaut.
Les scripts réseau exécutent séparément leurs contrats ignorés (11 passages
Rust au total). Les deux tests synthétiques de volume réussissent aussi.
Les tests Windows vendored et du registre sans features réussissent.

Avec `target/update-venv/Scripts/python.exe` : `scripts/test_release.py`
(6 tests), `scripts/test_versioning.py` (8), `scripts/test_state_maintenance.py`
(7), `scripts/test_phase08_sftp.py`, `scripts/test_phase08_webdav.py`,
`scripts/test_phase08_ftp.py`, `scripts/test_phase09_onedrive.py`, puis
`scripts/test_state_lock.py --binary target/debug/syncengine.exe` : succès.
La cohérence des bibliothèques de l'environnement est vérifiée par `uv pip check`.
Les fixtures HTTPS traitent uniquement les resets avant toute requête HTTP
(refus attendu du certificat jetable) ; les erreurs pendant une requête restent visibles.

## Limites et avis conservés

`cargo audit` : zéro vulnérabilité. `cargo audit --deny warnings` avec les six
exceptions préexistantes de la CI : succès. Sans exceptions, les avis de
maintenance/solidité préexistants restent présents : RUSTSEC-2024-0436,
RUSTSEC-2026-0192, -0253, -0370, -0371, -0372. Aucune exception ajoutée.
Leurs dépendances parentes n'ont pas de nouvelle release corrigeant ces avis ;
voir les conditions de retrait dans [DEVELOPPEMENT.md](DEVELOPPEMENT.md).
Les copies vendored exigent toujours leur surveillance manuelle.

Non exécutés localement : compilation/tests Linux natifs, signal SIGTERM Linux,
interface graphique interactive, publication et installation sur runner jetable.
Docker n'est pas démarré et WSL ne contient que docker-desktop : aucune
validation Ubuntu/Debian locale n'est annoncée. Les jobs CI restent à exécuter
sur GitHub, notamment la nouvelle cible Ubuntu 26.04. Le test NSIS historique
est réservé à un runner jetable ; la livraison actuelle utilise des archives portables.

## Fichiers modifiés

- `Cargo.toml`, `Cargo.lock` : dépendances et résolution actualisées.
- `crates/services/sync-history/src/lib.rs` : adaptation de l'API de sauvegarde.
- `.github/workflows/ci.yml` : Python, Ubuntu, requirements et warnings stricts.
- `scripts/requirements-network.txt` : versions des bibliothèques réseau de test.
- `scripts/test_phase08_webdav.py`, `scripts/test_phase09_onedrive.py` : déconnexions TLS attendues des fixtures.
- `README.md`, `docs/DEVELOPPEMENT.md`, ce rapport : versions, commandes et limites.
- `CHANGELOG.md`, `RELEASE_NOTES.md` : notes du changement.
