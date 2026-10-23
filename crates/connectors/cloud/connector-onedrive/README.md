# Source OneDrive — phase 09

Source **expérimentale en lecture seule**, Microsoft Graph global v1.0.
Code réel GET HTTPS, tests sur fixture HTTPS isolée ; aucun compte Microsoft
réel validé. Aucune destination OneDrive annoncée. Feature `onedrive` désactivée
par défaut ; aucune modification des profils persistants ou de la GUI.

## Utilisation Rust

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

Le registre propose aussi `connector_registry::onedrive::connect` avec sa feature
`onedrive`. Cette factory additive ne change pas les enums exhaustifs réseau
`Provider`/`Endpoint` ni leur liste `available()` ; elle retourne le même trait
`StorageConnector`. Pas de promotion de cohérence forte pour OneDrive.

## Authentification et réseau

L'application fournit un `AccessTokenProvider` lié à l'audience Graph et au
compte. Il acquiert/renouvelle/révoque les identifiants hors du connecteur,
borne son travail et utilise un magasin OS approprié si nécessaire.
`sync-credentials` est seulement ce port runtime : aucun login, coffre OS,
refresh token ou inscription Entra n'est simulé. Une intégration de bureau
peut utiliser un composant OAuth éprouvé avec Authorization Code + PKCE ;
aucun secret client ne doit être embarqué. Scope délégué minimal des opérations
livrées : `Files.Read` ; application : `Files.Read.All` selon les permissions
réellement accordées. Révocation/expiration ⇒ `Unauthenticated` sur 401, sans
nouvel essai de connexion automatique. Le provider est consulté à chaque GET Graph.

TLS >= 1.2, certificats système vérifiés, proxy implicite, compression,
redirections automatiques et retry interne reqwest désactivés.
Métadonnées uniquement sur `graph.microsoft.com/v1.0/drives/{drive_id}`.
Une étape par composant contrôle parent/drive/nom et refuse `remoteItem`,
éléments supprimés et packages. Les URL préauthentifiées sont obtenues par
GET `content` (302). Une seconde requête sans Authorization ni Referer ne vise
que les hôtes HTTPS exacts approuvés par l'appelant, port 443 en production.
Toute redirection supplémentaire est refusée. Aucun wildcard/hôte IP accepté.
Liste vide d'hôtes ⇒ capacités Read/StreamingRead absentes, lecture refusée.
Cette restriction peut refuser certains CDN Microsoft : valider leur identité
avant d'étendre la configuration, ne pas désactiver TLS. L'approbation des hôtes
et leur résolution DNS relèvent de l'administrateur ; pas de découverte automatique.

Les erreurs n'incorporent jamais corps fournisseur, jeton, URL signée ou erreur
HTTP brute. Les tokens n'ont ni Debug ni Serialize ; effacement mémoire non garanti.
Les applications ne doivent pas journaliser les secrets exposés au provider ou
activer des traces de transport susceptibles d'afficher des en-têtes/URL.

## Capacités et sûreté

- List/stat, flux Read si hôtes configurés, Cancellation coopérative et
  pagination serveur. Mutations héritées = `NotSupported`, sans réseau.
- Cohérence, casse et précision temporelle inconnues ; dates volontairement
  absentes et aucun hash natif exposé. Une date inconnue n'est pas une identité.
  `children` n'est pas un snapshot : le scanner garde l'absence inconnue et
  interdit les suppressions miroir. Pas de delta persistant ni de baseline cloud.
- Pages <= 1000 entrées, réponse JSON <= 8 Mio ; une énumération active par
  instance, <= 1000 pages/100000 entrées. Noms et ETag <= 1024 octets, chemins
  <= 4096 octets/256 composants, nextLink <= 8192 octets. Les ensembles de noms,
  identifiants et URL restent en mémoire jusqu'à la fin. Pas de borne globale
  du scanner ni validation de millions d'entrées.
- Jetons de page opaques, liés à l'instance/répertoire et consommés une fois ;
  nouveau listing invalide le précédent. Doublons de noms (minuscules Unicode)
  ou identifiants, parent étranger, page invalide ou lien extérieur ⇒ erreur,
  jamais page vide. Ce contrôle de casse n'est pas une normalisation universelle.
- GET Graph et ouverture téléchargement : trois tentatives maximum sur
  429/500/502/503/504, backoff exponentiel 1/2 s sans Retry-After. Délai numérique
  <= 2 s respecté ; délai supérieur ou HTTP-date remonté sans réessai précoce.
  401/403/404 distincts. Pas de retry transparent après remise du flux.
- Lecture bornée par le buffer appelant ; taille attendue et EOF vérifiés,
  relecture chemin/ID/ETag/taille à EOF. Changement ou flux court ⇒ erreur,
  lecteur ensuite inutilisable. L'appelant doit lire jusqu'à EOF avant publication.
  Le contrôle ETag dépend du service, ce n'est pas une preuve cryptographique.
- E/S synchrones sur worker, connexion 10 s et requête/flux 30 s, délais additifs
  par composant et tentative. Annulation entre appels/blocs, pas instantanée
  pendant TLS/syscall ou attente du mutex de pagination. Aucun état à reprendre
  automatiquement après interruption : recommencer un scan.

## Vérification et références

`cargo test -p connector-onedrive -p sync-credentials --all-features` ;
`target/phase08-fixtures/Scripts/python.exe scripts/test_phase09_onedrive.py`
sous Windows. Sous Linux, utiliser le Python du venv avec `cryptography`.
La fixture crée une CA et clé temporaires sous target et ne modifie pas le
magasin OS. La CA n'est ajoutée qu'au client privé du test ; aucune option de
production pour changer l'origine Graph ou désactiver les certificats.

Sources officielles vérifiées le 23 octobre 2026 :

- [Métadonnées et permissions](https://learn.microsoft.com/en-us/graph/api/driveitem-get?view=graph-rest-1.0).
- [Enfants et pagination](https://learn.microsoft.com/en-us/graph/api/driveitem-list-children?view=graph-rest-1.0).
- [Téléchargement et redirection](https://learn.microsoft.com/en-us/graph/api/driveitem-get-content?view=graph-rest-1.0).
- [Adressage et encodage](https://learn.microsoft.com/en-us/graph/onedrive-addressing-driveitems).
- [Limites de children et delta](https://learn.microsoft.com/en-us/graph/api/driveitem-delta?view=graph-rest-1.0).
- [Throttling](https://learn.microsoft.com/en-us/graph/throttling).
- [Authentification native](https://devblogs.microsoft.com/identity/app-types-and-auth-flows/).

Non validés : comptes Personal/Business réels, tenants et clouds souverains,
CDN réels, OAuth/consentement, quotas réels, Linux, concurrence hostile et charge.
La connexion testée sur fixture ne prouve pas une connexion Microsoft réelle.
Avant activation produit : revue d'intégration, recette sur tenant dédié et CI
Linux. Destination, delta et authentification interactive nécessitent une
nouvelle portée explicite ; aucune phase 10 commencée.
