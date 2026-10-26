# Stabilité des copies et filtres d’historique — 26 octobre 2026

Mise à jour : le miroir accepte aussi la croissance d’un fichier actif, par
copie bornée et vérification BLAKE3 du préfixe. Voir
[Fusion de casse et journaux actifs](FUSION_CASSE_ET_JOURNAUX_ACTIFS.md).

Ce complément à l’[audit initial](AUDIT_COPIE_NAS.md) répond au retour réel :
compteurs à 5/1 954, baisses fortes du débit, refus de sources modifiées et
`database is locked` en consultant les filtres. Les modifications précédentes
étaient encore présentes dans le workspace : elles ont été conservées.

## Diagnostic vérifié sur le dernier rapport utilisateur

La base et le journal utilisateur ont été lus sans mutation. La base fait
environ 1 058 Mio et utilise encore le journal SQLite DELETE avant migration.
Le dernier plan comporte **1 885 remplacements, 69 créations, 219 752 conservations
et 125 éléments ignorés**. Le résultat contient 1 920 réussites, 34 refus de
source modifiée et **zéro échec de copie**. Les 34 refus ne sont donc pas des
erreurs d’accès supposées.

Le journal révèle un intervalle de **123,818 s** entre `staging_created` et
`version_preserved` pour un fichier de **5 405 Mio**, puis d’autres de 12,177 s
pour 430 Mio et 4,623 s pour 198 Mio. La durée totale consignée est 296,18 s.
Ces intervalles incluent transfert, vérification et conservation de version :
ils ne permettent pas d’attribuer précisément chaque seconde à une phase.
Ils ne constituent pas un délai de retry sur les refus.

Le chemin des remplacements versionnés était encore exécuté sur le coordinateur.
Il empêchait celui-ci de distribuer les travaux suivants et de collecter les
résultats des autres workers pendant une grosse opération. Comme la façade
conserve les versions, cela concernait presque tout ce test réel, contrairement
aux premiers essais limités à des créations dans un dossier vide.

Le compteur « volume validé » n’avance qu’après publication d’un fichier entier.
Il était donc possible de transférer plusieurs Go sans voir ce compteur bouger.
Le débit calculé sur les blocs transférés restait distinct, mais l’interface ne
montrait ni la relecture ni l’archivage local entre deux transferts.

L’historique ouvrait chaque connexion en faisant `quick_check` de toute la base
puis `BEGIN IMMEDIATE`, même pour lire une page. Plusieurs demandes concurrentes
de filtres pouvaient donc entrer en concurrence comme des écrivains. Les requêtes
reparcouraient aussi les gros JSON de plan/rapport à chaque clic.

## Comparaison approfondie avec les sources FreeFileSync fournies

Référence locale : `F:\Project\referance\syncengine\FreeFileSync`.

- `Source/base/synchronization.cpp`, vers 1105–1419 : files FIFO de travaux,
  redistribution entre workers, dossiers avant contenu et déverrouillage pendant
  les E/S. Les erreurs sont gérées au niveau de chaque travail.
- `Source/base/status_handler_impl.h`, vers 466 : une erreur quitte le travail
  si le callback demande de l’ignorer ; une nouvelle tentative est explicite.
  `base/structures.h` définit zéro tentative automatique par défaut, délai 5 s
  lorsqu’elles sont configurées. Notre délai local de partage/verrou est borné
  à environ 1,3 s ; il n’explique pas à lui seul la pause de 123 s.
- `Source/afs/abstract.cpp`, vers 96–160 : actualisation des attributs de la
  source via son flux lorsque possible, copie en flux et validation de la taille
  réellement lue. La présence du fichier n’est pas la seule condition.
- `Source/base/synchronization.cpp`, vers 2287–2317 : notification de progression
  au fil des blocs, vérification complète conditionnelle après copie.
  `Source/config.h:156` initialise `verifyFileCopy` à **false**. Cela ne prouve
  pas le réglage utilisé dans le profil de l’utilisateur.
- `Source/base/versioning.cpp`, vers 215–238 : essai de déplacement vers le
  magasin de versions, puis copie en repli. Notre magasin peut être sur un autre
  volume que D et conserve une copie avant écrasement : cette E/S supplémentaire
  demeure. Aucun déplacement destructif ni désactivation de vérification n’a été
  introduit pour rendre artificiellement les mesures comparables.

Le clone fourni instancie `Workload(1, ...)` dans ce chemin ; il ne permet pas de
déduire la concurrence du binaire Windows installé. Aucun code FreeFileSync n’a
été recopié, aucune supériorité absolue n’est déduite des seuls sources.

## Corrections

### Exécution et versions

Les copies et remplacements indépendants utilisent désormais le même pool,
y compris avec versions. Le plafond est le minimum de l’option demandée, des
capacités source/destination/magasin et de seize. La façade demande quatre ;
local et SMB annoncent huit. Le bidirectionnel reste séquentiel.

