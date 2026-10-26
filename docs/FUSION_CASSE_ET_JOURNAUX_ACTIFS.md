# Fusion de casse NAS vers Windows et journaux actifs

## Comportement livré

En prévisualisation unidirectionnelle, les variantes de dossiers sont réunies
si le récepteur annonce explicitement une casse insensible avec conservation
du nom. `People/B/Artist/photo.jpg` et `People/b/bbno$/folder.jpg` deviennent
deux enfants d’un seul `People/B`, ou `People/b` si ce nom existe déjà côté cible.
Le NAS n’est pas renommé. Une restauration du dossier fusionné ne recrée pas les
deux arbres distincts : c’est la représentation Windows demandée par l’utilisateur.

Deux fichiers source distincts destinés au même chemin restent en conflit,
même à tailles/dates égales. Leurs voisins ne sont plus bloqués. Les collisions
de types et les destinations ambiguës restent protégées. Le bidirectionnel
conserve son contrat. Le comparateur bas niveau reste conservateur sans
`CompareOptions::with_directory_merge(true)` ; façade et preview CLI l’activent.
L’option ne projette rien vers une cible sensible à la casse ou de capacité inconnue.

## Plans et sécurité

Les plans projetés utilisent le **schéma 3**. `EntrySnapshot.origin_paths`
conserve les noms physiques source : un fichier a au plus une origine, un dossier
fusionné peut en avoir plusieurs. Sans ce champ, les anciens plans gardent leur
sémantique. Les formats 1/2 restent lisibles ; un ancien binaire doit refuser le 3.

Le chargement valide que les origines diffèrent seulement par la casse du chemin
logique, sont triées et uniques, ne figurent jamais dans l’observation destination
et ne sont pas utilisées en bidirectionnel. Les protections des chemins relatifs,
des racines et du parcours confiné sans suivi de liens restent actives.

Lectures et préconditions source utilisent le chemin physique. Écritures,
versions et préconditions destination utilisent le chemin logique. Un dossier
fusionné est créé une fois ; les dépendances de son contenu sont conservées.

Avant suppression, l’exécuteur vérifie toutes les variantes du plus proche
ancêtre source projeté, même pour un sous-arbre devenu absent. Une réapparition
dans `b` protège donc le fichier cible sous `B`. Ce contrôle est répété après
archivage. Erreur réseau, exclusion et scan incomplet ne deviennent pas absence.
Les seuils de suppression, source vide et confirmation du plan restent protégés.
La comparaison par contenu utilise les noms physiques de chaque côté pour les
empreintes ; ses compteurs comptent les chemins réellement présents.

## Journal Zigbee2MQTT

Refus retrouvé dans l’historique réel pour
`zigbee2mqtt/data/log/2026-10-26.16-20-40/log.log` :
`source_changed_during_transfer`, **2 947 916 → 2 948 385 octets**. Le journal
avait gagné **469 octets pendant la lecture**, après l’actualisation initiale.

En miroir avec actualisation de source, seuls les fichiers sélectionnés sont
toujours copiés depuis une observation fraîche. Le transfert est borné à cette
taille, même si le journal grandit ensuite. Si ses métadonnées changent pendant
la lecture, une deuxième lecture BLAKE3 vérifie exactement le préfixe copié.
Croissance et date modifiée sont acceptées si ce contenu est identique ;
réécriture du préfixe, troncature, disparition et erreurs d’E/S restent protégées.
La relecture du temporaire destination reste active selon la stratégie choisie.
Les ajouts après la limite sont repris lors de la simulation suivante.
Les autres modes restent stricts. Ce n’est pas un snapshot transactionnel de base.

## Vérification de FreeFileSync

Le code local fourni rapproche les noms dans `base/comparison.cpp::matchFolders`
et réutilise le dossier existant dans
`base/synchronization.cpp::FolderPairSyncer::synchronizeFolderInt`.
Son lecteur fourni est POSIX ; il ne décrit pas à lui seul le binaire Windows.

Une instance isolée de **FreeFileSync 14.12** a donc comparé People vers un
dossier de test vide, sans synchronisation. Les appels de lecture ont été
observés par un outil isolé dans `target`, sans nouvelle dépendance produit ni
capture d’identifiants. Le binaire lit 213 enfants dans `B`, puis 4 dans `b`,
à environ 1,2 s d’intervalle : NtOpenFile puis NtQueryDirectoryFile, classe 37,
lots de 64 Kio, sans expression ni réinitialisation explicite.

Une reproduction minimale des **mêmes** appels confond les listings quand les
noms sont ouverts immédiatement. Rejouer ces appels dans le processus FreeFileSync
reproduit aussi la confusion. Lire 256 dossiers intermédiaires, ou attendre 11 s,
permet de lire l’autre branche. Le choix d’une API différente n’explique donc pas
toute la différence ; le composant exact qui réutilise le résultat n’a pas été
attribué par capture protocolaire.

