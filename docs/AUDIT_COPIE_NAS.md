# Copie NAS : mesures bornées et diagnostic

Ce rapport décrit la première itération. Le comportement actuel des remplacements
versionnés, des sources miroir et de l’historique est décrit dans le
[complément de stabilité](STABILITE_COPIE_HISTORIQUE.md).

Audit Windows du 25–26 octobre 2026, source `\\NAS\testsync`, destination
`D:\sync\1`. Le redémarrage du PC a été pris en compte : aucun ancien worker
de copie actif, source et destination accessibles, résultats précédents présents.
Les dossiers de test sont dédiés ; aucun profil utilisateur ni historique réel
n'a été modifié. La base utilisateur a seulement été lue pour comprendre les refus.

## Résultats réels

Chaque essai construit et sauvegarde son plan avant exécution : uniquement des
créations dans une destination vide, jamais de remplacement ni de suppression.
Plafond de 512 Mio par essai, sélection de fichiers entiers sous 90 % du plafond,
arrêt coopératif si le compteur dépasse le plafond. Pas de copie des 120 Go.

| Essai | Fichiers | Octets vérifiés | Copie et vérification | Moyenne |
| --- | ---: | ---: | ---: | ---: |
| Séquentiel initial | 2 004 | 483 183 635 | 267,50 s | 1,81 Mo/s |
| Première optimisation, 4 workers | 2 004 | 483 183 635 | 93,70 s | 5,16 Mo/s |
| Ouverture minimale, 8 workers | 2 004 | 483 183 635 | 68,56 s | 7,05 Mo/s |
| Gros fichiers, 4 workers | 4 | 483 183 636 | 6,61 s | 73,07 Mo/s |

Total écrit : **1 932 734 541 octets**, inférieur à 2 Go. Tous les essais ont
réussi, sans refus, échec, annulation ni temporaire restant. Le lot mixte comprend
2 000 fichiers de moins de 64 Kio, quatre plus gros et 1 104 dossiers ; le dernier
lot comprend quatre fichiers et dix-huit dossiers. Vérification du contenu par
relecture, mêmes préconditions, temporaires et publication que le moteur normal.
Les mesures incluent la vérification et les barrières de journal, pas le preview.

Le lot mixte est environ **3,9 fois plus rapide** que le premier essai, mais les
changements de concurrence et de métadonnées sont combinés : ce n'est pas une
mesure isolée du seul gain de huit workers. La valeur par défaut reste quatre.
Les caches NAS/Windows et le redémarrage influencent ces mesures ponctuelles.

Contrôle final de simulation après séparation des accès preview/exécution :
**52,68 s de scan**, 310 ms de comparaison et 531 ms de plan, soit environ
53,5 s au total, 221 730 entrées et zéro erreur d'inventaire. Des essais
intermédiaires avaient pris 81 à 115 s ; le preview conserve finalement son
chemin historique de cache et ses droits de métadonnées. Aucune copie n'est
effectuée par ce contrôle.

Le débit échantillonné atteint environ 100 Mo/s dans le lot mixte et 116 Mo/s
pendant une fenêtre du lot de gros fichiers. Cela ne prouve pas un débit réseau
constant : les caches, la durée d'échantillonnage et les compteurs d'octets
transférés influencent les pointes. **100 Mo/s continus ne sont pas atteints.**
Les petits fichiers restent dominés par ouvertures, préconditions, création de
dossiers, durabilité, fermeture et validation ; aucune donnée ne circule pendant
certaines de ces étapes. Les chutes ne peuvent pas être exclues honnêtement.
La taille mémoire reste bornée par les buffers et le nombre de workers, en plus
du plan et de ses dépendances déjà en mémoire ; aucun gros fichier n'est chargé
entièrement. Il n'y a pas eu de test réel sur un million de transferts.

## Causes et corrections

- L'exécuteur attendait la fin complète de chaque opération avant la suivante.
  Un pool persistant borné occupe désormais plusieurs attentes indépendantes.
  Le coordinateur respecte le graphe du plan et publie les résultats dans
  l'ordre original. Les dossiers indépendants peuvent avancer en parallèle,
  jamais avant leur parent. Les suppressions commencent après jonction du pool.
- Les résolutions de chemins redemandaient des métadonnées avant une ouverture
  qui refaisait déjà la validation. L'ouverture confinée valide directement
  type et absence de lien ; sous Windows, une requête fixe FileAttributeTagInfo
  évite les métadonnées génériques complètes de chaque ancêtre. Mesure NAS sur
  300 ouvertures : 657 µs contre 423 µs. Voir la
  [revue du correctif Windows](../vendor/cap-primitives/SYNCENGINE-PATCH.md).
- La racine courante est vérifiée en une requête d'identité, avec les droits
  GENERIC_READ historiques pendant l'exécution. Le preview garde ses droits de
  métadonnées et son chemin de cache historique ; les écritures ne réutilisent
  jamais ce cache. Les chemins sont encore reparcourus à chaque opération.
- Progression agrégée sous verrou, événements bornés, pause et annulation
  héritées via un jeton enfant. Une erreur fatale interrompt les autres workers
  sans annuler le jeton de l'appelant. Un panic de worker devient un échec fatal.

La référence FreeFileSync locale a été consultée dans
`Source/base/synchronization.cpp` (Workload et commentaires de synchronisation
des métadonnées/libération du verrou pendant les E/S, vers les lignes 1105–1318),
ainsi que `zen/file_access.cpp`. Elle confirme l'intérêt d'une file de travaux
indépendants et du respect des dossiers parents. Cette version des sources ne
permet pas de déduire le nombre exact de workers du binaire Windows de l'utilisateur,
ni de promettre un débit identique. Aucun code FreeFileSync n'a été recopié.

## Refus et interface

