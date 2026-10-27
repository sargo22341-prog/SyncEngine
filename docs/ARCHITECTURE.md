# Architecture

En unidirectionnel vers un récepteur explicitement insensible à la casse, la
comparaison peut projeter les variantes de dossiers dans un seul dossier cible.
Le plan de schéma 3 conserve les chemins physiques source ; l’exécuteur les
utilise en lecture et vérifie toutes leurs variantes avant une suppression.
Les conflits réels de fichiers restent bloqués individuellement. Le comparateur
bas niveau reste conservateur sans cette option, ainsi que le bidirectionnel.
Voir [le diagnostic et les mesures](CASSE_CHEMINS_SMB.md).

Les inventaires Windows utilisent un lecteur par poignée et lots de 64 Kio
dans l'adaptateur local de prévisualisation, sans normalisation de chemin ;
la classe FileFullDirectoryInfo évite de demander les identités des fichiers.
Les points reparse ont une lecture de métadonnées confinée sans suivi.
Le lecteur FindFirstFileEx reste le repli des serveurs incompatibles.
Le scanner peut lancer quatre listings indépendants et bornés si le connecteur
l'annonce ; les autres connecteurs gardent un listing à la fois.
Les moteurs restent indépendants de Win32. Voir [la mesure NAS](AUDIT_NAS_REEL.md)
et [l'audit SMB précédent](AUDIT_SMB_205S.md).
Contrats et limites : [audit des performances réseau](AUDIT_PERFORMANCE_RESEAU.md).

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

Les copies locales/SMB indépendantes peuvent utiliser un pool borné (quatre
workers demandés par défaut, huit annoncés par ces connecteurs, plafond moteur
de seize). Les autres connecteurs restent séquentiels tant qu'ils n'annoncent
pas explicitement la capacité. Le coordinateur libère les dépendances après
résultat : un dossier parent précède son contenu. Les suppressions attendent la
fin de tous les workers ; le bidirectionnel reste ordonné. Les remplacements
versionnés utilisent aussi le pool, borné par la capacité du magasin. Chaque
worker utilise les mêmes préconditions, temporaires et
vérifications de contenu ; le journal durable est sérialisé. Un jeton enfant
hérite de la pause/annulation et permet d'arrêter les autres workers sur erreur
fatale sans annuler le jeton de l'appelant. Voir [l'audit copie](AUDIT_COPIE_NAS.md).

La façade active en miroir l’actualisation de l’observation des fichiers déjà
prévus à la copie, juste avant leur lecture ; un changement pendant lecture
exige une vérification BLAKE3 supplémentaire du préfixe copié si les métadonnées
ont changé. Le transfert est borné à la taille fraîche, ce qui accepte un journal
en ajout continu ; une réécriture ou troncature du préfixe reste refusée. Les
autres modes, la destination et les suppressions restent
stricts. L’option moteur est explicite et désactivée par défaut hors façade.
Les E/S de vérification et d’archivage ont des compteurs informatifs séparés.
Le magasin et le connecteur revalident toujours les dossiers des versions et
chaque écriture effective ; aucun cache ne permet une mutation.

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

L’historique desktop migre de v1 à v2 avec un index dérivé des seuls problèmes,
dans la même transaction que les nouveaux rapports, et WAL pour les accès
concurrents. Un ancien rapport construit cet index une seule fois via TEMP,
sans réserver le verrou d’écriture pendant le parcours. Si la courte publication
du cache rencontre un autre écrivain, la projection TEMP reste consultable.
Les plans et rapports originaux restent la vérité ; la rétention supprime aussi
leurs index. Le contrôle d’intégrité complet a lieu à l’ouverture applicative,
pas à chaque filtre. Voir [les mesures et la migration](STABILITE_COPIE_HISTORIQUE.md).

La v3 ajoute `run_overview` : un résumé borné et la taille stockée de chaque
tentative, écrits par `begin`/`finish`. La liste affichée au démarrage ne lit
plus que cette table (une ligne sans résumé retombe sur le JSON original).

