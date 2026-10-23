# Journal des modifications

## [0.1.0] — non publié

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
