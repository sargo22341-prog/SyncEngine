# connector-webdav

Connecteur WebDAV HTTPS expérimental, derrière la feature `webdav`. Lecture
seule par défaut ; `with_mutations()` active MKCOL, dépôt PUT/MOVE et
suppression conditionnelle de fichiers. Aucune règle de synchronisation.

```rust
use connector_webdav::{WebDavConnector, WebDavOptions};
use connector_api::StorageConnector;
use sync_core::{CancellationToken, ResourcePath};
# fn example(bearer_from_credential_service: &str) -> Result<(), Box<dyn std::error::Error>> {
let source = WebDavConnector::new(WebDavOptions {
    root: "https://backup.example/dav/".into(),
}, bearer_from_credential_service)?;
let cancel = CancellationToken::new();
let entry = source.stat(&ResourcePath::parse("archive.zip")?, &cancel)?;
let _metadata = entry.metadata();
# Ok(()) }
```

- HTTPS obligatoire (TLS ≥ 1.2, certificats système), aucune redirection,
  proxy implicite ni cookie. Jeton Bearer fourni par l'appelant ; un 401 n'est
  pas rejoué.
- PROPFIND Depth 0/1, XML limité à 8 Mio et 10 000 enfants ; toute réponse
  incohérente fait échouer le listing entier, jamais une page vide.
- Lectures : trois tentatives, `Retry-After` ≤ 5 s respecté, sinon `Throttled`.
  Les mutations ne sont jamais rejouées.
- Remplacements conditionnés par un ETag fort (sinon `NotSupported`) ; une
  collection utilisateur n'est jamais supprimée (DELETE DAV est récursif).
- Dates gérées par le serveur : comparer par contenu. Cohérence inconnue.

Tests réseau : `python scripts/test_phase08_webdav.py` (serveur HTTPS éphémère).
