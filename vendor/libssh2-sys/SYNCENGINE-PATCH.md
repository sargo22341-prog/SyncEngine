# Provenance et correctif local

Source : crate crates.io `libssh2-sys 0.3.3` (libssh2 `1.11.1_DEV` embarqué),
dépôt https://github.com/alexcrichton/ssh2-rs ; hash Git source dans
`.cargo_vcs_info.json`. Licence upstream : MIT OR Apache-2.0 (crate), BSD-3-Clause
(libssh2, `libssh2/COPYING`), textes inclus dans ce répertoire.

Copie de l'archive crates.io, exclue des membres du workspace (dépendance
tierce), sans `libssh2/tests/` : suite de tests C upstream, non utilisée par
`build.rs`, qui contient des clés privées de test publiques. Aucun code SSH ou
cryptographique réécrit ici.

Correctif : `libssh2/src/wincng.c` uniquement (backend WinCNG, Windows), portage
du correctif upstream libssh2#2583 « wincng: normalize fixed-width DH values
before mpint encoding » (fusionné le 2026-09-08, après la sortie de 0.3.3).
CNG rend le secret partagé DH/ECDH et la clé publique DH sur une largeur fixe ;
quand l'octet de tête est nul (≈ 1 fois sur 256), libssh2 l'encodait en mpint
non canonique, le hash d'échange différait de celui du serveur et la connexion
échouait avec `Session(-8)` (LIBSSH2_ERROR_KEY_EXCHANGE_FAILURE). Linux
(backend OpenSSL) n'est pas concerné. Les blocs modifiés portent la mention
`SYNCENGINE-PATCH`.

Vérification : `scripts/test_phase08_sftp.py` sous Windows, 55 exécutions
consécutives sans échec (avant correctif : environ une exécution sur douze
échouait, chaque exécution ouvrant plusieurs dizaines de connexions).

`cargo audit` ne contrôle pas les dépendances à chemin local : surveiller les
avis RustSec de `libssh2-sys` et `ssh2` à la main tant que ce correctif existe.

À retirer dès qu'une version publiée de `libssh2-sys` embarque libssh2 avec
libssh2#2583, après répétition du test SFTP sous Windows.

SHA-256 archive crates.io originale : `0f5eb74291e8691cab524a01274a1b1e7742b1a94f29d8b101d8aadc8372c1cd`.
