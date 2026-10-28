# Débit de copie et stabilité de la courbe

Analyse du 28 octobre 2026 (Windows 11, 34 Go de RAM). Symptômes signalés :
la courbe oscillait entre 15 et 50 Mo/s alors qu’un fichier unique
progressait à 50 Mo/s, et trois vidéos (2 × 1,4 Go, 0,9 Go) étaient copiées à
un débit fluctuant, quand FreeFileSync reste constant (113 Mo/s, la limite
d’un réseau gigabit, vers le NAS).

## Méthode

`crates/application/sync-orchestrator/examples/copy_throughput.rs` exécute un
vrai plan de sauvegarde (créations seules) avec les options de la façade,
échantillonne toutes les 200 ms le compteur bloc par bloc et calcule la
courbe exactement comme l’interface (fenêtre glissante de 4 s, première
seconde ignorée).

```powershell
cargo build --release -p sync-orchestrator --example copy_throughput
target\release\examples\copy_throughput.exe 'D:\source' 'E:\nouvelle-destination' 4 --confirm '*.iso'
```

La destination doit être un nouveau dossier. Chaque essai prend des fichiers
sources jamais lus : un fichier déjà lu vient du cache du système, ce qui
masque le disque source. Disques : deux disques durs SATA 4 To
(Seagate ST4000DM004/DM005, enregistrement SMR) et un SSD NVMe.

## Différences avec FreeFileSync et mesures

| Point | FreeFileSync | SyncEngine avant | Effet mesuré |
| --- | --- | --- | --- |
| Copies parallèles | une opération à la fois par défaut | jusqu’à quatre gros fichiers en même temps | 29 Mo/s (4 workers) contre 62 Mo/s (1 worker), disque dur vers disque dur |
| Vidage disque | aucun pendant la copie | `sync_data` tous les 128 Mio dans le fil de copie | lecture arrêtée pendant chaque vidage ; paliers de 128 Mio |
| Coût d’un vidage | — | vide aussi le cache de l’appareil | 55 Mo/s par pas de 8 Mio, 72 Mo/s par 16 Mio, contre 102–106 Mo/s par 128 Mio |
| Vérification | désactivée par défaut | relecture BLAKE3 du temporaire | conservée (intégrité d’abord) ; la relecture vient du cache |
| Courbe | octets acceptés par le cache, fenêtre de 4 s | même calcul, mais compteur par paliers de 128 Mio | oscillation de ±33 Mo/s (128 Mio / 4 s) |

Le calcul de la courbe n’était pas faux : il suivait honnêtement un compteur
qui avançait par paliers. Avec un vidage tous les 128 Mio, l’écriture
remplissait le cache à la vitesse de la mémoire puis s’arrêtait pendant le
vidage. Sur une fenêtre de 4 s, la moyenne voyait une ou deux marches selon
l’instant : 33, 67, 100 ou 134 Mo/s. À 50 Mo/s, une marche dure 2,7 s, d’où
les valeurs observées entre 15 et 50 Mo/s.

FreeFileSync ne vide pas le disque : sa progression est celle du cache, que
Windows freine quand trop de données attendent ; vers un NAS, le réseau
impose un rythme régulier. SyncEngine garde son `sync_all` avant publication
(intégrité) ; la correction porte donc sur la manière d’attendre le disque.

## Corrections

1. **Un seul gros fichier à la fois** (`ExecutionOptions::large_transfers`,
   défaut 1, seuil `LARGE_TRANSFER_BYTES` = 8 Mio). Les petits fichiers,
   dominés par la latence d’ouverture, continuent d’utiliser les autres
   workers ; le gain mesuré sur NAS pour les petits fichiers est conservé.
2. **Vidage en arrière-plan** (`connector-local/src/flush.rs`) : un fil
   dédié vide le temporaire par une seconde poignée pendant que la copie
   continue ; l’écriture n’attend que si plus de deux pas restent à vider.
   La lecture de la source chevauche donc l’écriture sur la destination.
3. **Vidage des données seules sous Windows**, sans vider le cache de
   l’appareil (`NtFlushBuffersFileEx` avec `FLUSH_FLAGS_FILE_DATA_ONLY |
   FLUSH_FLAGS_NO_SYNC`), tous les 16 Mio : la progression suit le disque à
   16 Mio près (±4 Mo/s sur 4 s) sans le coût des vidages complets. Si le
   système de fichiers refuse cette requête (redirecteur réseau, FAT…), ou
   hors de Windows, `sync_data` est utilisé tous les 128 Mio, toujours en
   arrière-plan.
4. La courbe date chaque échantillon à la lecture du compteur, pas à l’image
   qui l’a déclenchée.

## Résultats

| Essai (même machine) | Avant | Après |
| --- | ---: | ---: |
| SSD → disque dur, 1,4 Go | 102–106 Mo/s, courbe par marches de 33 Mo/s | 117–125 Mo/s, p10–p90 110–152 |
| Disque dur → disque dur, 2 × 2 Go, 1 worker | 62–70 Mo/s | 120 Mo/s |
| Disque dur → disque dur, 4 fichiers de 1,1–1,4 Go, 4 workers | 29 Mo/s | 102 Mo/s, p10–p90 90–115 |

Les variations restantes sont celles des disques eux-mêmes (disques SMR,
position sur le plateau). Entre deux fichiers, la relecture de vérification
et la publication laissent une courte baisse.

## Limites

- Le partage réseau (NAS) n’a pas été mesuré dans cette session : aucun essai
  n’a écrit sur le NAS de l’utilisateur. Le redirecteur SMB peut refuser le
  vidage des données seules ; le repli `sync_data` tous les 128 Mio
  s’applique alors, en arrière-plan.
- Linux n’a pas été compilé ni mesuré ici ; il utilise `sync_data` tous les
  128 Mio en arrière-plan (`sync_file_range` n’est pas utilisé : il exigerait
  du code `unsafe` ou une dépendance supplémentaire).
- Le vidage des données seules n’est pas une barrière de durabilité ; il ne
  sert qu’au rythme. Le `sync_all` final reste fait sur la poignée d’écriture.
