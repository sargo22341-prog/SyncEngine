# Livraison de l'agent résident — 25 octobre 2026

## Comportement livré

Agent Windows résident sous tâche utilisateur ordinaire, indépendant de la
fenêtre. Les horaires restent dans SQLite et sont relus toutes les cinq
secondes au repos. Session WTS persistée, délais ancrés à la connexion réelle,
exclusion des doubles instances, pause, arrêt coopératif et présence vérifiée
par verrou OS. Migration des anciennes tâches avec vérification d'appartenance,
reprise après erreur et conservation des autorisations. Les moteurs, garde-fous,
conflits bidirectionnels et unités Linux existants sont réutilisés.

Le fonctionnement, les commandes, l'exemple utilisateur et les limites sont
décrits dans [AGENT_RESIDENT.md](AGENT_RESIDENT.md).

## Fichiers créés ou modifiés

| Fichiers | Raison |
| --- | --- |
| `apps/agent/src/main.rs`, `apps/agent/src/resident.rs` (nouveau) | commandes de cycle de vie, partage du passage d'exécution, boucle résidente et annulation |
| `apps/agent/tests/agent.rs` | vrais processus isolés : instance unique, changements à chaud, pause, arrêt et crash |
| `apps/desktop/Cargo.toml` | adaptateur Windows utilisable par la préparation native en production, dépendances déjà présentes en tests |
| `apps/desktop/src/backend.rs` | observation du résident, migration automatique au démarrage et reprise native |
| `apps/desktop/src/app.rs`, `apps/desktop/src/actions.rs`, `apps/desktop/src/update.rs` | transport des observations et commande de reprise hors du thread graphique |
| `apps/desktop/src/schedules.rs` | états disponibles, périmés, suspendus, en cours et erreurs contextualisées |
| `apps/desktop/src/windows_tasks.rs` | installation de la tâche résidente et migration contrôlée des tâches historiques |
| `apps/desktop/src/windows_tasks_mock.ps1` (nouveau) | superviseur entièrement en mémoire pour les scénarios de migration et de refus |
| `crates/adapters/platform-windows/src/lib.rs` | XML résident, relances bornées, arguments natifs et lecture de session avec délai maximal |
| `crates/adapters/platform-windows/Cargo.toml` | description ajustée à la responsabilité native effective |
| `crates/adapters/platform-windows/src/session.ps1` (nouveau) | pont WTS courant, sans requête administrative sur les autres sessions |
| `crates/adapters/platform-windows/tests/native.rs` | validation COM sans installation et stabilité de l'identité WTS |
| `crates/application/sync-orchestrator/src/lib.rs`, `crates/application/sync-orchestrator/src/resident.rs` (nouveau) | API minimale de présence, phases typées et marqueur d'arrêt |
| `crates/application/sync-orchestrator/src/runs.rs` | journaux terminés limités par la rétention existante ; tests de sécurité du nettoyage |
| `crates/services/sync-scheduler/src/store.rs` | migration v4, session durable et raccord des anciens reçus sans rejeu |
| `crates/services/sync-scheduler/migrations/004_resident.sql` (nouveau) | une ligne de session courante, sans suppression des données existantes |
| `crates/services/sync-scheduler/tests/scheduler.rs` | sessions, reprises, délais, politiques d'échéance, horloge et changements sur une autre connexion |
| `docs/ARCHITECTURE.md`, `docs/PLANIFICATION.md`, `docs/GUIDE.md` | contrats, parcours utilisateur et fichiers d'état mis à jour |
| `docs/AGENT_RESIDENT.md`, ce rapport (nouveaux) | choix Windows, cycle de vie, migration, exemple complet et validation reproductible |
| `CHANGELOG.md`, `RELEASE_NOTES.md` | comportement détaillé et ligne française sous le véritable marqueur |

Aucun changement manuel de version, commit ou push.

Le changement entrant `2604896` (`Version 0.1.5 [skip ci]`) a été intégré
par avance rapide après `git fetch origin`. Le chevauchement de
`RELEASE_NOTES.md` a été résolu en gardant la nouvelle note du résident et le
retrait effectué par la CI de la note déjà publiée. La version `0.1.5` et le
lockfile proviennent entièrement de ce commit CI. Les autres changements
locaux ont été vérifiés identiques octet par octet après intégration ; `main`
est alignée sur `origin/main`, avec le travail de cette tâche non commité.

## Commandes réellement exécutées

