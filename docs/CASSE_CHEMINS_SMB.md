# Dossiers B/b : diagnostic Windows/SMB du 26 octobre 2026

**Mise à jour : la fusion est livrée ; la simulation réelle termine sans saut
ni conflit en 43,69 s.** Voir [le comportement actuel et les tests](FUSION_CASSE_ET_JOURNAUX_ACTIFS.md).
Le présent document conserve le diagnostic initial et sa protection intermédiaire.

## Périmètre et constat

Diagnostic en lecture seule de `\\NAS\docker\jellyfin\config\metadata\People`
et `F:\doker_backup\jellyfin\config\metadata\People`. Aucun fichier de données
réelles n’a été modifié. L’audit ultérieur du binaire FreeFileSync a créé puis
retiré son verrou temporaire. Aucun transfert massif n’a été lancé.

Le caractère `$` est valide et n’explique pas le refus de `bbno$`. Le NAS
énumère deux noms distincts pour chacune des paires `B/b`, `C/c`, `S/s`.
Les ouvertures relatives confinées donnent également des identités distinctes :

| Nom | Identifiant de fichier NAS |
| --- | --- |
| B | 16352265 |
| b | 16739710 |
| C | 16352618 |
| c | 16582200 |
| S | 16359287 |
| s | 24444697 |

Sur la destination NTFS existante, chaque paire ouvre un seul et même objet.
Les identifiants sont propres à cette observation, pas des constantes du code.

Mais les listings Windows ne suivent pas cette distinction de manière fiable :

| Ordre de lecture | B et b : enfants chacun | C et c | S et s |
| --- | --- | --- | --- |
| Majuscule en premier | 213, sans préfixe bbno$ | 344 | 452 |
| Minuscule en premier | 4, tous préfixés bbno$ | 0 | 1 |

Le lecteur direct `GetFileInformationByHandleEx/FileFullDirectoryInfo` a été
utilisé sans repli. Le lecteur avec identifiants étendus déclenche ici un repli
FindFirstFileEx et reproduit le problème. Un essai séparé de FindFirstFileEx,
avec et sans `FIND_FIRST_EX_CASE_SENSITIVE`, reproduit aussi les quatre enfants
pour les deux noms. Le changement de lecteur seul ne résout donc pas le cas.

L’effet dépend de l’ordre et est compatible avec une confusion de cache de
noms. Sans capture protocolaire, son emplacement exact dans la pile client/
serveur n’est pas établi. Une fin normale d’énumération ne suffit pas à rendre
ces branches sûres. La variante `OBJ_CASE_INSENSITIVE` de la dépendance vendue
a aussi été examinée ; son changement global n’a pas été tenté : il modifierait
les ouvertures de tous les chemins, sans démontrer que ce problème est résolu.

## Différence précise avec FreeFileSync

Référence locale : `F:\Project\referance\syncengine\FreeFileSync`.

- `FreeFileSync/Source/base/comparison.cpp`, `matchFolders` (vers 780) : regroupe
  les noms canoniques, rapproche les correspondances un contre un, puis réessaie
  avec la casse exacte quand le groupe est ambigu. SyncEngine applique pour
  l’instant un blocage conservateur à tous les chemins de même forme minuscule.
- `FreeFileSync/Source/base/synchronization.cpp`,
  `FolderPairSyncer::synchronizeFolderInt` (vers 2150) : après échec de création,
  si la destination est déjà un dossier, poursuit la synchronisation. Cela
  permet de réutiliser le dossier Windows existant et d’y copier des enfants.
- `FreeFileSync/Source/afs/native.cpp`, `copyNewFolderForSameAfsType` : la
  création native échoue bien si la cible existe ; c’est la couche précédente
  qui traite cette situation, pas une suppression des caractères du nom.

Cette logique explique comment une copie de `bbno$` peut aboutir. Elle ne prouve
pas, à partir de sa seule présence à destination, que les deux branches NAS
ont été intégralement énumérées pendant cette exécution. Le binaire Windows de
FreeFileSync n’a pas été instrumenté ni comparé par un inventaire exhaustif.

