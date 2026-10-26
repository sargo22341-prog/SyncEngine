# Provenance et correctif local

Source : archive crates.io `cap-primitives 4.0.3`, dépôt
https://github.com/bytecodealliance/cap-std, révision
`b7acf8e8807fe3fab991884d2208b7e03d35a409` (voir `.cargo_vcs_info.json`).
SHA-256 de l’archive originale :
`8b5f74729fd2f44701d1a8eb47e906cdb3ccd9ec0f02baad85a744b791940b18`.
Licences upstream et copyright conservés intégralement. Le marqueur du cache
Cargo et le lockfile du workspace amont ne sont pas copiés.

Le changement de comportement concerne `src/windows/fs/get_path.rs` et son
helper `normalize_path.rs`. La suppression uniforme de quatre caractères dans
un chemin de handle `\\?\UNC\server\share` produisait `UNC\server\share`,
chemin relatif au répertoire courant. Listings, créations, renommages et
retraits utilisant cette conversion pouvaient échouer ou viser un chemin local
homonyme. Le correctif conserve la racine réseau : `\\server\share`.
Les chemins de lecteur gardent leur conversion habituelle ; les autres chemins
de périphérique et les unités UTF-16 non Unicode sont conservés.
L’import inutilisé `std::borrow::Cow` de `src/fs/manually/open.rs` est retiré
pour permettre les contrôles sans warnings de cette copie locale.

Les tests du helper exact sont inclus par `connector-local` sous Windows :
`cargo test -p connector-local --lib unc_path_regression`. Aucun accès réseau,
partage réel ou droit administrateur n’est nécessaire. Les cas couvrent racine
UNC, sous-dossier, Unicode, lecteur, périphérique et nom UTF-16 non Unicode.
Given un chemin UNC étendu, When sa forme usuelle est calculée, Then il reste
absolu et ne peut pas désigner un arbre `UNC` relatif au répertoire courant.

Le correctif UNC ne crée pas de nouvel `unsafe`.
La version 4.0.3 est inchangée. Le patch est également présent
dans le graphe Linux, mais aucun code Unix n’est modifié. Retirer ce patch
quand une version publiée corrige la conversion, après les tests de régression.
`cargo audit` ne contrôle pas cette dépendance à chemin local : surveiller les
avis RustSec de `cap-primitives` et `cap-std` tant que le patch est nécessaire.

## Listing Windows par lots (API opt-in)

`windows_listing::{ReadDir, Entry}` est une extension locale, utilisée uniquement
pour les inventaires exhaustifs avec réutilisation des poignées. Les API amont
restent inchangées. `large_read_dir.rs` utilise `FindFirstFileExW`,
`FindExInfoBasic` et `FIND_FIRST_EX_LARGE_FETCH`, avec repli sans le drapeau
seulement pour ERROR_INVALID_PARAMETER ou ERROR_NOT_SUPPORTED. Les métadonnées
proviennent du WIN32_FIND_DATAW, comme dans l'itérateur standard Windows.
`metadata_ext.rs` construit les attributs sans inventer d'identité de fichier.

Justification des appels FFI : l'itérateur standard Rust n'expose pas le drapeau
LARGE_FETCH. L'exception `allow(unsafe_code)` est limitée à ce module Windows
dans la dépendance vendue ; aucun moteur métier ne reçoit d'API unsafe.

Revue de sûreté interne : buffers typés initialisés et alignés, chemin UTF-16
terminé par NUL et vivant pendant l'appel, filtre nul, code d'erreur lu avant
tout autre appel Win32. Chaque handle réussi a un propriétaire RAII unique et
est fermé une seule fois par FindClose ; aucun entier opaque n'est déréférencé.
L'itération exige &mut, le handle du dossier reste ouvert jusqu'à la fin et le
curseur de recherche est détruit avant lui. Un échec de lecture invalide le
curseur ; seul ERROR_NO_MORE_FILES signifie une fin normale. Une première
recherche sans résultat exige une vérification fraîche du dossier. Les tags
name-surrogate restent des liens, y compris les jonctions ; les noms UTF-16
non Unicode sont préservés jusqu'au contrôle du connecteur. Cette revue n'est
pas une revue indépendante externe.

Tests : `cargo test -p cap-primitives --lib large_read_dir --locked`, exécuté
également en CI Windows, plus les contrats connector-local (pagination, racine
changée, liens, erreurs, métadonnées conservées après disparition du nom).
Le nouveau module se formate directement avec rustfmt, car vendor est exclu
de `cargo fmt --all`.

Références :
- https://raw.githubusercontent.com/rust-lang/rust/master/library/std/src/sys/fs/windows.rs
- https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-findfirstfileexw

## Lecture directe de la poignée (complément SMB)

La prévisualisation préfère désormais `HandleReadDir::new_without_file_ids`
(`FileFullDirectoryRestartInfo` puis `FileFullDirectoryInfo`). Le NAS réel
refuse la classe étendue précédente mais accepte cette classe sans identités,
inutiles pour construire l'inventaire. Le constructeur `new` précédent reste
disponible et testé. Mesures et comparaison : [audit NAS](../../docs/AUDIT_NAS_REEL.md).

Le même buffer aligné et le même parseur borné acceptent les deux formats ;
seul l'offset du nom et la présence du tag changent. Les champs fixes utilisés
précèdent EaSize et ont les mêmes offsets. FULL ne fournit pas le tag reparse :
pour ces seules entrées, `fs::stat` confiné avec `FollowSymlinks::No` récupère
la classification name-surrogate, y compris jonctions, sans suivre la cible.
Un échec invalide le listing ; aucun point reparse n'est deviné comme ordinaire.
La classification est reportée dans Entry, sans prétendre restituer le tag brut.
Les autres fichiers n'ont aucune requête de métadonnées individuelle.

