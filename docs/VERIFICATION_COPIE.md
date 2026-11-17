# Vérification après copie : état des lieux

Analyse du 17 novembre 2026. Question posée : la vérification des fichiers
copiés ralentit-elle la copie, et la sortir dans une étape séparée
(« simulation → synchronisation → validation → bilan ») ou la rendre facultative
comme dans FreeFileSync ferait-il gagner du débit ?

Aucun comportement n'est modifié par ce document.

## Ce que fait SyncEngine aujourd'hui

Chemin d'une copie ou d'un remplacement (`sync-executor/src/run.rs`,
`transfer` puis `publish`) :

1. Lecture de la source et écriture dans un **temporaire**. L'empreinte BLAKE3
   du flux est calculée **dans la même boucle** (`sync-copy`, `stage_copy`),
   quel que soit le mode : c'est elle que la vérification compare ensuite.
2. `finish` : `sync_all` final du temporaire (durabilité).
3. Contrôle de taille et revérification des métadonnées de la source.
4. Signal `Staged` : l'emplacement de copie est **libéré**, la copie suivante
   démarre (`run/parallel.rs`).
5. Si `VerifyMode::Content` (défaut) : relecture complète du temporaire par la
   poignée d'écriture (`read_back`, connecteur local et SMB monté) et
   comparaison BLAKE3. Pour un connecteur sans `read_back` (SFTP, FTP/FTPS,
   WebDAV, clouds via `connector-staging`), le temporaire distant est relu par
   le connecteur, donc **téléchargé à nouveau**.
6. Publication (renommage) seulement si la relecture est conforme.

La vérification n'a donc pas lieu « en même temps » que la copie d'un même
fichier : elle est **pipelinée**. Le fichier N est relu pendant que le
fichier N+1 est copié, par deux threads de plus (`VERIFY_THREADS`), dans la
limite de `transfer_concurrency` des deux connecteurs.

`VerifyMode::Size` existe déjà : taille et empreinte du flux, sans relecture.
Il est exposé par la CLI (`--verify size`) mais **pas** par l'application de
bureau ni les profils : la façade (`sync-orchestrator`) utilise toujours
`Content`.

## FreeFileSync (référence)

Lu dans un miroir GitHub du code (`hkneptune/FreeFileSync`,
`Source/base/synchronization.cpp` vers 647–680 et 2287–2320, `Source/config.h:156`) :

- l'option « Vérifier les fichiers copiés » est **désactivée par défaut**
  (`verifyFileCopy = false`) ;
- activée, la vérification se fait **juste après chaque fichier**, dans le même
  fil, avant le fichier suivant : pas d'étape séparée ;
- elle a lieu **après** la publication du fichier (le fichier cible est
  supprimé en cas d'écart) ;
- elle relit **la source et la destination** et les compare octet par octet,
  après un `fsync` de la cible ; le code note lui-même que les tampons du
  système ne sont pas invalidés (« snake oil??? »).

Activée, la vérification de FreeFileSync coûte donc plus cher que celle de
SyncEngine (deux relectures, bloquantes) et protège moins (publication avant
contrôle). FreeFileSync n'a pas de phase « validation » distincte.

## Mesures

Machine de la session : conteneur Linux, 4 cœurs, 15 Go de RAM, disque
virtualisé `virtio` (rapide, ~600 Mo/s en écriture). CLI compilée en release,
mode sauvegarde vers une destination vide, cache vidé
(`echo 3 > /proc/sys/vm/drop_caches`) avant chaque essai, 4 workers.

| Jeu | `--verify content` (défaut) | `--verify size` |
| --- | ---: | ---: |
| 6 × 1 Gio | 11,56 · 9,65 · 9,71 s | 9,33 · 10,12 · 9,94 s |
| 20 000 × 32 Kio (40 dossiers) | 9,82 · 9,39 · 9,08 s | 9,75 · 8,80 · 9,08 s |

Écart dans le bruit de mesure (le premier essai « content » à 11,56 s est
isolé). Ici la relecture vient du cache et le BLAKE3 tourne sur d'autres cœurs :
**la vérification pipelinée ne coûte rien de mesurable**.

Validation **différée** (copie en `size`, puis relecture BLAKE3 de toute la
destination avec `syncengine scan <dest> --hash`) :

| Jeu | Copie | Validation, cache chaud | Validation, cache vidé |
| --- | ---: | ---: | ---: |
| 6 × 1 Gio | 14,68 s | 1,84 s | 2,83 s |
| 20 000 × 32 Kio | 9,57 s | 0,51 s | 1,45 s |

Une phase séparée **s'ajoute** à la copie : +19 % (gros fichiers) et +15 %
(petits fichiers) sur ce disque très rapide en lecture, alors que la même
relecture pipelinée était gratuite. Le hachage BLAKE3 seul dépasse 3 Go/s :
le processeur n'est pas le facteur limitant.

Limites : pas de disque dur, pas de NAS, pas de Windows dans cette session.
Les ordres de grandeur ci-dessous pour ces supports sont des **estimations**
à confirmer avec `copy_throughput` (voir [le débit de copie](DEBIT_COPIE.md)).

## Réponse à la théorie

### Séparer « synchronisation » puis « validation » : non, plus lent

- **Durée totale** : en pipeline, durée ≈ max(copie, relecture) ; en deux
  phases, durée = copie + relecture. La phase séparée ne peut être que plus
  longue ou égale.
- **Cache perdu** : relu juste après écriture, un fichier vient du cache
  mémoire (coût quasi nul). Relu à la fin d'une longue copie, il vient du disque
  ou du NAS. Estimation pour 100 Go vers un NAS gigabit (≈ 113 Mo/s) : environ
  15 min de copie, puis jusqu'à 15 min de validation, soit presque **2 ×**.
- **Sécurité** : aujourd'hui un fichier n'est publié qu'après relecture
  conforme. Une validation en fin d'exécution impose soit de **garder tous les
  temporaires** non publiés jusqu'au bout (espace disque cumulé, destination
  dans un état intermédiaire beaucoup plus long, beaucoup plus de restes en cas
  d'interruption), soit de **publier sans contrôle** puis vérifier (un
  remplacement corrompu a déjà écrasé l'ancienne version, sauf corbeille de
  versions). Les deux reculent par rapport à « intégrité d'abord ».
