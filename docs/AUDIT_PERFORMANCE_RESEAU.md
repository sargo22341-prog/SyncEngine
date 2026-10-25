# Inventaires réseau sans empreintes — 25 octobre 2026

Complément après le retour « 205 s contre 60 s » :
[audit SMB ciblé et lecture directe par poignée](AUDIT_SMB_205S.md).
Les résultats et limites ci-dessous décrivent la première livraison 300ab55.

Périmètre confirmé : « Simulation : lecture des dossiers », zéro empreinte.
Aucun accès au NAS ni mesure sur ses données. L'objectif de gain ×2 à ×5 reste
à vérifier par l'utilisateur. Le temps de lecture des contenus et la copie ne
sont pas mesurés par cet audit. L'audit précédent : [AUDIT_RESEAU.md](AUDIT_RESEAU.md).

## Comparaison avec la référence fournie

Sources lues dans `F:/Project/referance/syncengine/FreeFileSync` :
`Source/afs/native.cpp`, `Source/base/parallel_scan.cpp`, `Source/afs/sftp.cpp`
et `Source/afs/ftp.cpp`, sous le dossier FreeFileSync. La variante native fournie
est POSIX : elle ne prouve pas quel appel Windows utilise FreeFileSync.

| Chemin | Référence observée | Écart traité dans SyncEngine |
| --- | --- | --- |
| Natif/réseau monté | Listing puis exploitation des attributs ; un worker par appareil dans cette source | Listing Windows LARGE_FETCH opt-in, cache borné de 64 dossiers, contrôle no-follow unique par ouverture et suppression du garde SMB Windows doublant le contrôle local |
| SFTP/SSH | Attributs renvoyés par readdir, gestionnaire de sessions saines | Listing sans stat par fichier déjà présent ; session maintenant réutilisée également pour stat, abandon après erreur et une reprise transitoire maximum |
| FTP/FTPS | MLSD et opérations ciblées de métadonnées, sessions réutilisées | MLST ciblé remplace la relecture systématique du parent pour stat ; fallback complet conservateur, dates modify validées |
| WebDAV | Protocole différent, pas d'équivalence native revendiquée | getlastmodified demandé avec les propriétés Depth:1 et exploité sans requête supplémentaire |
| OneDrive | Pas de connecteur Microsoft Graph équivalent dans les fichiers étudiés | lastModifiedDateTime du listing désormais conservé ; pagination, ancrage et retries existants gardés |

Il ne s'agit pas d'une transplantation du code FreeFileSync. Les contrats de
confinement, d'incertitude et de plans de SyncEngine restent applicables.
Le listing standard Rust Windows utilisait déjà les métadonnées de
WIN32_FIND_DATAW : l'ancien listing n'ouvrait pas chaque fichier pour son stat.
En revanche Rust désactive volontairement LARGE_FETCH dans son itérateur
généraliste. Notre inventaire exhaustif peut demander ce drapeau. Ce choix
réduit potentiellement les échanges de listing SMB, sans garantir un facteur.

## Garde-fous

Le lecteur natif n'est actif que pour le connecteur local de preview, dont SMB
réutilise l'implémentation. La copie, le stat précédant une mutation et les
préconditions de publication ne bénéficient pas d'un contrôle allégé.
Une continuation conserve le même curseur possédé et confiné ; la dernière
page multilot revérifie la disponibilité et l'identité de la racine. Un échec
ne devient ni dossier vide ni autorisation de suppression. Un nom ambigu,
une jonction ou un nom non représentable garde le traitement de sécurité
existant. Le drapeau LARGE_FETCH n'est retiré que si explicitement refusé.
La justification FFI et sa revue interne se trouvent dans
[le correctif vendored](../vendor/cap-primitives/SYNCENGINE-PATCH.md).

FTP ne confond pas 550 avec un fichier absent : ce code peut signifier accès
refusé. Si MLST échoue ainsi, un listing complet du parent est nécessaire.
Les mutations ne sont jamais rejouées par ces reprises de métadonnées.
chrono, déjà dans Cargo.lock, est utilisé sans ses fonctionnalités par défaut
pour parser les dates UTC ; aucun nouvel algorithme TLS/SSH n'est ajouté.

## Mesure et validation

`cargo run --release -p sync-orchestrator --example scan_handles_timing --locked` :
arbre temporaire de 1 744 entrées, quatre tours en ordre alterné, inventaires
comparés sans empreintes. Moyennes : 92,5 ms sans cache, 37,0 ms avec le parcours
optimisé (×2,50). Le cache existait déjà avant ce complément : cette mesure ne
doit donc pas être présentée comme un gain additionnel ×2,50 sur la précédente
version test. Sa mesure antérieure avec cache était environ 41,6 ms, sur une
autre exécution ; ce petit écart local n'est pas un benchmark SMB contrôlé.

Commandes de vérification Windows :

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
$env:RUSTFLAGS='-D warnings'
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo test -p cap-primitives --lib large_read_dir --locked
target/network-audit-env/Scripts/python.exe scripts/test_phase08_sftp.py
target/network-audit-env/Scripts/python.exe scripts/test_phase08_ftp.py
target/network-audit-env/Scripts/python.exe scripts/test_phase08_webdav.py
target/network-audit-env/Scripts/python.exe scripts/test_phase09_onedrive.py
```

Les fixtures sont temporaires et les serveurs sur localhost. Aucun compte réel
ni partage personnel. Résultats : fmt, Clippy avec warnings interdits et check
avec RUSTFLAGS passent ; 518 tests workspace passent, zéro échec et neuf tests
réseau ignorés par défaut, tous exécutés avec succès par les scripts isolés.
Les trois tests supplémentaires du lecteur natif vendored passent également.
Linux n'est pas compilé ou exécuté dans cette session ;
la CI doit le vérifier. Les tests couvrent racine remplacée en cours de
pagination, noms Unicode/non Unicode, attributs de liens Windows, métadonnées
de listing conservées après disparition du fichier, dates invalides, session
réutilisée, déconnexion, erreurs et refus de mutations selon les contrats.

## Limites restantes

- Pas de profilage du NAS demandé : la cause exacte des minutes sur ce partage
  n'est donc pas démontrée ; antivirus, latence, droits et structure restent
  des facteurs possibles à mesurer, sans les désigner comme responsables.
- Aucun essai sur un million d'entrées n'est réalisé ; le benchmark local ne
  valide pas la consommation mémoire ou les temps à cette échelle.
- Pas de parallélisme illimité ajouté ; la référence fournie n'en fait pas
  une justification. Le listing SFTP/FTP/DAV reste limité à 10 000 éléments
  par dossier et échoue explicitement au-delà, comme documenté auparavant.
- Pas de pool de flux de contenu SFTP/FTP ni de benchmark de copie ajouté.
  Les ouvertures de contenu peuvent encore établir une session dédiée.
- Les horodatages absents restent inconnus ; aucune date n'est inventée.
  Aucun fournisseur non implémenté n'est présenté comme disponible.

Fichiers concernés : connecteurs local/SMB (confinement, pagination, tests),
transport FTP, SFTP, parsers WebDAV/OneDrive et leurs manifestes, dépendances
workspace/lock, extension Windows cap-primitives et sa provenance, étape CI
Windows, notes de version et documentation liée. Les moteurs de comparaison,
stratégie, copie et exécution ne changent pas dans ce complément.