Les 37 refus de la dernière copie utilisateur se répartissent en 33 sources
ayant changé après la simulation et quatre sources disparues. Ils correspondent
à des garde-fous, pas à 37 erreurs d'accès supposées. La copie ne force pas ces
fichiers. Refaire une simulation permet de travailler sur leur état actuel.

Dans **Historique**, ouvrir une exécution, puis sélectionner **Problèmes**,
**Échecs**, **Refusés**, **Annulées / non commencées** ou **Ignorées / conflits**. La recherche s'applique
au chemin et au diagnostic ; appuyer sur Entrée. Le filtrage précède la pagination
de 100 lignes. Les messages réels d'erreur restent visibles, les refus sont
expliqués en français. Un résultat non enregistré reste inconnu, jamais réussi.
Après une copie lancée dans la fenêtre, le bilan affiche les premiers problèmes
et un bouton ouvre directement les problèmes de cette exécution.
Les jobs d'agent restent consultables dans leur historique.

Les anciennes bases restent compatibles, sans migration : les résultats durables
JSON sont interrogés côté application. SQLite JSONB sert seulement au calcul en
mémoire, le format persistant reste inchangé. Sur une très grosse exécution,
le premier filtrage peut encore prendre plusieurs secondes ; il tourne sur un
thread de travail. Les réponses obsolètes sont ignorées grâce à une génération
de requête ; aucune recherche n'est lancée à chaque frappe.

## Reproduire sans copier tout le partage

```powershell
cargo build --release -p sync-orchestrator --example copy_sample_timing --locked
# Créer explicitement un NOUVEAU dossier vide sur le disque de test.
target/release/examples/copy_sample_timing.exe '\\NAS\testsync' 'D:\sync\1\nouvel-essai' "$PWD/target/nouvel-audit" 4 512 --confirm
# Facultatif : mêmes limites, mais seulement des fichiers plus gros.
target/release/examples/copy_sample_timing.exe '\\NAS\testsync' 'D:\sync\1\nouvel-essai-gros' "$PWD/target/nouvel-audit-gros" 4 512 --confirm --large-files
```

L'exemple refuse une destination non vide, un dossier de sortie existant, des
racines imbriquées et un budget hors 32–1024 Mio. Les courbes de débit sont
dans `target/copy-audit/{serial,parallel4,parallel8,large4}` et le bilan agrégé
dans `target/copy-audit/summary.json` (ignorés par Git). Les plans, rapports et
journaux temporaires contenant des noms réels ont été supprimés après validation.
Les seuls dossiers créés sur D sont `mesure-sequentielle`, `mesure-parallele-4`,
`mesure-parallele-8` et `mesure-gros-fichiers`, sous `D:\sync\1`.

Pour régler la concurrence en CLI : ajouter `--workers 1` à `execute` pour forcer
le séquentiel, ou `--workers 8` pour demander huit workers. Les connecteurs doivent
annoncer cette capacité : local et SMB annoncent huit, les autres un. Plafond
global seize. Les remplacements avec versions restent sur le coordinateur ; les
nouvelles copies profitent du pool même si la conservation des versions est active.
Le bidirectionnel reste séquentiel. Aucune suppression n'est autorisée par cette
option : elle exige toujours sa politique et sa confirmation habituelles.

## Validation et périmètre des fichiers

Contrôles Windows réellement exécutés :

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
$env:RUSTFLAGS='-D warnings'
cargo check --workspace --all-targets --all-features --locked
cargo build --release -p syncengine-desktop -p syncengine-cli -p syncengine-agent --all-features --locked
```

Les tests de sûreté couvrent concurrence effective et plafond, dépendances,
équivalence des contenus, ordre durable, annulation/reprise par nouvelle
simulation, versions conservées, refus de source changée, échec de dossier,
disque plein, déconnexion et échec de journal avec zéro suppression. Les tests
locaux couvrent jonctions, ambiguïtés de noms, identité, ACL et confinement.
Les nouveaux tests d'historique couvrent filtres avant pagination, rapports
directs/bidirectionnels, isolation de profil, recherche et interruption.
Les serveurs de protocoles externes requis par les tests ignorés n'ont pas été
relancés pour cette tâche. Linux n'a pas été compilé ou exécuté ici.
Résultat du workspace : **542 tests réussis, zéro échec, dix ignorés**
(neuf nécessitent des serveurs isolés et un concerne le million d'entrées).
`fmt`, `clippy` et `check` avec warnings interdits réussissent. Les premières
tentatives de tests ACL dans le bac à sable échouaient sur `icacls` ; les mêmes
tests ont été relancés hors bac à sable sur leurs dossiers temporaires et passent.

Livraison locale : trois exécutables **0.1.6, windows-x86_64, release** dans
`target/release`, avec empreintes SHA-256 dans `SHA256SUMS-copy.txt`. `syncengine
version` et l'aide CLI avec `--workers` ont été exécutés. L'interface n'a pas été
validée visuellement dans cette session ; sa compilation et les tests de décodage
des diagnostics passent. Aucune installation, publication ni modification des
profils réels n'est effectuée.

Fichiers concernés : `sync-core/src/cancel.rs` (jeton enfant), contrat
`connector-api`, connecteurs local et SMB et leur support mémoire, exécuteur
(`options`, `report`, `error`, `run`, `run/parallel` et tests), orchestrateur
(`history`, `pages`, `runs`, export et exemple plafonné), CLI (`cli`, `execute`),
présentation desktop (`actions`, `app`, `backend`, `model`, `pages`, `profile`,
`update`), extension Windows vendue (`lib`, `mod`, `handle_read_dir`,
`open_directory`), exemple `directory_timing`, architecture, notes et changelog.
Pas de changement de dépendance, de version ou de règle de synchronisation.
