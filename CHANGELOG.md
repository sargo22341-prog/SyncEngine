# Journal des modifications

## [0.1.0] — non publié

### Ajouté (phase 08 — sources réseau expérimentales)
- Complément de clôture : destinations SFTP/WebDAV, providers FTP/FTPS séparés,
  adaptateur SMB natif (UNC Windows ; CIFS Linux lecture seule) et registre statique.
- Dépôt finalisé avant vérification, spool disque borné en mémoire, original
  préservé avant remplacement non atomique ; erreurs/cleanup/interruption explicites.
- Cohérence inconnue/éventuelle => inventaire incomplet, aucune absence prouvée
  ni suppression miroir. Option de cohérence forte soumise à validation du serveur.
- SuppaFTP 12.2.1 vendored avec correction des traces PASS/réponses ; test de
  non-divulgation avec capture TRACE, sans désactivation globale de la verbosité.
- GUI/profils locaux inchangés, pas de cloud ou phase suivante.

Première livraison de cette phase (complétée ci-dessus) :
- Crates SFTP et WebDAV indépendants, features désactivées par défaut,
  listing paginé, stat et lecture en flux ; erreurs et annulation explicites.
- Vérification SSH known_hosts avant authentification injectée ; WebDAV HTTPS
  TLS >= 1.2 sans redirections, secrets absents des options et diagnostics.
- Tests transport et contrats SSH/SFTP/HTTPS sur serveurs loopback éphémères,
  ajoutés au workflow CI. Aucune destination réseau ou mutation distante,
  aucun profil/GUI réseau ; FTP, FTPS et SMB restent à leurs sous-sessions.

### Ajouté (phase 07 — scheduler et agent)
- Agent Rust sans fenêtre ni dépendance GUI, réutilisant la façade et les moteurs
  locaux ; autorisation explicite liée au profil, suppressions séparément approuvées.
- Scheduler SQLite v1 : intervalles/horaires UTC, événements de session selon OS,
  échéances manquées regroupées ou ignorées, claims durables et diagnostic de reprise.
- Adaptateurs Windows Task Scheduler et Linux systemd utilisateur : export natif
  inspectable, installation explicite, sans promesse d'achèvement au logoff.
- Verrou OS commun aux exécutions de la façade, annulation persistante et
  reconnaissance explicite des interruptions avec conservation de la baseline.

### Ajouté (phase 06 — interface locale)
- Desktop Tauri 2/TypeScript : profils/groupes, assistant, différences filtrées
  et paginées, simulation distincte des écritures, confirmations destructives,
  progression agrégée, annulation, historique détaillé et affichage compact.
- `sync-profiles` : configuration SQLite v1 validée et préférences.
- `sync-orchestrator` : façade des trois modes locaux, consommation unique du
  plan prévisualisé, versions systématiques, intentions/résultats durables,
  exclusion des jobs desktop concurrents et verrou du répertoire d’état.
- CI préparée pour construire le frontend avant les contrôles Rust et installer
  les prérequis Tauri Ubuntu. Aucun agent ni connecteur distant ajouté.

### Ajouté (phase 05 — synchronisation bidirectionnelle)
- `sync-bidirectional` : décisions par baseline et BLAKE3 dans les deux sens ;
  conflits préservés en place, tombstones sans expiration, aucune suppression
  déduite sans baseline ni comparaison sur timestamps seuls.
- `sync-history` : SQLite embarqué, migration transactionnelle, historique daté,
  rétention configurable, backup cohérent, refus des bases corrompues/étrangères,
  réservation des profils et racines identiques dans une même base.
- `sync-session` : preview/exécution du même plan, rescan avant publication
  atomique de baseline, diagnostic et replanification après interruption ;
  exemple local réel avec versions systématiques et simulation par défaut.

### Modifié (phase 05)
- Plan v2 bidirectionnel avec direction et preuve baseline ; plans v1 et
  constructeurs backup/mirror conservés, aucun instantané JSON régénéré.
- Exécuteur : capacités directionnelles, relecture des empreintes avant
  mutations bidirectionnelles ; journal/rapport identifient le receveur.
- Champ public `InFlight.target` ajouté (littéraux externes à adapter) ;
  nouveau type `OperationDirection` rejeté par les anciens lecteurs de journal.
  Revue d'intégration demandée dans l'ADR 0005.

### Ajouté (phase 04 — exécution sécurisée locale)
- `sync-copy`, `sync-verify`, `sync-versioning`, `sync-journal`, `sync-executor` :
  transferts en flux, vérification BLAKE3 par défaut, versions facultatives,
  journal JSON Lines v1 durable et diagnostic après interruption.
- Exécution du plan de preview avec préconditions revérifiées, refus motivés,
  dépendances respectées, suppressions non récursives en dernier, progression
  agrégée et annulation coopérative dans les moteurs.
