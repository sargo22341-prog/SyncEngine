# Journal des modifications

## [0.1.0] — non publié

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