Aucun délai artificiel, pression de cache, changement d’IP, registre Windows ou
nouvelle authentification n’a été ajouté. Le parcours existant est déjà en
profondeur avec parallélisme borné. La correction porte sur le rapprochement,
les plans et leur exécution ; la mesure réelle ci-dessous valide ce profil,
sans promettre une correction universelle du cache SMB.

FreeFileSync a créé puis retiré son verrou temporaire `sync.ffs_lock`. Aucun
fichier de données réelles n’a été modifié. Sa fenêtre de diagnostic a été fermée
et les chemins LastRun d’origine restaurés après vérification que la configuration
était bien celle de cet audit.

## Mesures et reproduction

Simulation réelle, état isolé sous `target`, **sans exécution** : **43,69 s**,
deux scans complets, zéro avertissement de cohérence, **0 saut et 0 conflit**.
19 copies, 172 remplacements, 1 suppression proposée, 221 573 entrées conservées.
La suppression proposée n’a pas été exécutée. Les fichiers actifs peuvent faire
varier ces chiffres entre deux simulations.

```powershell
cargo build --release -p sync-orchestrator --example smb_absence_audit --all-features --locked
target/release/examples/smb_absence_audit.exe preview PROFIL_EXPORTÉ.json NOUVEL_ÉTAT_LOCAL
```

Copie isolée dans `\\NAS\testsync\.syncengine-case-20261026-a`, destination
sous `target`, moins de **41 Mio** cumulés sur deux passes :

- Source `People/B`, cible existante `People/b` : remplacement de
  `bbno$/folder.jpg` et création d’un voisin, contenus vérifiés.
- Journal de 16 Mio : **675 840 octets ajoutés pendant l’exécution**,
  **16 814 080 octets copiés** et comparés au préfixe source.
- Deuxième passe statique, suppression d’un fichier de test obsolète et
  simulation finale sans copie/remplacement/suppression/saut/conflit.

```powershell
cargo build --release -p sync-orchestrator --example case_merge_audit --all-features --locked
target/release/examples/case_merge_audit.exe '\\NAS\testsync\.syncengine-case-NOUVEAU' 'F:\Project\SyncEngine\target\case-fixture-NOUVEAU' --confirm-fixture
```

L’audit exige des répertoires inexistants et ne sait exécuter aucun profil réel.
Les sources réellement distinctes `B/b`, cibles vides/existantes, répétition,
conflits de fichiers, exclusions, réapparition avant suppression, format altéré
et hachages avec casse différente sont également testés via le stockage mémoire.

## Fichiers concernés

- `sync-compare` : projection, origines et candidats physiques de hachage.
- `sync-plan` : origines, schéma 3, validation et test de version future.
- `sync-decisions` : transmission des origines au plan.
- `sync-executor` : chemins source, absence sur toutes les variantes, préfixes
  actifs, tests `case_merge.rs` et `memory.rs`.
- `sync-copy` : limite de lecture ; `sync-verify` : vérification de préfixe.
- Connecteur mémoire : injection d’un changement de contenu à l’ouverture.
- Façade, preview CLI, audit NAS, architecture, diagnostics et notes de version.
- Test `sync-session/tests/session.rs` : diagnostic d’assertion enrichi pour
  l’échec intermittent constaté, sans modification de son comportement métier.

Contrôles : fmt, clippy tous crates/cibles/features avec `-D warnings`, tests
workspace toutes features, check toutes cibles/features avec `RUSTFLAGS=-D warnings`.
Tests ACL et Task Scheduler dans le contexte utilisateur Windows réel.
Linux n’a pas été exécuté dans cette session.

Résultat Windows final : **561 tests réussis, 0 échoué, 10 ignorés** (fixtures externes
et volumétrie opt-in). Le test de version future utilise désormais 999, puisque
le schéma 3 est pris en charge ; la première exécution de suite avait signalé
cette assertion devenue obsolète, corrigée avant la validation complète.

Une passe ultérieure a relevé un échec intermittent de
`sync-session::local_sqlite_cycle_and_verified_versions_are_real_files` : baseline
non validée. Le test a réussi 12 fois isolément puis dans la suite complète sans
compilation concurrente. Sa cause n’a pas été déterminée ; le diagnostic de son
assertion affiche désormais le rapport complet. Aucun comportement de session
bidirectionnelle n’a été assoupli pour faire passer ce test.

Livraison : compilation release Windows x86_64 0.1.6 des trois exécutables
standards, empreintes dans `target/release/SHA256SUMS.txt`. Répertoires de test
NAS/local, états temporaires, outils de traçage et références FreeFileSync de
diagnostic nettoyés ; journaux de validation conservés dans `target`.
