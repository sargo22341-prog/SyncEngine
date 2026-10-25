# Simulation NAS réelle — 25 octobre 2026

Objectif : atteindre environ 60 secondes sans dégrader le miroir. Partage
fourni : `\\NAS\testsync`. Toutes les opérations sur ce partage ont été des
lectures ; aucun transfert, écrasement ou retrait n'y a été exécuté.
Destination de simulation : dossier vide isolé sous `target/`.
Comparaison par tailles/dates, aucun calcul d'empreinte de contenu sur le NAS.

## Résultats et périmètre

| Version mesurée, release Windows MSVC x86_64 | Scan | Comparaison | Plan |
| --- | ---: | ---: | ---: |
| Départ, lecteur étendu refusé, repli FindFirst | 224,36 s | 349 ms | 485 ms |
| Lots compatibles sans identifiants | 156,60 s | 280 ms | 587 ms |
| Même lecteur et contrôle d'identité en une requête | 99,52 s | 276 ms | 485 ms |
| Quatre workers, attente par lots (essai intermédiaire retiré) | 75,66 s | 271 ms | 484 ms |
| Quatre workers alimentés dès qu'ils sont libres | 43,49 s | 268 ms | 496 ms |

Simulation complète par `Application::preview_paged`, même façade que la GUI :
**59,55 s** pendant les compilations et tests, puis **45,10 s** avec les sources
finales et sans compilation simultanée. Ce dernier temps inclut la préparation
du plan et sa présentation paginée. L'objectif est atteint pour cet arbre et
ce mode. Le temps FreeFileSync d'environ 60 s est celui fourni par l'utilisateur,
pas une exécution de FreeFileSync instrumentée dans cette session.

Le premier passage voit 221 722 entrées ; les suivants voient tous 221 730
entrées : 172 352 fichiers et 49 378 dossiers, inventaires complets, zéro
anomalie de listing. Le partage a donc changé entre le premier passage et les
suivants ; l'égalité exacte avec le premier inventaire n'est pas revendiquée.
Les quatre inventaires sauvegardés ensuite, avec leurs opérations de miroir,
sont identiques octet pour octet : SHA-256
`f12c85a7050c608e7c8612778f3a5b1af7ffbc7d0bf346ac40b92d501951db9c`.

Le plan contient 219 782 créations/copies, zéro suppression et zéro remplacement
vers cette destination vide. Six collisions de casse restent incertaines et
1 942 descendants sont écartés par `parent_unavailable` : ces refus existaient
déjà et sont identiques entre parcours séquentiel et parallèle. Ils ne sont
ni des fichiers oubliés par le scan ni des copies réussies. Les métadonnées
annoncent 142 167 475 400 octets au total, dont 142 023 100 805 octets planifiés.

## Causes vérifiées

1. **Le lecteur rapide précédent n'était pas utilisé sur ce serveur.**
   `directory_timing` constate 0/100 lectures directes avec la classe étendue,
   puis 100/100 avec `FileFullDirectoryInfo`. Les identifiants de fichiers
   n'étaient pas utilisés pour la comparaison. La classe compatible évite le
   repli qui convertissait la poignée en chemin normalisé avant FindFirst.
   Le buffer reste de 64 Kio ; aucun stat n'est ajouté par fichier ordinaire.
2. **Le contrôle de racine répétait des lectures Windows.** L'ouverture cap-std
   vérifiait déjà le type par métadonnées ; `dir_metadata` redemandait les
   métadonnées puis leur identité. Sur 100 essais : ancien contrôle environ
   1,14 ms (733 µs + 409 µs), nouveau contrôle environ 317 µs. Le nouveau helper
   ouvre la racine courante et lit attribut répertoire, volume et index dans
   une seule requête. La comparaison avec l'identité épinglée reste obligatoire.
3. **49 378 dossiers séquentiels accumulaient la latence SMB.** Le scanner
   utilise quatre threads persistants et distribue un nouveau dossier dès
   qu'un worker termine. L'essai par lots montrait encore 75,66 s : attendre
   le dossier le plus lent d'un lot laissait les autres workers inactifs.
4. **La comparaison et la stratégie ne prenaient pas les minutes observées.**
   Elles restent sous une seconde au total. Le mode contenu reste un autre
   coût : il peut lire les contenus des candidats, contrairement à ces essais.

La référence FreeFileSync a été consultée dans
`F:/Project/referance/syncengine/FreeFileSync` :
`FreeFileSync/Source/afs/native.cpp`, `Source/base/parallel_scan.cpp`,
`zen/file_traverser.cpp` et `zen/file_access.cpp`. Elle utilise les attributs
du listing et des workers par appareil ; cette archive contient le natif POSIX,
pas son implémentation Windows. Aucun code C++ n'est transplanté. Le choix de
quatre lectures est validé par les mesures SyncEngine, pas attribué à FreeFileSync.