Un worker garde l’ancienne version avant publication. Le journal reste durable
et sérialisé ; parents avant enfants, suppressions après jonction du pool,
annulation et arrêt sur erreur fatale sont inchangés. Une création concurrente
de parent n’est acceptée qu’après une observation fraîche confirmant un dossier.

Le magasin prépare les dossiers nécessaires et le connecteur revalide toujours
la racine et les ancêtres au moment de l’écriture. Aucun cache de capacités
n’autorise une mutation ; un parent remplacé par un fichier/lien bloque la
conservation de version et donc l’écrasement. Une erreur de copie n’est pas
rejouée aveuglément.

Sous Windows, la lecture locale supprime un `symlink_metadata` redondant avant
l’ouverture sans suivi. Le type du fichier effectivement ouvert est toujours
contrôlé par sa poignée. Le contrôle préalable reste sur les autres plateformes,
notamment pour éviter d’ouvrir un FIFO avant d’en connaître le type.

### Source changée entre simulation et copie

La façade GUI/agent active `ExecutionOptions::refresh_mirror_source` en miroir.
Seuls les fichiers déjà sélectionnés pour copie/remplacement sont concernés :
une nouvelle observation est prise juste avant lecture, sa taille/date sert au
transfert, puis elle est vérifiée à nouveau après lecture. Le texte est affiché
dans le plan de simulation. Les fichiers non sélectionnés ne sont pas ajoutés,
les suppressions et la destination conservent leurs contrôles. Le moteur et la
CLI restent stricts par défaut ; le bidirectionnel reste strict même si l’option
est passée par erreur.

Une source disparue, un lien, un type changé ou des métadonnées non vérifiables
reste refusé. En miroir actualisé, une croissance ou modification de date pendant
lecture est acceptée seulement si le préfixe borné copié est revérifié par BLAKE3.
Une réécriture ou troncature de ce préfixe retire le temporaire et conserve
l’ancienne destination. Une base de données active peut rester
impossible à copier de manière cohérente ainsi : les contrôles de taille/date
et l’égalité du flux copié ne garantissent pas une transaction applicative
cohérente. Il faut alors une sauvegarde propre à l’application ou un snapshot ;
la présence du nom de fichier ne suffit pas à justifier une copie forcée.

### Activité visible sans faux débit

`IoActivity` sépare les octets relus pour vérification et archivés en versions
des octets transférés à la destination. Les notifications sont agrégées par Mio
et fin d’opération ; aucun événement par octet. `JobSnapshot` expose les deux
compteurs avec défaut zéro pour lire les anciens snapshots d’agent.
La fenêtre montre transféré/en vérification/versions, et explique que le volume
validé attend la publication complète. Le graphe ne mélange pas ces E/S locales
avec le débit de copie : une absence réelle de transfert n’est pas masquée.
Les refus de changement affichent aussi le champ concerné : tailles prévue et
observée, dates, changement de type ou empreinte différente.

### Historique desktop v2

Migration transactionnelle `002_problem_index.sql`, version applicative Rust
inchangée. Les plans et rapports JSON originaux restent autoritaires ; seules
les lignes présentant un problème sont dupliquées dans un index dérivé. Les
nouveaux rapports et leur index sont écrits dans la même transaction.

SQLite WAL permet les lectures pendant une écriture. `quick_check` reste au
démarrage applicatif, jamais sur chaque filtre ; les lectures normales ne font
plus `BEGIN IMMEDIATE`. La rétention cascade les index avec leurs exécutions.

Un ancien rapport prépare son index une seule fois, en parcourant les résultats
puis le plan une seule fois avec des recherches par ordinal entier dans TEMP.
Cette préparation ne réserve pas le verrou d’écriture principal. La publication
du petit index est ensuite courte. Si un autre écrivain l’occupe, après 50 ms la
lecture utilise la projection TEMP valide au lieu de retourner `database locked`.
Les autres erreurs (corruption, disque plein…) restent rapportées.

La première consultation d’un ancien rapport peut encore prendre environ une
seconde. « Toutes » et les exécutions sans résultat durable continuent d’utiliser
le plan/rapport original ; elles ne bénéficient pas de l’index des seuls problèmes.
Il n’y a pas de promesse que tout accès SQLite ne puisse jamais échouer.

**Compatibilité :** un ancien exécutable comprenant seulement le schéma v1
refusera l’historique v2. La migration sera appliquée au prochain démarrage du
nouvel exécutable ; les essais ont migré uniquement une copie isolée du dernier
rapport, pas la base utilisateur. Pour une sauvegarde manuelle, fermer GUI et
agent puis conserver tout le dossier d’état ; ne pas copier seulement le `.sqlite`
pendant une écriture WAL. Aucune base étrangère ou plus récente n’est réinitialisée.

## Mesures plafonnées

Le NAS reste en lecture seule. Chaque essai utilise un nouveau dossier vide
sous `D:\sync\1`, un budget de **256 Mio** et des fichiers entiers. La destination
est préparée avec de petites anciennes données synthétiques ; ces seules données
créées pour le test peuvent être remplacées. Les versions sont conservées dans
un magasin isolé sous `target`. Aucun fichier utilisateur préexistant n’est écrasé.

