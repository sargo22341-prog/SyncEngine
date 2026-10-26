# Absences SMB et conflits de noms — 26 octobre 2026

**Mise à jour :** la fusion explicite NAS vers Windows est maintenant livrée,
ainsi que la copie bornée des journaux actifs. Voir
[Fusion de casse et journaux actifs](FUSION_CASSE_ET_JOURNAUX_ACTIFS.md).

## Problème constaté et périmètre

Le connecteur SMB déclarait systématiquement `ListingConsistency::Unknown`.
Le scanner marquait donc chaque dossier incomplet, même après lecture réussie
de toutes ses pages. Un élément trouvé uniquement à destination devenait
`presence_unknown` à la source et était conservé. Le miroir ne pouvait pas
nettoyer ces éléments, et « Observation incertaine : aucune mutation » ne disait
pas pourquoi.

Le dernier rapport utilisateur a été lu sans modification : **104 présences
inconnues** et **20 collisions de casse**. Ce sont deux situations distinctes.
La correction porte sur le connecteur SMB natif, les préconditions de suppression
après archivage et les explications affichées. Les autres modes et connecteurs,
les politiques de suppression et les formats persistés restent inchangés.

## Ce que fait FreeFileSync

Sources consultées dans le clone fourni :

- `FreeFileSync/Source/base/parallel_scan.cpp`, vers 215–265 et 385–402 : les
  erreurs de lecture de dossiers et d’éléments sont enregistrées séparément ;
  une nouvelle tentative ou l’abandon de cette lecture est explicite.
- `FreeFileSync/Source/base/comparison.cpp`, vers 659–730 et 989–1044 : les
  erreurs sont attachées aux chemins et propagées aux descendants. Ces zones
  sont désactivées/exclues, au lieu d’être traitées comme des dossiers vides.
  Une lecture normale permet de comparer les présences des deux côtés.
- Le même fichier, vers 784–849 : rapprochement des noms, puis traitement
  explicite des groupes ambigus. `base/synchronization.cpp`, vers 1195, définit
  aussi une détection des noms en collision sans distinction de casse.

Cela confirme la distinction recherchée : lecture terminée, lecture réellement
échouée et conflit de noms. Le code FreeFileSync n’a pas été recopié. Ses règles
de rapprochement/renommage ne sont pas présentées comme intégralement portées.

Le protocole SMB distingue la fin d’énumération des erreurs d’accès, de session,
de partage ou de corruption. Le lecteur natif conserve cette distinction ; le
correctif n’invente pas une fin de lecture après une erreur.
[Spécification Microsoft SMB2 QUERY_DIRECTORY](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/29dfcc9b-3aec-406b-abb5-0b4fe96712e2).
La cohérence du cache des clients est également encadrée par les ruptures de
lease ; cela ne constitue pas un snapshot de toute l’arborescence.
[Notifications de rupture de lease](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-smb2/4f35576a-6f3b-40f0-a832-1c30b0afccb3).

## Comportement corrigé

Le connecteur UNC/CIFS natif annonce désormais la cohérence du système de
fichiers délégué (`Strong`). Cela ne rend pas chaque page fiable par décret :
le scanner doit toujours consommer toutes les pages jusqu’à leur fin normale.
Les erreurs, pages répétées, entrées omises, doublons, noms non représentables,
budget dépassé et annulations conservent leurs protections.

Une énumération complète peut établir une absence **pour préparer le plan**.
Une énumération incomplète ne le peut jamais. Le garde-fou de scan incomplet
reste global pour les suppressions du miroir ; nous n’avons pas réduit cette
protection pour imiter l’exclusion locale de FreeFileSync.

Avant chaque suppression, l’exécuteur vérifie toujours :

1. La disponibilité des racines avant la phase de suppression.
2. L’absence à la source par `stat` : seul `NotFound` la confirme. Une erreur
   réseau ou un accès refusé ne deviennent pas une absence.
3. L’état attendu de la destination, et la conservation de version si demandée.
4. **Après l’archivage d’une version**, une seconde vérification de l’absence
   à la source, puis de la destination. Un fichier peut réapparaître pendant
   cette E/S longue ; il est alors conservé à destination.

Ce dernier contrôle corrige une fenêtre réelle du chemin versionné. Il ne
promet pas une transaction atomique entre deux machines : une fenêtre résiduelle
subsiste entre la dernière vérification et l’appel de suppression.

Les connecteurs `Unknown` ou `Eventual` gardent leurs protections antérieures.
Aucune garantie de cohérence n’est ajoutée aux clouds, FTP ou autres protocoles.
Les plafonds, la protection de source vide, les filtres et les confirmations
explicites des suppressions restent obligatoires.

## Fusion de casse désormais livrée

Les vingt chemins du premier rapport n’étaient pas tous impossibles à copier :
`$` est valide. FreeFileSync rapproche les noms et réutilise le dossier existant.
La protection intermédiaire des descendants a étendu le blocage sans résoudre
la copie ; elle n’est plus la règle de la prévisualisation unidirectionnelle
vers un récepteur explicitement insensible à la casse.

La fusion demandée réunit maintenant les contenus `B/b`, `C/c`, `S/s` sous
un seul nom cible, avec leurs chemins source conservés dans le plan. Les conflits
de fichiers réellement distincts restent bloqués individuellement. Le miroir
revérifie toutes les variantes avant suppression. Le NAS n’est ni renommé ni
reconfiguré ; la distinction des deux noms de dossiers n’est pas recréée lors
d’une restauration du dossier fusionné.

La nouvelle simulation réelle termine en 43,69 s avec zéro saut et zéro conflit.
Voir [la correction, les mesures et les limites](FUSION_CASSE_ET_JOURNAUX_ACTIFS.md).