- **Débit affiché** : la courbe de la phase de copie serait un peu plus haute
  là où la relecture concurrence l'écriture, mais la fin réelle arriverait plus
  tard.

### Rendre la vérification facultative : gain réel seulement dans certains cas

Le moteur sait déjà ne pas relire (`VerifyMode::Size`). Le gain attendu dépend
du support :

| Situation | Coût de la relecture aujourd'hui | Gain sans relecture |
| --- | --- | --- |
| Local/SMB, fichiers plus petits que le cache libre | relecture depuis le cache, en parallèle | ≈ 0 (mesuré) |
| Disque dur, très gros fichiers (plus grands que le cache) | relecture disque qui concurrence l'écriture suivante sur la même tête | modéré, à mesurer |
| Relectures plus lentes que les copies | les threads de vérification se remplissent, les workers restent occupés, la copie finit par attendre (contre-pression) | modéré à fort |
| Connecteurs distants (SFTP, FTP/FTPS, WebDAV, clouds) | `transfer_concurrency` = 1 : envoi **puis** retéléchargement du temporaire, sans chevauchement | jusqu'à ≈ 2 × sur un lien symétrique |

Ce que l'on perd sans relecture : la détection d'un temporaire différent de ce
qui a été lu (écriture tronquée, erreur du système de fichiers, serveur distant
qui altère le contenu). Le contrôle de taille, l'empreinte du flux et la
revérification de la source restent. Comme pour FreeFileSync, une relecture
servie par le cache ne prouve pas que les données sont sur le disque.

### Le libellé « Vérification » de l'interface

Dans la vue d'exécution (`apps/desktop/src/profile.rs`, vers la ligne 657),
les étapes sont fixes : « Écriture » toujours en cours, « Vérification » et
« Bilan » toujours « à venir ». La vérification réelle tourne en même temps que
l'écriture et l'étape « Vérification » ne devient jamais active : ce libellé ne
reflète pas l'état réel (règle « capacités réelles » d'AGENTS.md).

## Recommandations

1. **Ne pas** créer de phase de validation globale après la copie : plus lent
   et moins sûr.
2. Exposer le niveau de vérification dans l'application (préférence ou profil,
   défaut `Content`), libellé honnête, par exemple « Vérifier chaque fichier
   copié (relecture, recommandé) » / « Contrôler seulement la taille et
   l'empreinte du flux (plus rapide sur les connexions distantes) ». Chantier :
   champ additif dans `Preferences::performance` ou le profil, `with_verify`
   dans `sync-orchestrator`, mention dans l'historique de l'exécution, tests.
3. Corriger les étapes affichées : « Écriture et vérification » tant que des
   octets sont copiés, puis « Vérification » tant qu'il reste des relectures
   (compteur `verified` déjà disponible), puis « Bilan » ; en mode `Size`,
   pas d'étape « Vérification ».
4. Une « validation finale » ne vaut que comme **action séparée et
   facultative** (relecture de la destination après coup, cache froid, par
   exemple `compare --hash`) pour qui veut contrôler le support lui-même ;
   jamais sur le chemin critique de la copie.
5. Avant d'aller plus loin, mesurer `content` contre `size` sur le NAS réel et
   sur un connecteur distant ; c'est là que le gain est attendu.

## Commandes de reproduction (Linux)

```bash
cargo build --release -p syncengine-cli
```

```bash
sync; echo 3 | sudo tee /proc/sys/vm/drop_caches
```

```bash
time target/release/syncengine execute SOURCE DEST_VIDE --confirm --journal /chemin/j.jsonl --verify content
```

```bash
time target/release/syncengine execute SOURCE DEST_VIDE2 --confirm --journal /chemin/j2.jsonl --verify size
```

```bash
time target/release/syncengine scan DEST_VIDE2 --hash
```
