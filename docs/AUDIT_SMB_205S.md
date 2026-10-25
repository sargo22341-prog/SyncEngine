# Audit ciblé : 205 secondes contre 60 secondes

Retour utilisateur après la précédente livraison : lecture des dossiers,
zéro empreinte, environ 205 s dans SyncEngine et 60 s dans FreeFileSync.
Le rapport est un point de départ, pas une mesure instrumentée du NAS.
Aucun accès au NAS, aucun réglage SMB de la machine, aucun contenu personnel
modifié pendant ce complément.

## Ce que la référence permet réellement de vérifier

Fichiers relus dans les sources FreeFileSync fournies :
`FreeFileSync/Source/afs/native.cpp` (getDirContentFlat, getItemDetails,
SingleFolderTraverser, NativeFileSystem::traverseFolderRecursive),
`FreeFileSync/Source/base/parallel_scan.cpp` (parallelFolderScan),
`zen/file_traverser.cpp` et `zen/file_access.cpp`.

Le parcours natif fourni travaille à partir de chemins déjà connus :
opendir/readdir, puis lstat pour les attributs dans cette variante POSIX.
Il accumule les sous-dossiers dans une pile, sans reconstruire un chemin
normalisé depuis une poignée à chaque descente. Le fichier parallel_scan.cpp
regroupe les chemins par appareil et lance un worker par appareil, avec
parallelOps = 1 dans ce code. SyncEngine utilise déjà une pile et un thread
par côté ; multiplier aveuglément les connexions n'est donc pas une méthode
justifiée par cette référence.

Cette archive contient l'implémentation native POSIX, pas celle de Windows.
Il serait inexact d'affirmer qu'on y a trouvé l'appel Win32 précis utilisé
par FreeFileSync sous Windows. Aucun code C++ du projet n'est recopié.

## Connexion et listing SMB dans SyncEngine

Un chemin UNC est traité par le client SMB de Windows, via ses API fichiers.
SyncEngine ne crée pas une session TCP/SMB ou une authentification par fichier.
Les ouvertures de poignées sont des opérations de fichiers, distinctes d'une
nouvelle connexion authentifiée. Le nom du NAS et son adresse IP aboutissent
au même parcours de connecteur ; changer uniquement le nom ne corrige donc
pas le coût algorithmique trouvé ici.

Les scans SMB passent par SmbConnector::for_scan, puis le lecteur local
Windows avec cache de dossiers. L'exécution reconstruit ses propres connecteurs
avec les vérifications complètes des préconditions. Les identifiants, options
de signature, chiffrement, droits et paramètres du NAS ne sont pas modifiés.

## Coûts manqués lors du précédent audit

### Conversion poignée → chemin normalisé

L'ancien lecteur de preview demandait get_path(directory) avant FindFirstFileEx.
Ce helper appelle winx::file::get_file_path, qui appelle
GetFinalPathNameByHandleW avec flags = 0, donc FILE_NAME_NORMALIZED.
Microsoft documente que, sur SMB, cette normalisation peut résoudre chaque
composant du chemin séparément et échouer si un ancêtre est inaccessible.
Une opération apparemment unique peut donc masquer plusieurs requêtes.

Source primaire : [GetFinalPathNameByHandleW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew).
Les serveurs récents peuvent supporter la requête normalisée directement :
l'amplification exacte dépend du serveur et n'est pas mesurée ici.

Correction : le nouveau lecteur utilise directement la poignée déjà ouverte
et confinée. GetFileInformationByHandleEx, avec FileIdExtdDirectoryRestartInfo
puis FileIdExtdDirectoryInfo, récupère les entrées par blocs de 64 Kio.
Noms, attributs, tailles, dates et tags de jonctions/symlinks proviennent
du même lot. Il ne convertit plus la poignée en chemin et n'ouvre pas un
second curseur de recherche par nom. Aucun stat par fichier n'est ajouté.

API : [GetFileInformationByHandleEx](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex),
[FILE_ID_EXTD_DIR_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info).

### Métadonnées immuables de la racine épinglée

L'ancien contrôle relisait à chaque dossier l'identité de la racine courante
ET celle de la poignée racine déjà conservée. L'identité d'un objet épinglé
est mémorisée à la construction ; le contrôle continue de lire celle de la
racine actuellement désignée par le chemin. On retire ainsi une requête
redondante par contrôle, sans autoriser une racine remplacée ou disparue.

Sur le grand arbre de test de 2 113 dossiers, le chemin direct évite 2 113
conversions normalisées et environ 2 112 relectures de l'identité épinglée.
Ce sont des nombres d'opérations du code, pas une capture de paquets SMB.

## Sécurité et compatibilité

- Seule la prévisualisation Windows choisit le nouveau lecteur. Linux garde
  son lecteur habituel ; le cache d'identité de la racine s'applique aussi
  au connecteur local Linux. FTP/SFTP/WebDAV/OneDrive ne sont pas modifiés.
  Les règles de préconditions de mutation restent identiques.