- Arrêt sur disque plein, déconnexion, annulation ou journal indisponible.
- CLI `execute` (`--confirm`, nouveau `--journal`, `--confirm-delete` si
  nécessaire, `--plan`, `--verify`, `--versions-dir`, `--progress`, `--json`)
  et `journal` ; journal/versions refusés dans les arbres synchronisés.

### Modifié (phase 04)
- `StorageConnector` : `create_directory`, `open_write`, `delete`, avec
  défaut `NotSupported` ; écriture `ContentWriter` en deux temps.
- Connecteur local : mutations sur activation explicite, temporaire + fsync,
  date source préservée, contrôle de la position réelle du parent renforcé
  (fenêtre résiduelle documentée dans l'ADR 0004, revue demandée).
- Double mémoire : mutations et pannes injectables ; règle d'architecture
  interdisant les dépendances de l'exécution vers les stratégies/décisions.
- Aide CLI et deux instantanés texte de preview : seule la dernière ligne
  est régénérée car l'exécution existe désormais.
- CLI : un journal situé à côté de la destination est accepté.

### Ajouté (phase 03 — décisions et plans, aucune mutation)
- `sync-plan` : plan d'exécution immuable, sérialisable en JSON versionné
  (`schema_version` 1) et validé au chargement ; opérations `copy`, `replace`,
  `keep`, `delete`, `conflict`, `skip` avec motif, risque, préconditions,
  dépendances et observations ; politiques d'écrasement et de suppression ;
  garde-fous de suppression (inventaires complets, source non vide, plafonds).
- `sync-decisions` : contrat `PlanStrategy`, règle commune des modes
  unidirectionnels, passes de sûreté (parent indisponible, répertoire non vidé,
  suppressions refusées en bloc).
- `sync-backup` : sauvegarde (jamais de suppression ; conflit si la destination
  est plus récente, par défaut).
- `sync-mirror` : miroir (suppressions seulement sur autorisation explicite).
- `sync-core` : `PlanId`. `sync-compare` : `ComparisonReport::filter_rules`,
  `left_connector`, `right_connector`.
- CLI : commande `preview` (texte ou `--json`), refus d'une destination
  imbriquée dans la source. Rien n'est exécuté.

### Modifié (phase 03)
- CLI : l'aide décrit la commande `preview` et ses codes de sortie ; les tests
  d'intégration partagent leurs aides (`apps/cli/tests/common`).

### Ajouté (phase 02 — scan et comparaison, lecture seule)
- `connector-api` : opérations `list` (paginée), `stat`, `open_read` (flux),
  `ListPage`, `PageToken`, `ListingIssue` ; modèle synchrone, annulation coopérative.
- `sync-core` : `CancellationToken`, `ContentHash`, empreinte optionnelle dans `FileMetadata`.
- `connector-local` : système de fichiers local en lecture seule (liens et jonctions
  non suivis, noms ambigus Win32 refusés, racine absente = `Disconnected`).
- `sync-filter` : règles d'inclusion et d'exclusion par globs, sérialisables.
- `sync-scanner` : inventaire à couverture explicite (répertoires incomplets,
  présence inconnue), empreintes BLAKE3 en flux, annulation avec inventaire partiel.
- `sync-compare` : constats gauche/droite, équivalences, différences, incertitudes
  (scan partiel, précision des dates, collisions de casse).
- CLI : commandes `scan` et `compare` en lecture seule.

### Modifié (phase 02)
- `StorageConnector` : trois méthodes obligatoires ajoutées (changement d'API, ADR 0002).
- CLI : une sortie standard inutilisable renvoie désormais le code 4 (au lieu de 1) ;
  le code 1 signifie « différences constatées ».

### Ajouté (phase 01 — fondations)
- Cargo Workspace (édition 2024, chaîne 1.98.1 figée, lints stricts).
- `sync-core` : identifiants typés, `ResourcePath`, `Timestamp`/`TimestampPrecision`,
  `FileEntry`/`FileMetadata`/`EntryKind`, `HashAlgorithm`, `ExecutionStatus`.
- `sync-errors` : `ErrorClass`, `Severity`, `ErrorContext`, `ClassifiedError`,
  `ErrorReport`, `redact_secrets`.
- `sync-events` : `Event`, `EventKind`, `ProgressSnapshot`, `EventSink`,
  `BoundedEventSink`, `EventEmitter`.
- `connector-api` : `StorageCapabilities`, `StorageFeature`, `StorageError`,
  `StorageErrorKind`, `StorageOperation`, trait `StorageConnector`, `ensure_supported`.
- CLI `syncengine` : commandes `help` et `version` uniquement.
- Tests d'architecture (direction des dépendances), CI Windows/Linux, ADR 0001.