## Contrats et sécurité

- `StorageConnector::listing_concurrency` est une extension avec défaut à 1.
  Le scanner borne les workers par cette annonce, `ScanOptions` (défaut 4) et
  un maximum de 16. Le connecteur local Windows de preview annonce 4 ; SMB
  délègue cette capacité. Les autres connecteurs restent séquentiels.
- Les curseurs ont un jeton à usage unique, lié au chemin et à l'instance.
  Seize curseurs au maximum ; un chemin réservé ne peut pas être énuméré en
  parallèle via des duplicatas de la même poignée. Le mutex libère les E/S.
  Le cache de dossiers reste borné à 64 poignées par connecteur.
- Le budget d'entrées est partagé atomiquement, y compris les anomalies et
  exclusions. Son épuisement produit un scan incomplet ; les entrées retenues
  à la limite peuvent varier avec l'ordre des workers, jamais devenir une
  preuve d'absence. Les anomalies de listing sont triées par chemin.
- Seuls les enfants inclus par le filtre sont planifiés pour le parcours.
  Une annulation draine les workers et conserve les zones non visitées comme
  inconnues. Un panic de worker devient une erreur fatale structurée, sans
  plan exploitable ; les canaux bornés se ferment avant la jonction des threads.
- FULL ne contient pas le tag reparse : les seules entrées reparse reçoivent
  une lecture confinée de métadonnées **sans suivre** la cible. Une erreur
  de cette lecture invalide le listing. Les jonctions restent des liens,
  les placeholders sans substitution de nom restent ordinaires.
- La racine actuellement nommée est toujours vérifiée avant chaque dossier
  et à la dernière page d'un dossier multipage. Les connecteurs d'exécution
  ne réutilisent pas les poignées du preview et gardent leurs contrôles complets.
- La copie et l'exécuteur ont été relus : transfert séquentiel par blocs de
  256 Kio, dépôt temporaire, contrôle d'intégrité, revérification de la source
  et de la destination avant publication, suppressions en dernier. Ces règles
  ne sont pas allégées pour atteindre le temps de simulation. Aucun débit de
  transfert réel NAS ni gain de copie n'est annoncé par cette mesure.

