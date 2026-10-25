# connector-sftp

Connecteur SFTP expérimental (libssh2), derrière la feature `sftp`. Lecture
seule par défaut ; `with_mutations()` active création de dossiers, dépôt et
suppression non récursive. Aucune règle de synchronisation.

```rust
use connector_sftp::{SftpConnector, SftpOptions};
use connector_api::StorageConnector;
use sync_core::{CancellationToken, ResourcePath};
# fn example() -> Result<(), Box<dyn std::error::Error>> {
let source = SftpConnector::new(SftpOptions {
    address: "192.0.2.1:22".parse()?,
    host: "backup.example".into(), username: "backup-reader".into(),
    known_hosts: "configuration/known_hosts".into(), root: "/backups".into(),
})?;
let cancel = CancellationToken::new();
let mut page = source.list(&ResourcePath::root(), None, &cancel)?;
while let Some(token) = page.next_page() {
    page = source.list(&ResourcePath::root(), Some(token), &cancel)?;
}
# Ok(()) }
```

- Authentification par l'agent SSH (`new`) ou un `SftpAuthentication` fourni
  par l'appelant (`with_authentication`). La clé d'hôte doit figurer dans
  `known_hosts` ; le fichier n'est jamais modifié.
- Liens jamais suivis ; pages de 256 entrées, 10 000 entrées au plus par
  dossier ; dates à la seconde, pas d'empreinte serveur.
- Appels bloquants (timeout 10 s, trois tentatives de connexion) : à placer
  sur un thread dédié.
- Remplacement non atomique : l'ancien fichier est déplacé sous
  `dépôt/previous` avant la publication du nouveau, jamais supprimé d'abord.
- Cohérence du listing inconnue : le scanner ne conclut à aucune absence.
- Une session de listing par connecteur est réutilisée après succès seulement.
  Une erreur l’invalide ; un échec transitoire peut recommencer le listing une
  fois sur une nouvelle session. Les mutations et les flux fichiers gardent
  leurs connexions propres ; aucune mutation n’est rejouée automatiquement.

Tests réseau : `python scripts/test_phase08_sftp.py` (serveur loopback éphémère).
