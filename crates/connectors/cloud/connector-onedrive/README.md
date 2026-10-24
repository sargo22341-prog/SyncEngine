# connector-onedrive

Source OneDrive **expérimentale en lecture seule** (Microsoft Graph v1.0),
derrière la feature `onedrive`. Toute mutation renvoie `NotSupported`.

```no_run
use std::sync::Arc;
use connector_api::StorageConnector;
use connector_onedrive::{OneDriveConnector, OneDriveOptions};
use sync_core::{CancellationToken, ResourcePath};
use sync_credentials::AccessTokenProvider;

fn inspect(tokens: Arc<dyn AccessTokenProvider>, drive_id: String,
           approved_download_hosts: Vec<String>) -> Result<(), connector_api::StorageError> {
    let cancel = CancellationToken::new();
    let source = OneDriveConnector::new(OneDriveOptions {
        drive_id, download_hosts: approved_download_hosts,
    }, tokens)?;
    source.connect(&cancel)?; // GET réel, new() seul n'authentifie rien
    let root = ResourcePath::root();
    let mut page = source.list(&root, None, &cancel)?;
    while let Some(next) = page.next_page() {
        page = source.list(&root, Some(next), &cancel)?;
    }
    Ok(())
}
```

- Le jeton vient d'un `AccessTokenProvider` fourni par l'application, consulté
  à chaque requête ; aucun OAuth intégré. Scope minimal : `Files.Read`.
- Métadonnées sur `graph.microsoft.com` uniquement. Les téléchargements ne
  visent que les hôtes HTTPS exacts approuvés par l'appelant, sans en-tête
  d'autorisation ; liste vide ⇒ lecture refusée.
- Dates et empreintes non exposées, cohérence inconnue : le miroir ne supprime
  rien à partir de cette source.
- Retry sur 429/5xx (trois tentatives, backoff 1/2 s) ; taille, ETag et EOF
  contrôlés à la lecture.
- Validé seulement sur une fixture HTTPS locale, pas sur un vrai compte.

Tests : `python scripts/test_phase09_onedrive.py`.