Justification de la couche Windows et tests FFI :
[correctif vendored](../vendor/cap-primitives/SYNCENGINE-PATCH.md).
Sources primaires :
[classes GetFileInformationByHandleEx](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getfileinformationbyhandleex),
[FILE_FULL_DIR_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_full_dir_info),
[normalisation SMB](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew),
[classification name-surrogate de Rust](https://github.com/rust-lang/rust/blob/master/library/std/src/sys/fs/windows.rs).

## Reproduire sans mutation du partage

Créer uniquement les dossiers locaux de destination vide et d'état isolé :

```powershell
New-Item -ItemType Directory -Force target/nas-preview-empty, target/nas-preview-state
cargo build --release -p sync-orchestrator --examples --all-features --locked
target/release/examples/preview_timing.exe '\\NAS\testsync' "$PWD/target/nas-preview-empty" none 1 reuse
target/release/examples/facade_timing.exe '\\NAS\testsync' "$PWD/target/nas-preview-empty" "$PWD/target/nas-preview-state" size preview-only
cargo run --release -p connector-local --example directory_timing --locked -- '\\NAS\testsync' 100
```

`preview-only` est impératif pour utiliser `facade_timing` en simulation : sans
cet argument, cet exemple de développement effectue réellement la copie.
Une faute dans ce drapeau ou sa position est refusée avant ouverture de l'état
applicatif ; ces deux cas sont vérifiés sur un dossier temporaire resté vide.
`preview_timing` n'exécute jamais son plan. Il accepte aussi un fichier JSON
de snapshot **nouveau**, puis un nombre de workers de listing ; exemple final
`none 1 reuse target/inventory.json 1` pour mesurer le séquentiel. Les snapshots
contiennent des noms de fichiers et doivent rester dans le dossier ignoré
`target/`, puis être supprimés après comparaison. Les commandes ont été lancées
sur les exécutables compilés ; l'accès NAS a nécessité une sortie du sandbox.

## Vérifications exécutées

```powershell
cargo fmt --all -- --check
rustfmt --edition 2021 --check vendor/cap-primitives/src/windows/fs/handle_read_dir.rs
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
$env:RUSTFLAGS='-D warnings'
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo test -p cap-primitives --lib handle_read_dir --locked
cargo test -p cap-primitives --lib large_read_dir --locked
cargo test --release -p sync-scanner --test parallel_listing --locked -- --ignored --nocapture
cargo test --release -p cap-primitives --lib handle_read_dir::tests::million_records_in_bounded_batches --locked -- --ignored --nocapture
target/network-audit-env/Scripts/python.exe scripts/test_phase08_sftp.py
target/network-audit-env/Scripts/python.exe scripts/test_phase08_ftp.py
target/network-audit-env/Scripts/python.exe scripts/test_phase08_webdav.py
target/network-audit-env/Scripts/python.exe scripts/test_phase09_onedrive.py
cargo build --release -p syncengine-desktop -p syncengine-cli -p syncengine-agent --all-features --locked
```

Fmt, Clippy et check passent avec warnings interdits. Windows : 528 tests
workspace réussis, dix tests ignorés par défaut (neuf réseau et un de volume) ;
les tests réseau ignorés par défaut sont exécutés avec
succès par les quatre scripts localhost. Le test de volume ignoré par défaut
est aussi exécuté explicitement : deux inventaires identiques de 1 001 000
entrées synthétiques, 900 ms séquentiel / 363 ms parallèle au premier essai,
845 / 273 ms au second. Le second processus atteint environ 642 Mio de working
set de pointe pour **les deux inventaires conservés ensemble**. Ce test mesure
la partie scanner en RAM, pas un million de vrais fichiers sur SMB. Le parseur
traite séparément un million d'enregistrements synthétiques en 36,85 ms.
Les sept tests vendored courants passent également.

Le premier test ACL local échouait parce que le sandbox refusait `icacls` ;
le même test et le workspace complet passent hors sandbox, sur leurs fixtures
temporaires isolées. Aucun échec n'est masqué par un skip ajouté au code.
Linux n'a pas été compilé ou exécuté ici ; il reste couvert par la CI existante.
Les appels SMB synchrones en cours ne peuvent toujours pas être interrompus
avant le retour de Windows. Le scanner conserve un inventaire en mémoire,
borné par son budget ; ce changement n'introduit pas un inventaire sur disque.

## Fichiers concernés

- `connector-api/src/connector.rs` : annonce conservatrice de concurrence.
- `connector-local/src/{connector,paging}.rs` : racine légère, lecteurs FULL,
  curseurs indépendants bornés ; `tests/local.rs`, exemple `directory_timing`.
- `connector-smb/src/mounted.rs` : délégation de la concurrence effective.
- `sync-scanner/src/{lib,scan}.rs`, `src/scan/parallel.rs` : options, workers,
  budget partagé, échec structuré ; `tests/parallel_listing.rs`.
- `connector-memory/src/lib.rs` : annonce configurable pour injection de tests.
- `sync-mirror/tests/mirror.rs` : plan inchangé, déconnexion/accès refusé ⇒
  zéro suppression avec scans parallèles.
- `sync-orchestrator` : exemples de mesure, commentaire du parcours ;
  `sync-session/src/lib.rs` : documentation de la concurrence.
- `vendor/cap-primitives/src/{lib.rs,windows/fs/handle_read_dir.rs}` et sa
  notice : classe FULL, parsing, identité et revue de sûreté.
- Architecture, développement, présent audit, changelog et note de version.

Aucune nouvelle dépendance, migration, modification de version ou changement
des préconditions de mutation. Les fixtures NAS ne sont ni distribuées ni
versionnées. Les snapshots, états et anciens artefacts de test sous `target/`
sont nettoyés après validation ; les exécutables finaux sont reconstruits
avec les connecteurs réseau et OneDrive compilés.

Nettoyage réalisé : anciennes constructions `target/debug` et
`target/x86_64-pc-windows-msvc`, environnement Python d'audit, fixtures
`phase08-*`/`phase09-*`, `version-test`, `tmp`, états et dossiers NAS locaux,
quatre snapshots JSON, anciens journaux d'audit et exécutables de benchmarks.
Les suppressions sont limitées à des chemins vérifiés sous `target/` ; les
anciennes fixtures ont nécessité les mêmes droits hors sandbox que leurs tests.
Les rapports courants `target/nas-*.log`, les notices tierces et le cache release
utile à la reconstruction sont conservés. Les trois exécutables à tester sont
dans `target/release/`, avec `SHA256SUMS.txt`. La version reste 0.1.6 ; aucune
publication ou installation n'est faite par cette session.
Les commandes `target/release/syncengine.exe help` et `version` réussissent
après reconstruction ; la CLI annonce `windows-x86_64, release`.
