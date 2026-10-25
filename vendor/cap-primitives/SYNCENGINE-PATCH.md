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

Pas de repli vers des E/S ambiantes dans les moteurs ni de nouvel `unsafe`.
La version 4.0.3 et les API sont inchangées. Le patch est également présent
dans le graphe Linux, mais aucun code Unix n’est modifié. Retirer ce patch
quand une version publiée corrige la conversion, après les tests de régression.
`cargo audit` ne contrôle pas cette dépendance à chemin local : surveiller les
avis RustSec de `cap-primitives` et `cap-std` tant que le patch est nécessaire.