| Chemin testé | Fichiers remplacés | Octets NAS vérifiés | Exécution |
| --- | ---: | ---: | ---: |
| Séquentiel versionné | 2 003 | 241 585 075 | 166,70 s |
| Remplacements parallèles, quatre workers | 2 003 | 241 585 075 | 77,61 s |
| Sonde supplémentaire des métadonnées | 2 003 | 241 585 075 | 80,63 s |

La sonde ajoutait un cache de préparation des dossiers de versions. Aucun gain
n’étant démontré sur cet échantillon, ce cache et son mécanisme de retry ont été
retirés ; les tests de revalidation des parents sont conservés. La suppression
de la lecture Windows redondante avant ouverture sans suivi est conservée.
Les mesures combinées ne permettent pas d’isoler son gain propre. Le résultat
principal démontré est le gain d’environ **2,1 fois** des remplacements parallèles.
Total NAS de cette reprise : **724 755 225 octets**, moins de 750 Mo, zéro
refus/échec/annulation et aucun temporaire restant lors de ces trois essais.

Ces lots mélangent 2 000 petits fichiers et trois plus gros. Leur moyenne ne
représente pas le débit des gros fichiers : elle inclut les métadonnées et les
versions. Les anciennes données synthétiques ne reproduisent pas l’archivage
d’un ancien fichier de 5,4 Go ; cet essai géant n’a volontairement pas été lancé.
**Un débit constant de 80–100 Mo/s n’est pas démontré**, ni garanti sur les petits
fichiers, une attente système, la vérification ou l’archivage du dernier gros fichier.

Historique : copie isolée de **224,4 Mio** contenant exactement le dernier plan
et son rapport, 159 problèmes dont 34 refus et 125 ignorés. Première préparation
avec la requête intermédiaire : 13 901 ms ; après le parcours unique : **1 305 ms**.
Filtres suivants : refus 2 ms, échecs 1 ms, inachevés 1 ms, ignorés 2 ms, problèmes
2 ms. L’ouverture avec contrôle d’intégrité de cette copie prend 125 ms ; il ne
s’agit pas d’une mesure du démarrage sur toute la base utilisateur de 1 Go.

## Reproduction et validation

```powershell
cargo build --release -p sync-orchestrator --examples --all-features --locked
# NOUVEAU dossier vide obligatoire ; plafond total par invocation : 256 Mio.
target/release/examples/copy_sample_timing.exe '\\NAS\testsync' 'D:\sync\1\nouveau-test' "$PWD/target/nouveau-audit" 4 256 --confirm --replacements
# Seulement sur une COPIE ISOLÉE de l’état :
target/release/examples/history_filter_timing.exe 'F:\chemin\etat-isole' 'profil' 'execution'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
$env:RUSTFLAGS='-D warnings'
cargo check --workspace --all-targets --all-features --locked
cargo build --release -p syncengine-desktop -p syncengine-cli -p syncengine-agent --all-features --locked
```

Les tests couvrent notamment quatre remplacements versionnés réellement
simultanés, collision de création de parent, disparition/changement d’un parent
déjà préparé, observation fraîche en miroir, source changée pendant lecture,
fichiers non planifiés, compteurs d’E/S avant publication, migration v1, filtres
paginés et lectures avec un écrivain actif (index présent ou absent). Les tests
existants d’annulation, erreurs fatales, versions, liens, ACL et zéro suppression
sur source indisponible restent requis. Linux et les serveurs de protocoles
isolés ne sont pas exécutés ici ; la GUI n’est pas validée visuellement.

Résultats Windows finaux : **546 tests réussis, zéro échec, dix ignorés**
(neuf fixtures de protocoles isolés et un test de performance au million
d’entrées). `fmt`, `clippy -D warnings`, `check` avec `RUSTFLAGS=-D warnings`
réussissent. Les trois exécutables release sont reconstruits, version **0.1.6,
windows-x86_64** ; leurs empreintes sont dans
`target/release/SHA256SUMS-stability.txt`. Aucune installation ni publication.
Un contrôle ciblé du bureau lancé dans le bac à sable échouait sur deux tests
de validation native Task Scheduler. Relancé hors bac à sable comme la suite
complète, il réussit : 35 tests, zéro échec, sans installation de tâche.

Périmètre : `sync-executor` (options, source, pool et observateurs/tests),
`sync-versioning` (concurrence, parents et observateur/tests), `sync-copy`
(relecture observée), `sync-events` (activité auxiliaire), `connector-local`
(lecture Windows), façade (snapshots, activation miroir, historique/migration
et tests), `desktop/profile.rs` et `model.rs` (activité, explication miroir,
cause précise des refus), exemples
plafonnés et documentation. Pas de nouvelle dépendance ni de changement manuel
de version. Les fichiers des essais et la copie d’historique contenant des noms
réels ont été supprimés après mesure. Seuls les courbes et bilans agrégés sont
conservés dans `target/stability-audit` ; les dossiers de test sur D contiennent
au total moins de 750 Mo et restent disponibles.
