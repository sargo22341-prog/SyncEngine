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
