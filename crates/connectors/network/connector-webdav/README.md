# Phase 08 — connecteur WebDAV expérimental

Feature `webdav` désactivée par défaut. `StorageConnector` en lecture seule par défaut :
listing, métadonnées, flux GET. `with_mutations()` active les mutations documentées
ci-dessous. Copie serveur et reprise non supportées.
La façade locale et la GUI ne proposent pas encore ce connecteur.

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

Déclarer `connector-webdav = { path = "…/connector-webdav", features = ["webdav"] }`.
Exemple d'intégration sans requête réseau exécutée en doctest. Appeler sur un
thread bloquant, hors runtime async. Authentification Bearer fournie par un
service externe ; pas de Basic, Digest, OAuth, rafraîchissement ou stockage système
implémenté ici. Portée requise : PROPFIND/GET sur la collection, sans écriture.
Renouvellement/révocation : recréer le connecteur avec le jeton courant ; 401
reste une erreur d'authentification, sans réessai automatique.

HTTPS obligatoire, TLS >= 1.2 via reqwest/native-tls (SChannel sous Windows,
OpenSSL sous Linux), validation système des certificats et noms d'hôte. Aucun
commutateur pour désactiver TLS, aucune redirection suivie, aucun proxy implicite,
cookie ni compression. Racine sans userinfo/query/fragment. En-tête Authorization
marqué sensible ; pas de Debug du connecteur ni sérialisation des secrets. Ni
les corps d'erreur ni les erreurs HTTP brutes contenant des URL ne sont exposés.
Cette protection ne promet pas l'effacement cryptographique de la mémoire.

PROPFIND Depth 0/1, XML DAV avec namespaces contrôlés et DTD refusée ; 8 Mio/XML
et 10000 enfants maximum. Propriétés absentes/invalides, doublons, réponse partielle,
href hors collection, autre origine, traversal et séparateurs encodés ⇒ erreur
du listing entier. La réponse doit inclure la collection demandée, même vide.
Chaque page contient au plus 256 entrées d'un instantané local trié ; une seule
énumération active par instance. Jetons liés à l'instance/répertoire, usage unique,
invalidés par tout nouveau listing. Pas de pagination serveur ni de snapshot DAV
atomique. Un serveur peut masquer des entrées sans que le client le détecte :
cohérence et casse déclarées inconnues. Le serveur doit confiner les alias/liens
côté backend ; WebDAV ne fournit pas lstat POSIX. Les dates restent inconnues
dans cette livraison (pas de supposition sur leur précision).

Flux GET borné en mémoire par les blocs de l'appelant. Content-Length, si présent,
contrôlé jusqu'à EOF ; réponse tronquée ⇒ erreur, jamais reprise à zéro. Un flux
sans taille ne fournit pas de preuve autonome de complétude : utiliser les
vérifications du moteur de copie. GET refusé pour collection ou type inconnu.
Le serveur peut modifier un fichier entre stat et GET ; pas d'ETag/If-Match
dans le contrat actuel. Utiliser un jeu distant stable pour les sauvegardes.

Trois tentatives maximum pour requêtes de lecture avant remise du flux ; 100/200 ms
de backoff. 429/503 honorent Retry-After numérique <= 5 s ; délai plus long,
HTTP-date ou absent ⇒ erreur `Throttled`, laissant le report à l'appelant sans
réessai prématuré. 401/403/404 non rejoués. Timeouts connexion 10 s / requête
30 s, durée totale des tentatives additive. Annulation coopérative entre étapes,
blocs et pas de backoff de 20 ms ; pas d'interruption immédiate d'une E/S bloquée.

Tests : `cargo test -p connector-webdav --features webdav`. Tests HTTPS isolés :
`python scripts/test_phase08_webdav.py` avec `cryptography` (installé avec
`paramiko==5.1.0`). Certificat et clé jetables sous `target`, jamais ajoutés aux
certificats système. Le client de production refuse la CA inconnue ; un client
privé du test approuve cette seule CA et lit ensuite le contenu en HTTPS.
Les autres tests utilisent HTTP **uniquement** par remplacement du client privé
dans un module `cfg(test)`, sans option publique ou feature autorisant HTTP.

Windows : MSVC et magasin de certificats système. Debian/Ubuntu :
`build-essential pkg-config libssl-dev`, runtime `libssl3t64` sur Ubuntu 24.04
(adapter au système ciblé) et `ca-certificates`. Linux et serveurs réels tiers
non validés dans cette session ; les deux connecteurs restent expérimentaux.

## Destinations et préconditions (clôture)

`with_mutations()` autorise MKCOL, dépôt PUT/MOVE et suppression conditionnelle de
fichiers. Spool disque local égal à la taille transférée, mémoire bornée. Répertoire
aléatoire réservé par MKCOL ; finish fait PUT avec If-None-Match:* puis rend le
staging lisible. MOVE avec Overwrite:F pour CreateNew ; pour ReplaceExisting,
ETag fort lu sur la cible puis condition DAV If étiquetée sur l'URL destination.
Serveur conforme requis ; ETag faible/absent => NotSupported pour ces mutations.
Pas de retry des mutations, même sur timeout : résultat incertain à diagnostiquer.

DELETE public limité aux fichiers avec If-Match. Collection utilisateur refusée,
car DELETE DAV est récursif. Cleanup seulement dans la réservation privée aléatoire
du writer : fichier supprimé conditionnellement, collection vérifiée vide puis
retirée avec ETag. Exclure les autres écrivains de cet espace temporaire. Un
échec/interruption peut laisser des collections temporaires, sans succès inventé.

Dates gérées par le serveur : SetModifiedTime absent ; sync-copy omet modified,
un appel direct avec date explicite est refusé. Comparaison avec hash recommandée.
Cohérence inconnue => aucune absence prouvée par le scanner. Le registre ne peut
attester une cohérence forte qu'après validation du serveur par l'appelant.
Tests HTTPS : PUT/MOVE, conflit, remplacement, abort/cleanup, certificat inconnu
refusé et suppression de collection utilisateur refusée.
