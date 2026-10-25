# Validation : pause/reprise et chemins UNC

Session du 25 octobre 2026, Windows. Linux et vérification visuelle interactive
de l’interface non exécutés. Aucun fichier du NAS n’a été créé, modifié ou
supprimé ; après la consigne explicite de ne pas y toucher, aucun nouvel accès
au NAS n’a été effectué. Les tests utilisent des stockages mémoire, des chemins
fictifs ou des répertoires temporaires locaux. Le correctif UNC n’a pas été
retesté sur le NAS réel.

## Critères vérifiés

- Le contrôle Pause/Reprendre est commun aux fiches profil, au suivi d’activité,
  au contrôle latéral et au suivi de l’agent avant ouverture des bases.
- La copie reprend le même flux : dix blocs attendus, sans recommencer le
  fichier, puis contenu intégral vérifié. Annuler pendant l’attente ne publie
  pas le fichier final et nettoie le temporaire.
- L’inventaire reprend correctement ou retourne explicitement un inventaire
  incomplet en cas d’annulation. Les observateurs et commandes de contrôle
  restent disponibles pendant l’attente.
- Une analyse planifiée en pause conserve le verrou applicatif, ne publie
  rien, puis reprend jusqu’au succès ou s’annule sans écriture.
- Les demandes interprocessus périmées ne suspendent pas une autre commande.
- Given un chemin UNC étendu, When son préfixe est converti, Then il conserve
  sa racine absolue et ne devient pas un chemin relatif au dossier courant.
  Couverture : racine, sous-dossier, Unicode, UTF-16 non Unicode, lecteur,
  périphérique. Aucune connexion réseau n’est nécessaire pour ces tests.

## Commandes exécutées

Contrôles réussis :

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
$env:RUSTFLAGS = "-D warnings"
cargo check --workspace --all-targets --all-features --locked
```

La suite complète : **506 réussis, 0 échoué, 7 ignorés** (contrats réseau
nécessitant des serveurs éphémères). Un test d’intégration ajouté ensuite a
également réussi par la commande ciblée ci-dessous. Le premier passage de la
suite sous sandbox a échoué sur `icacls /deny` dans un test existant ; la
relance hors sandbox a réussi. Les permissions concernent uniquement le
répertoire temporaire du test, jamais un partage réseau.

Autres commandes ciblées exécutées :

```powershell
cargo test -p sync-core -p sync-copy -p sync-orchestrator --all-features
cargo test -p sync-scanner --all-features
cargo test -p sync-orchestrator --all-features
cargo test -p connector-local --lib unc_path_regression
cargo test -p sync-orchestrator --all-features paused_scheduled_analysis_keeps_ownership_and_can_resume_or_cancel
cargo clippy -p syncengine-desktop --all-targets --all-features --locked -- -D warnings
git diff --check
```

Le premier test unitaire interprocessus a révélé un dossier d’état manquant
dans son fixture ; corrigé, puis repassé avec succès. Clippy a signalé un
`eprintln!` interdit ; remplacé par une remontée d’erreur après annulation
coopérative. Les contrôles finaux ne masquent aucun lint.

## Fichiers concernés

- Présentation : `apps/desktop/src/{app,icons,profile,shell,update}.rs`.
- Agent : `apps/agent/src/main.rs` (watcher de pause et maintien de l’annulation).
- Jeton partagé : `crates/foundation/sync-core/src/cancel.rs`.
- Façade : `crates/application/sync-orchestrator/src/{lib,live}.rs` et
  `tests/application.rs` dans le même crate.
- Inventaire : `crates/discovery/sync-scanner/src/scan.rs` et `tests/scan.rs`.
- Copie : `crates/execution/sync-copy/src/lib.rs` et `tests/copy.rs`.
- Exécution : `crates/execution/sync-executor/src/{preflight,run}.rs`.
- Vérification : `crates/execution/sync-verify/src/lib.rs`.
- Adaptateur local : `crates/adapters/connector-local/src/{connector,write,lib}.rs`.
- Dépendances : `Cargo.toml`, `Cargo.lock`, copie tierce `vendor/cap-primitives/`
  (sources, manifeste, provenance, README, copyright et licences d’origine).
  Les seules différences par rapport à l’archive tierce sont détaillées dans
  [SYNCENGINE-PATCH.md](../vendor/cap-primitives/SYNCENGINE-PATCH.md).
- Documentation : `docs/{ARCHITECTURE,DEVELOPPEMENT,GUIDE,VALIDATION_PAUSE_UNC}.md`,
  `CHANGELOG.md`, `RELEASE_NOTES.md`.

## Limites et entretien

Pause coopérative, sans interruption des appels système engagés. La comparaison
et la planification CPU s’arrêtent à leur frontière, pas au milieu de leur
calcul. La demande affichée n’est pas un acquittement de tous les workers.
Les verrous et handles restent détenus ; la pause ne survit pas au processus.
Une requête de pause agent invalide provoque une annulation diagnostiquée,
plutôt qu’un succès silencieux.

Le correctif tiers Windows est une dette de maintenance à retirer dès sa
publication amont. Le défaut reste visible dans
[la conversion amont](https://github.com/bytecodealliance/cap-std/blob/b7acf8e8807fe3fab991884d2208b7e03d35a409/cap-primitives/src/windows/fs/get_path.rs).
Les licences sont conservées et l’outil de notices existant inventorie cette
dépendance à chemin local. Aucun nouveau contrat n’est introduit dans les
moteurs de comparaison ou de stratégie ; les ajouts au jeton et au snapshot
partagés sont documentés pour revue d’intégration.