## Interface et historique

La simulation et l’historique utilisent le motif structuré du plan : côté dont
la présence n’est pas confirmée, exclusion par filtre, taille/date indisponible,
précision des dates, type spécial, observations contradictoires ou conflit de noms.
Le libellé ne fait pas passer une opération ignorée pour une copie réussie.

Les anciens rapports restent inchangés : une ancienne exécution peut encore
contenir `presence_unknown`. La nouvelle interface l’explique mais ne réécrit
pas le résultat passé. **Une nouvelle simulation est nécessaire** pour bénéficier
de la correction du moteur.

## Vérifications réelles, sans copie intégrale

### Profil réel : simulation uniquement

Une copie du profil a été enregistrée dans un état isolé sous `target`, avec
son mode miroir, ses filtres (`*.tmp`, `~$*`, `*.part`, `*.swp`) et son autorisation
de suppression inchangés. Aucune exécution n’est possible sur cette branche de
l’exemple `smb_absence_audit preview`.

Résultat : **42,52 secondes**, source et destination complètes, **zéro
avertissement de cohérence**, 104 suppressions proposées, 106 remplacements
proposés, 221 607 conservations et 20 éléments ignorés, tous pour collision de
casse. Les 2 055 473 394 octets du plan sont seulement prévus : **aucun n’a été
copié, aucune des 104 suppressions réelles n’a été exécutée**.

### Fixture UNC minuscule

Un nouveau sous-dossier `.syncengine-presence-*` a été créé uniquement sous le
partage de test autorisé `\\NAS\testsync`. La destination, les versions et l’état
sont neufs et isolés sous `target`. Aucun fichier préexistant n’est écrasé.

- Lecture complète, zéro incertitude artificielle : cinq éléments synthétiques
  locaux absents de la source sont effectivement supprimés, anciennes versions
  conservées. Simulation 11,41 ms.
- Un fichier est ajouté à la source après la simulation : sa suppression locale
  est refusée et l’ancienne destination conservée. Simulation initiale 10,64 ms.
- Une nouvelle simulation sélectionne son remplacement : copie de 18 octets,
  contenu vérifié, miroir convergent. Simulation 10,61 ms.

Le contenu total de cette fixture est inférieur à **1 Kio**. Aucun transfert des
120 Go n’a été lancé. Les répertoires de fixture et le profil exporté contenant
des chemins réels sont supprimés après validation ; les bilans agrégés restent
sous `target`. Le profil utilisateur, sa base d’historique et ses dossiers réels
n’ont pas été modifiés par ces essais.

## Tests et commandes

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
$env:RUSTFLAGS='-D warnings'
cargo check --workspace --all-targets --all-features --locked
cargo build --release -p sync-orchestrator --example smb_absence_audit --all-features --locked
# Simulation d’un profil copié : jamais d’exécution sur cette branche.
target/release/examples/smb_absence_audit.exe preview target/profil-isole.json F:/chemin/nouvel-etat
# Seulement dans deux racines neuves réservées à cette fixture :
target/release/examples/smb_absence_audit.exe fixture '\\NAS\testsync\.syncengine-presence-nouveau' F:/chemin/nouveau-travail --confirm-fixture
```

Tests de sûreté Given/When/Then :

- Énumération native multi-pages : 600 entrées, fin normale, absence explicite.
- Source entièrement lue puis miroir confirmé : suppressions possibles ; source
  inaccessible, scan incomplet ou cohérence inconnue : zéro suppression.
- Source réapparue ou déconnectée pendant l’archivage : ancienne destination
  conservée. Ce nouveau test échouerait sans la seconde vérification.
- Pages incohérentes, noms non représentables, liens non suivis, filtres, collisions
  et annulation gardent les tests existants du scanner et des stratégies.
- Simulation et historique présentent la même cause lisible, dont les collisions
  de casse et le côté dont la présence n’est pas confirmée.

Windows est la plateforme exécutée. Linux/CIFS et les serveurs de protocoles
isolés ne sont pas disponibles dans cette session ; l’interface n’est pas
validée visuellement. Le test natif substituant un dossier local dans le crate
SMB est distingué de la fixture UNC effectivement exécutée.

Résultats : **549 tests réussis, zéro échec, dix ignorés** (neuf nécessitent
des serveurs isolés et un est le test de performance au million d’entrées).
`fmt`, `clippy -D warnings`, `check` avec warnings interdits réussissent.
La première compilation de l’exemple avait un avertissement de visibilité
publique inutile ; il a été corrigé avant ces vérifications finales.

L’interface actuelle est restée ouverte. La nouvelle GUI est donc compilée
séparément par la commande ci-dessous puis copiée depuis `target/release/deps`
vers **`target/release/syncengine-desktop-absence.exe`**, sans interrompre la
fenêtre existante. Quitter complètement l’ancienne instance, y compris la zone
de notification, avant de lancer ce nouveau binaire. CLI et agent ont été
reconstruits normalement. Version **0.1.6, windows-x86_64, release** ; empreintes
dans `target/release/SHA256SUMS-absence.txt`. Aucune installation ni publication.

```powershell
cargo rustc --release -p syncengine-desktop --bin syncengine-desktop --all-features --locked -- -C extra-filename=-absence
cargo build --release -p syncengine-cli -p syncengine-agent --all-features --locked
```

Fichiers de cette correction : `connector-smb/src/mounted.rs`, documentation de
`ListingConsistency` dans `connector-api`, `sync-executor/src/run.rs` et son test
mémoire, `desktop/src/model.rs` et `profile.rs`, exemple applicatif
`smb_absence_audit.rs`, architecture, changelog et notes. Aucun nouveau crate,
aucune nouvelle dépendance, aucun changement de version ou de format persistant.