Contrôles finaux réussis sous Windows :

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
$env:RUSTFLAGS='-D warnings'
cargo check --workspace --all-targets --all-features
git -c core.safecrlf=false diff --check
```

Résultat de la dernière suite complète : **490 tests réussis, zéro échec,
7 tests ignorés**, doctests inclus. Les sept tests ignorés exigent les serveurs
isolés FTP/FTPS, SFTP, WebDAV et OneDrive ; ils n'ont pas été exécutés.
La sortie locale est dans `target/resident-tests.log` (non versionnée).

Contrôles ciblés également exécutés pendant le développement :

```powershell
cargo check -p syncengine-agent -p syncengine-desktop --all-features
cargo test -p sync-scheduler -p platform-windows -p syncengine-agent --all-features
cargo test -p sync-scheduler -p syncengine-agent --all-features
cargo test -p platform-windows --test native --all-features
cargo test -p syncengine-desktop --all-features migration_is_idempotent -- --nocapture
cargo test -p syncengine-desktop --all-features
```

Les premières passes de compilation/Clippy ont signalé des erreurs de code ou
de format corrigées avant les contrôles finaux. La première validation COM
restreinte a échoué avec `0x80070003` lors de l'ouverture de la racine du
Planificateur. Elle réussit hors sandbox avec `TASK_VALIDATE_ONLY`, qui ne crée
aucune tâche et ne lance aucune synchronisation. La suite complète a utilisé
cet accès pour ces validations natives. Aucun test n'installe de tâche
persistante ni ne modifie le démarrage du compte personnel.

## Critères de sécurité vérifiés

- Étant donné un résident propriétaire du verrou, lorsqu'un second résident
  ou un ancien `tick` démarre, il est refusé avant toute distribution.
- Étant donné une connexion déjà observée, lorsqu'on rouvre le scheduler dans
  la même session, le logon n'est pas rejoué et son délai n'est pas décalé.
- Étant donné une interruption, les prises durables existantes empêchent un
  rejeu aveugle ; seul un diagnostic explicite permet la reprise.
- Étant donné un horaire modifié ou retiré par une autre connexion SQLite,
  le résident utilise le nouveau contenu au prochain passage libre.
- Étant donné une source miroir inaccessible ou incomplète, les tests des
  moteurs et de l'agent vérifient l'absence de suppressions. Les tests
  bidirectionnels de baseline, conflits et tombstones restent réussis.
- Étant donné un ancien job encore actif, une migration interrompue ou un
  refus d'accès simulé, la préparation signale l'échec, ne tue pas le job et
  peut être répétée. Les tâches étrangères sont conservées.
- Étant donné plus de 100 tentatives terminées, seuls leurs journaux expirés
  référencés sont retirés. Une prise non terminée et les fichiers étrangers
  restent conservés. Un identifiant avec traversée de chemin est refusé ; une
  erreur de nettoyage est signalée et ne valide pas silencieusement la fin.

## Vérifications non exécutées et limites

- Pas de test Linux : les unités et passages systemd restent inchangés, mais
  aucune compilation ni exécution Linux n'est revendiquée.
- Pas d'installation réelle, d'interaction UAC, de changement de session, de
  redémarrage système ni de relance réelle par Task Scheduler : il faut un
  compte/une VM jetable. Le superviseur de migration est simulé ; le XML et
  la session WTS sont validés nativement.
- Pas de vérification graphique manuelle des parcours profil/groupe/assistant
  dans cette session ; les contrôles Rust et tests de présentation passent.
- Trois relances à une minute constituent une politique bornée, sans garantie
  de relance illimitée ni récupération d'un processus bloqué. Un blocage d'E/S
  OS peut retarder l'annulation coopérative. La GUI signale une observation
  ancienne ; elle ne tue pas une mutation en cours.
- La première session détentrice du verrou possède le résident d'un état ;
  les autres sessions ne distribuent pas leur propre événement en parallèle.
- Les anciens journaux déjà orphelins restent conservés pour inspection ; les
  versions sauvegardées conservent leur politique existante.
- La surveillance de fichiers n'est pas implémentée.

## Texte de commit proposé

```text
Rendre l’agent Windows résident et autonomiser les planifications

- Superviser l’agent utilisateur avec une tâche Windows et des relances bornées
- Persister la session WTS pour reprendre les délais sans rejouer le logon
- Migrer les anciens déclenchements et protéger les exécutions concurrentes
- Afficher la présence, la pause et les erreurs ; permettre un arrêt coopératif
- Borner les journaux terminés, ajouter les tests et documenter les limites
```