`root_identity` compose l'ouverture standard Windows (droits de métadonnées,
BACKUP_SEMANTICS, partage lecture/écriture) et le wrapper sûr winx déjà employé
par cap-primitives pour `GetFileInformationByHandle`. La même requête vérifie
l'attribut répertoire et lit le couple volume/index. L'autorité ambiante est
explicite. Les tests comparent ce couple à celui du chemin historique et
vérifient le refus d'un fichier ou d'un nom absent. Aucun nouveau bloc unsafe
n'est nécessaire pour ces deux extensions.

Le connecteur possède jusqu'à seize curseurs indépendants, avec réservation
du chemin pendant la lecture : deux appels ne peuvent pas énumérer en parallèle
des duplicatas de la même poignée. Le mutex ne couvre pas les E/S ; chaque
curseur conserve sa poignée jusqu'à sa fin. La réservation se retire sur toute
erreur et est désarmée avant publication d'une continuation, sous le même verrou.
Le cache de dossiers reste borné à 64 poignées. Le scanner n'active que quatre
listings sur ce connecteur ; aucune mutation ne passe par ces raccourcis.

Le chemin normalisé obtenu par `get_path` peut entraîner une requête par
composant sur SMB. Le nouveau `HandleReadDir` préfère donc
`GetFileInformationByHandleEx(FileIdExtdDirectoryRestartInfo)` puis
`FileIdExtdDirectoryInfo`, sans convertir la poignée en chemin. Le buffer
est de 64 Kio ; noms, tailles, dates et tags reparse viennent du même lot.
Le lecteur FindFirstFileEx reste le repli pour ERROR_INVALID_FUNCTION,
ERROR_INVALID_PARAMETER ou ERROR_NOT_SUPPORTED lors du tout premier appel.
Aucun repli après une entrée publiée ou après une erreur réseau/d'accès.
Le connecteur mémorise ce refus de capacité pour éviter une sonde par dossier.

Justification et revue interne des trois nouveaux blocs unsafe :
- Le buffer Vec<u64> est initialisé, aligné sur huit octets et contient
  exactement 65 536 octets ; la poignée appartient au curseur. L'appel
  synchrone ne conserve aucun pointeur. Le même objet fichier ne doit pas
  être énuméré concurremment via une autre poignée dupliquée ; le connecteur
  sérialise ses pages et n'a qu'un curseur actif.
- GetLastError est capturé immédiatement après l'échec de l'API.
- La vue en octets n'est construite que sur la mémoire initialisée du Vec,
  ne dépasse pas sa longueur et ne survit pas à son emprunt. Le parsing est
  ensuite sûr : offsets croissants alignés, taille du nom paire et bornée,
  absence de NUL/séparateurs, taille de fichier non négative. Aucun cast vers
  une structure distante variable, aucune confiance dans ses longueurs.

Seul ERROR_NO_MORE_FILES signifie fin normale. Une réponse tronquée ou
malformée invalide le curseur. Les jonctions, symlinks et autres name-surrogate
restent des liens ; les tags cloud non substitutifs restent distincts.
L'identité de fichier renvoyée par le listing n'est pas utilisée comme
précondition d'exécution. Le tampon est réutilisé entre les appels.
Cette revue est interne, sans validation externe indépendante.

Tests : `cargo test -p cap-primitives --lib handle_read_dir --locked`,
contrats connector-local (1 500 fichiers, plusieurs lots, redémarrage,
identités et liens), et test de performance séparé :
`cargo test --release -p cap-primitives --lib handle_read_dir::tests::million_records_in_bounded_batches --locked -- --ignored --nocapture`.

Références Microsoft :
- https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew
- https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex
- https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_extd_dir_info

## Ouverture minimale des ancêtres pendant la copie

`readable_root_identity` garde la même requête d'identité que `root_identity`,
mais exige GENERIC_READ comme le chemin historique d'exécution. Le preview
conserve les droits de métadonnées et son cache sans changer le contrat d'écriture.

`open_directory_nofollow` ouvre un seul composant depuis une capacité parent,
avec OPEN_REPARSE_POINT et sans suivi. Il refuse composants ambigus, séparateurs,
flux alternatifs, NUL, point final et espace final. Une requête
FileAttributeTagInfo valide le type répertoire et rejette les tags substitutifs
avant de renvoyer la poignée. Le chemin complet est encore reparcouru à chaque
précondition et mutation ; aucun cache de preview n'autorise les écritures.
Le repli cap-primitives historique est limité aux trois erreurs explicites de
classe non supportée ; accès refusé et déconnexion restent des erreurs.

Revue interne du nouveau bloc unsafe : FILE_ATTRIBUTE_TAG_INFO est un objet
initialisé, aligné, de taille fixe, emprunté pendant un appel synchrone. La
poignée possédée reste ouverte, aucun pointeur ne sort de l'appel et aucun
buffer distant de taille variable n'est interprété. Le code ne renvoie la
poignée qu'après validation. Cette revue n'est pas une revue indépendante.
Tests : identité du dossier, fichiers ordinaires, jonctions sortantes, noms
ambigus et absence ; les tests de confinement et ACL existants sont conservés.

Mesure en lecture seule sur le NAS (300 ouvertures) : 657 µs par ouverture
historique contre 423 µs avec la requête minimale. Les droits de lecture et
partages lecture/écriture sont conservés, sans partage de suppression.

Références :
- https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_attribute_tag_info
- https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points-and-file-operations