L’historique n’enregistre que les opérations porteuses d’un changement, d’une
erreur ou d’un avertissement : les entrées identiques (`keep`/`equivalent`) et
celles conservées en destination d’une sauvegarde (`keep`/`destination_only`)
sont seulement comptées dans `unchanged_omitted`. Une exécution sans changement
stocke `"operations": []`. Les résultats du rapport sont filtrés de la même
façon et restent alignés par position ; l’index des problèmes est construit
depuis le rapport complet. La baseline bidirectionnelle n’est pas recopiée
(elle vit dans `state.sqlite`). Le plan exécuté, lui, reste complet. JSON
compact. La rétention garde au plus 30 tentatives terminées par profil et,
au-delà des 3 plus récentes, retire les plus anciennes dès que plans et
rapports dépassent 512 Mio pour ce profil. À l’ouverture, les tentatives
terminées des profils supprimés sont retirées avec leurs journaux (les
interruptions restent), les journaux `run-….jsonl` qu’aucune tentative ne
référence sont supprimés, puis la base est compactée (`VACUUM`) si plus de
la moitié du fichier et au moins 256 Mio sont libres. Une base v3 est refusée
par une version antérieure. Mesure : `cargo run --release -p sync-orchestrator
--all-features --example overview_timing -- <COPIE_DU_DOSSIER_ETAT>`.

## Lectures réseau

Les analyses SMB utilisent maintenant le même cache borné de 64 poignées que
les analyses locales ; seuls les connecteurs dédiés au preview y accèdent.
Les connecteurs d’exécution gardent la résolution complète des chemins.
Les listings SFTP et FTP/FTPS réutilisent au plus une session par instance,
conservée uniquement après succès, avec revalidation des chemins et de la racine
FTP. Une erreur invalide la session ; seule la lecture complète d’un listing
transitoire peut être recommencée une fois. Les mutations ne sont pas rejouées.
Les avertissements de cohérence sont séparés dans les statistiques de présentation,
sans changer les inventaires incomplets ni les garde-fous miroir.
Les listings natifs SMB suivent désormais le contrat du système de fichiers :
une énumération achevée sans erreur permet une absence pour le plan, sans
promesse de snapshot atomique. Une erreur d’énumération, page perdue, omission ou annulation
reste incomplète et bloque les suppressions. Les connecteurs dont la cohérence
reste inconnue ou éventuelle gardent la politique conservatrice. L’exécuteur
revérifie chaque absence, y compris après l’archivage d’une version avant
suppression. Les collisions de noms demeurent des conflits explicites, jamais
des disparitions. Voir [les preuves et la comparaison FreeFileSync](ABSENCE_SMB_ET_CONFLITS.md).
Voir l’[audit réseau](AUDIT_RESEAU.md) pour le périmètre et les limites.

## Contrôle pause/reprise

Le jeton partagé de `sync-core` conserve une interrogation d’annulation
non bloquante et expose `checkpoint` aux workers synchrones. Une variable de
condition suspend les workers sans scrutation ; reprise et annulation les
réveillent. Aucun verrou de présentation ne doit être détenu pendant l’attente.
Les scanners, la copie, l’exécuteur et les lectures locales utilisent ces
points sûrs ; les sections de publication ne sont pas suspendues.

La façade expose `Application::set_paused` et publie `pause_requested` dans
`JobSnapshot`. Pour l’agent, la fenêtre écrit une requête remplacée par renommage
(`agent-pause.request`, tuple JSON identité de commande/booléen). Le watcher
l’applique seulement si son identité correspond à la commande active. Les
champs supplémentaires du snapshot ont des valeurs par défaut pour lire les
anciennes observations ; une ancienne observation sans identité n’offre pas
de bouton de pause actif. La requête ne constitue jamais une autorisation
d’écriture, une baseline ou un résultat durable.