### Confirmation dans la sauvegarde existante

Deux lectures espacées, majuscules en premier puis minuscules en premier, ont
permis de comparer les ensembles de noms des enfants directs, sans se contenter
du nombre d’entrées. Dans `F:\doker_backup\…\People` :

| Dossier cible | Union des enfants source | Enfants cible | Manquants | Supplémentaires |
| --- | --- | --- | --- | --- |
| B | B : 213 + b : 4 = 217 | 217 | 0 | 0 |
| C | C : 344 + c : 0 = 344 | 344 | 0 | 0 |
| S | S : 452 + s : 1 = 453 | 453 | 0 | 0 |

Les ensembles des deux variantes sont disjoints pour ces enfants directs.
Les quatre noms de `b` sont les dossiers `bbno$`, `bbno$, Ironmouse`,
`bbno$, Käärijä`, `bbno$, VNLLA`. La sauvegarde existante confirme donc une
fusion des deux contenus sous les lettres majuscules. Ce contrôle porte sur les
noms des enfants directs, pas sur tous les octets ni tous leurs descendants.
L’option `--show-names` de l’exemple permet de conserver ces listes dans un
fichier de diagnostic local non versionné.

## Protection intermédiaire, remplacée par la fusion explicite

La protection intermédiaire propageait une collision aux descendants, empêchant
de supprimer `B/bbno$/folder.jpg` sur un listing ambigu mais bloquant aussi les
copies de toute la branche. Ce comportement reste celui du comparateur bas
niveau sans option de projection ; la façade unidirectionnelle active la fusion.
Une branche voisine `Beta` reste indépendante. Les tests de régression couvrent
un inventaire annoncé complet, une copie différente et une suppression autorisée.

Ce changement intermédiaire protégeait les données sans résoudre la copie.
Il a étendu les avertissements aux descendants et a été remplacé, dans la
prévisualisation unidirectionnelle vers Windows, par une projection explicite
conservant les origines physiques. Dire que chacun de ces fichiers était
intrinsèquement impossible à copier était trop général.

La fusion a depuis été demandée et implémentée, avec protection des conflits
réels de fichiers. Ni le NAS ni le registre Windows ne sont reconfigurés.

## Reproduction en lecture seule

```powershell
cargo build --release -p connector-local --example case_lookup_audit --locked
$people = '\\NAS\docker\jellyfin\config\metadata\People'
target/release/examples/case_lookup_audit.exe --upper-first $people
# Laisser expirer les caches entre les deux essais ; aucune purge globale.
target/release/examples/case_lookup_audit.exe $people
```

L’exemple ne crée aucun fichier et affiche identités, type de lecteur, nombre
d’enfants et nombre de noms préfixés `bbno$`. Chaque essai ouvre les six dossiers
du cas Jellyfin ; il ne prétend pas être un scanner générique.

## Vérifications Windows du correctif de protection

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features` : 551 réussis, 0 échoué, 10 ignorés
  (fixtures de protocoles externes et test volumétrique opt-in).
- `cargo check --workspace --all-targets --all-features` avec
  `RUSTFLAGS=-D warnings`.

Les tests complets ont été lancés dans le contexte utilisateur nécessaire aux
tests Windows ACL et Task Scheduler. Aucun test Linux n’a été exécuté ici.

Compilation livrée : `cargo build --release -p syncengine-desktop
-p syncengine-cli -p syncengine-agent --all-features --locked`, version 0.1.6
Windows. Les trois binaires standards sont dans `target/release`, avec leurs
empreintes `SHA256SUMS.txt`. Le précédent `syncengine-desktop-absence.exe` a été
conservé parce qu’il était encore ouvert ; aucun processus n’a été arrêté.

Fichiers de cette correction : comparaison (`sync-compare/src/compare.rs`),
tests de comparaison et de planification (`sync-decisions/tests/decisions.rs`),
diagnostic local (`connector-local/examples/case_lookup_audit.rs`), libellé
desktop (`apps/desktop/src/model.rs`), architecture, présent diagnostic,
rectification de `ABSENCE_SMB_ET_CONFLITS.md`, changelog et note de release.
