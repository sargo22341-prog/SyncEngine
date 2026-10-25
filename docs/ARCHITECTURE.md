# Architecture

SyncEngine est un workspace Cargo : un crate par responsabilité, tous liés à la
compilation (pas de plugins dynamiques). Les trois exécutables partagent les
mêmes moteurs.

## Flux d'une exécution

```text
connecteurs ──► scanner ──► comparaison ──► stratégie ──► plan ──► exécuteur
(lecture)       inventaire   constats        décisions     JSON      mutations
```

1. **Scan** (`sync-scanner`) : inventaire de chaque côté via un connecteur, filtres
   appliqués, empreintes BLAKE3 si demandées. Les zones illisibles sont
   marquées *incomplètes*, jamais vides. Ne modifie rien.
2. **Comparaison** (`sync-compare`) : confronte les deux inventaires et produit
   des **faits** par chemin (identique, différent, absent, incertain). Aucune
   décision.
3. **Stratégie** (`sync-backup`, `sync-mirror`, `sync-bidirectional`, règles
   communes dans `sync-decisions`) : transforme les faits en un
   `ExecutionPlan` (`sync-plan`) avec un motif et des préconditions par
   opération, plus les garde-fous de suppression. Aucune E/S.
4. **Exécution** (`sync-executor`) : valide le plan, revérifie chaque
   précondition juste avant la mutation, copie (`sync-copy`) vers un temporaire,
   vérifie (`sync-verify`), conserve l'ancienne version (`sync-versioning`),
   publie, et journalise chaque étape (`sync-journal`).

La simulation et l'exécution utilisent le **même** plan.

## Crates

| Dossier | Crates | Rôle |
| --- | --- | --- |
| `crates/foundation` | `sync-core`, `sync-errors`, `sync-events`, `sync-plan` | types partagés (chemins logiques, identifiants typés, horodatages), erreurs classées et caviardées, événements de progression, modèle de plan versionné |
| `crates/adapters` | `connector-api`, `connector-local`, `connector-staging`, `connector-registry` | contrat `StorageConnector` (capacités détaillées, `NotSupported`), système de fichiers local, dépôt temporaire pour les envois distants, fabrique des connecteurs compilés |
| | `platform-api`, `platform-windows`, `platform-linux` | génération des tâches planifiées natives (XML Task Scheduler, unités systemd) |
| `crates/discovery` | `sync-filter`, `sync-scanner`, `sync-compare` | filtres, inventaire, comparaison |
| `crates/strategy` | `sync-decisions`, `sync-backup`, `sync-mirror`, `sync-bidirectional` | production des plans |
| `crates/execution` | `sync-copy`, `sync-verify`, `sync-versioning`, `sync-journal`, `sync-executor` | application sûre des plans |
| `crates/services` | `sync-profiles`, `sync-history`, `sync-scheduler`, `sync-credentials` | profils, baselines et historique, planifications (SQLite), port de jetons |
| `crates/application` | `sync-orchestrator`, `sync-session` | façade applicative (profils, simulation, exécution, verrou), cycle bidirectionnel avec baseline |
| `crates/connectors` | `connector-sftp`, `connector-webdav`, `connector-ftp`, `connector-ftps`, `connector-ftp-transport`, `connector-smb`, `connector-onedrive` | un crate par protocole, chacun derrière sa feature |
| `apps` | `syncengine-desktop`, `syncengine-cli`, `syncengine-agent` | interface iced, CLI `syncengine`, agent planifié |
| `tests` | `architecture`, `connector-memory` | règles de dépendances, connecteur mémoire pour simuler les pannes |

## Règles de frontière

- Les moteurs ne dépendent ni de l'interface ni d'un connecteur concret.
- Un connecteur n'a aucune règle de synchronisation ; il déclare ses capacités
  et renvoie `NotSupported` pour ce qu'il ne sait pas faire.
- Le scanner et la comparaison ne modifient rien ; la stratégie produit un plan ;
  seul l'exécuteur écrit.
- L'interface est une couche de présentation : elle passe uniquement par
  `sync-orchestrator`, sur des threads dédiés.
- Les spécificités Windows et Linux restent dans leurs adaptateurs.

Sous Windows, l'agent utilisateur reste résident ; Task Scheduler supervise
son processus, tandis que SQLite et le scheduler applicatif gèrent les horaires.
L'identité WTS de connexion est persistée (schéma scheduler v4). Le verrou OS
de présence, l'observation du processus et les reçus métier sont distincts.
Linux conserve les passages ponctuels systemd. Voir
[le cycle de vie et la migration](AGENT_RESIDENT.md).

Ces dépendances autorisées sont vérifiées par `tests/architecture` : un nouveau
crate doit y être déclaré.

## Persistance

Bases SQLite à migrations versionnées (`migrations/` de chaque crate) ; une
base plus récente, étrangère ou corrompue est refusée, jamais réinitialisée.
La baseline bidirectionnelle n'est publiée qu'après une exécution cohérente.
Les plans et journaux sont des formats JSON versionnés. Contenu du dossier
d'état : [Guide](GUIDE.md#données-de-lapplication).