- Un refus explicite de classe d'information lors du premier appel déclenche
  le repli existant FindFirstFileEx/LARGE_FETCH. Le connecteur mémorise cette
  incapacité pour éviter une sonde par dossier. Ce repli conserve le coût de
  normalisation ; aucun gain équivalent n'est promis sur un serveur ancien.
- Une erreur d'accès, une déconnexion, une réponse malformée ou une erreur
  après publication d'une entrée ne déclenche pas de redémarrage silencieux.
  Un listing échoué reste incomplet, jamais vide.
- La pagination utilise le même curseur possédé ; les lots sont bornés,
  les offsets/noms sont validés avant lecture. Les reparse tags sont conservés.
- La racine courante reste vérifiée à chaque nouveau dossier et à la fin
  des listings multipages. Une indisponibilité ne prouve aucune suppression.
- La pause/annulation reste contrôlée entre les entrées/pages ; un appel SMB
  synchrone en cours attend toujours le retour de Windows.

Revue interne des appels FFI, limites de partage d'une poignée dupliquée et
provenance : [SYNCENGINE-PATCH.md](../vendor/cap-primitives/SYNCENGINE-PATCH.md).

## Mesures reproductibles et vérifications

Arbres temporaires locaux uniquement, builds release, inventaires complets
comparés pour vérifier que les mêmes noms et métadonnées sont retournés.
Le benchmark effectue quatre passages en ordre alterné cache/sans cache,
ignore le premier pour les moyennes et n'effectue aucune empreinte.

```powershell
cargo run --release -p sync-orchestrator --example scan_handles_timing --locked
cargo run --release -p sync-orchestrator --example scan_handles_timing --locked -- 64 32 64
cargo test --release -p cap-primitives --lib handle_read_dir::tests::million_records_in_bounded_batches --locked -- --ignored --nocapture
```

Petit arbre de 1 744 entrées : précédent parcours avec cache environ 37 ms,
nouveau parcours environ 20,2 ms. Grand arbre de 133 184 entrées : nouveau
parcours avec cache 447,7 ms contre 2 120 ms sans cache. Ce ratio ×4,74 ne
représente PAS le gain additionnel sur la précédente version, qui avait déjà
un cache. Après recompilation des deux fichiers du lecteur du commit 300ab55,
le même benchmark donne 622,5 ms avec cache et 2 125,2 ms sans cache.
Le gain additionnel observé est donc environ 28 % sur cet arbre local
(622,5 → 447,7 ms, ×1,39), avec des mesures sans cache très proches.
Les sources courantes ont été restaurées automatiquement après la comparaison.

Le parseur traite aussi un million d'enregistrements synthétiques par lots de
48 000 octets : 36 ms dans cet essai release. Cela valide ce traitement borné,
pas la vitesse d'un million de fichiers réels ni la mémoire totale du scanner.

Contrôles : fmt, Clippy avec -D warnings, check avec RUSTFLAGS=-D warnings,
tests workspace all-features, tests vendored large_read_dir et handle_read_dir.
Les nouveaux tests couvrent tailles >4 Gio, dates, UTF-16 non Unicode, jonctions,
offsets/noms corrompus, erreurs exclues du repli, plusieurs buffers natifs et
redémarrage d'énumération. Les autres contrats de fichiers temporaires couvrent
toujours accès refusé, pagination, liens et refus des mutations dangereuses.
Linux et le NAS réel ne sont pas testés dans cette session.

Résultats Windows : 519 tests workspace réussis, zéro échec, neuf tests réseau
ignorés par défaut puis tous exécutés avec succès sur les fixtures localhost
SFTP, FTP/FTPS, WebDAV et OneDrive. Les six tests unitaires vendored passent,
ainsi que le test séparé du million d'enregistrements synthétiques. Fmt,
Clippy avec warnings interdits et check avec RUSTFLAGS=-D warnings passent.
Les commandes réellement lancées sont celles du bloc de mesure ci-dessus,
`cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
`cargo check --workspace --all-targets --all-features --locked` avec RUSTFLAGS,
`cargo test --workspace --all-features --locked`, les deux commandes de tests
vendored de SYNCENGINE-PATCH.md et les quatre scripts
`scripts/test_phase08_{sftp,ftp,webdav}.py`, `scripts/test_phase09_onedrive.py`.

## Fichiers concernés

- connector-local/src/connector.rs : identité de la racine épinglée.
- connector-local/src/paging.rs : choix du lecteur et refus de capacité mémorisé.
- connector-local/tests/local.rs : métadonnées, plusieurs buffers, redémarrage.
- sync-orchestrator/examples/scan_handles_timing.rs : dimensions configurables.
- vendor/cap-primitives : nouveau lecteur Windows, visibilité de Entry,
  réexport et documentation de sûreté ; aucune API amont remplacée.
- CI Windows : tests du lecteur ajoutés ; notes, changelog et documentation.

Le critère restant à vérifier sur la nouvelle version est le temps de lecture
du même profil, à zéro empreinte, face aux 205 s précédents et aux 60 s de
FreeFileSync. L'économie exacte de 145 s ne peut pas être annoncée sans cette
mesure réelle et sans vérifier que le serveur accepte le chemin direct.
