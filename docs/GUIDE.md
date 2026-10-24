# Guide d'utilisation

SyncEngine synchronise des dossiers selon des **profils**. Chaque exécution suit
le même cycle : **simulation** (rien n'est écrit) → lecture du plan →
**exécution confirmée** de ce plan exact.

## Les trois modes

| Mode | Effet | Suppressions |
| --- | --- | --- |
| **Sauvegarde** | copie source → destination, remplace les fichiers modifiés | jamais |
| **Miroir** | rend la destination identique à la source | seulement si le profil l'autorise, avec confirmation séparée |
| **Bidirectionnel** | propage les changements dans les deux sens (deux dossiers locaux) | suppressions détectées grâce à l'état précédent |

- **Sauvegarde** : un fichier présent seulement à la destination est conservé.
  Si la destination est plus récente que la source, c'est un conflit : rien
  n'est écrasé.
- **Miroir** : la source fait foi et ses fichiers remplacent ceux de la
  destination. Les suppressions sont refusées si la source est vide ou si un
  inventaire est incomplet. Au-delà de 50 % des entrées de la destination, et
  à partir de 10 suppressions (même seuil que FreeFileSync), elles sont
  bloquées comme symptôme d'un mauvais dossier : la simulation affiche la cause
  exacte et propose **« Autoriser ces N suppressions pour cette simulation »**.
  Cet accord ne vaut que pour ce plan (« Recalculer » rétablit le plafond),
  n'est jamais enregistré ni utilisé par l'agent, et l'exécution demande
  toujours sa confirmation des suppressions. Une destination presque vide (un
  fichier obsolète sur un) n'est donc plus bloquée.
- **Bidirectionnel** : la première exécution enregistre une *baseline* (état
  de référence). Ensuite, un fichier modifié d'un seul côté est propagé, un
  fichier supprimé d'un côté est supprimé de l'autre (une *tombstone* garde la
  trace de la suppression), un fichier modifié des deux côtés devient un
  **conflit** : les deux versions restent en place, à résoudre à la main. Les
  dates ne désignent jamais un gagnant.

## Comparaison des fichiers

Réglée par profil (étape « Sécurité » de l'assistant) pour la sauvegarde et le
miroir :

- **Taille et date** (défaut) : rapide, seules les listes de dossiers sont lues ;
- **Contenu** : empreinte BLAKE3 des fichiers de même taille des deux côtés ;
  plus lent, détecte une modification qui ne change ni la taille ni la date.

Le bidirectionnel compare toujours par contenu.

## Le plan

La simulation produit un plan, une ligne par chemin, avec son motif :

| Action | Sens |
| --- | --- |
| Copier | absent de la destination |
| Remplacer | différent, la source l'emporte |
| Conserver | rien à faire |
| Supprimer | absent de la source (miroir) ou supprimé d'un côté (bidirectionnel) |
| Conflit | décision impossible sans risque : rien n'est écrit |
| Ignorer | incertitude (dossier illisible, date non fiable, collision de casse, lien…) ou suppression refusée |

Le plan affiché est celui qui sera exécuté. Si les dossiers changent entre la
simulation et l'exécution, l'opération concernée est refusée plutôt
qu'appliquée à l'aveugle.

## Garde-fous

- Un lecteur absent, un dossier illisible ou un scan partiel n'est **jamais**
  pris pour un dossier vide et n'entraîne aucune suppression.
- Une destination placée à l'intérieur de la source (ou l'inverse) est refusée.
- Chaque précondition est revérifiée juste avant chaque écriture.
- Chaque fichier est écrit dans un fichier temporaire, vérifié (empreinte ou
  taille), puis publié ; l'original n'est jamais effacé d'abord. Sous Windows,
  si un antivirus ou l'indexeur tient brièvement le fichier ouvert, la
  publication est retentée pendant environ une seconde avant d'être signalée
  en échec.
- Les fichiers remplacés ou supprimés sont conservés dans le dossier d'état
  (`versions/<exécution>/`).
- Un journal durable est tenu pendant l'exécution : après une coupure, l'état
  partiel est connu et la prochaine simulation repart d'un nouveau scan.
- Les liens symboliques ne sont jamais suivis.
- Une seule exécution à la fois par compte utilisateur (interface, CLI et agent
  partagent un verrou).

## L'interface

- **Rail** à gauche : Profils, Activité, Historique, Planification, thème,
  Réglages.
- **Barre des profils** : recherche, groupes repliables, état du dernier
  résultat ; un profil se déplace d'un groupe à l'autre par glisser-déposer.
- **Page profil** : source → destination, simulation, résultat, dernières
  exécutions, planification.

`Ctrl+N` ouvre l'assistant en sept étapes : mode, source, destination, filtres,
sécurité, planification, récapitulatif. « Enregistrer et simuler » lance
directement la simulation. Les filtres sont des motifs glob sur le chemin
relatif (`*.tmp`, `build/**/*.o`, `**/node_modules`) ; ils s'appliquent à
l'identique en simulation et en exécution.

Le bouton d'exécution est rouge si le plan contient des suppressions, et ces
suppressions demandent une case de confirmation distincte. Une tâche en cours
peut être annulée à tout moment ; l'état partiel est enregistré.

Pendant l'exécution, le débit et la courbe suivent les octets copiés bloc par
bloc (tous les 1 Mio), y compris à l'intérieur d'un gros fichier, et sont
moyennés sur une fenêtre glissante de 4 s, comme FreeFileSync ; la première
seconde affiche « mesure… ». Les fichiers locaux sont vidés sur le disque tous
les 128 Mio pendant l'écriture : le débit affiché est celui du disque, pas
celui du cache de Windows/Linux. Le nombre d'opérations par seconde est
affiché à côté (création de dossiers, petits fichiers). La relecture de
vérification d'un fichier n'est pas comptée comme débit.

Le pourcentage de l'anneau mesure **le volume** : (octets copiés + opérations
terminées) / (octets prévus + opérations prévues), la formule de FreeFileSync.
Il n'est jamais arrondi vers le haut et garde une décimale sous 10 %. Le
compteur d'opérations affiché à côté avance, lui, au nombre de fichiers : si
des milliers de miniatures passent avant quelques grosses vidéos, on peut lire
par exemple 9 906 / 41 099 opérations pour 1,4 % (831 Mo sur 58,4 Go).
Le volume validé s'affiche sous la forme « copié / prévu ».

La fenêtre n'a pas de cadre système : la barre de titre et les bords sont
dessinés par l'application. Glisser la barre déplace la fenêtre (ancrage
Windows compris), un double-clic l'agrandit ou la restaure, un clic droit
ouvre le menu système, et les bords et coins la redimensionnent. `Alt+F4`
reste actif.

Réduire la fenêtre la retire de la barre des tâches : elle reste dans la zone
de notification, d'où un clic sur l'icône la rouvre (sans icône disponible,
elle est simplement réduite). Par défaut, le bouton **×** la réduit aussi : une
tâche en cours continue.
Pour quitter : icône de la zone de notification › « Quitter SyncEngine »,
`Ctrl+Q`, ou Réglages › Fenêtre (où l'on peut aussi rétablir la fermeture
classique).

| Raccourci | Action |
| --- | --- |
| `Ctrl+N` / `Ctrl+E` | nouveau profil / modifier le profil sélectionné |
| `F5` / `Entrée` | simuler / exécuter la simulation affichée |
| `Suppr` | retirer le profil sélectionné |
| `↑` `↓` | changer de profil |
| `Ctrl+F` | rechercher |
| `Ctrl+1…5` | changer de section |
| `Ctrl+T` / `Ctrl+,` | basculer le thème / réglages |
| `Ctrl+R` | relire l'état |
| `Ctrl+Q` | quitter |
| `Échap` | fermer menu, dialogue ou assistant |
| `F1` | aide des raccourcis |

## Données de l'application

Dossier d'état : `%APPDATA%\org.syncengine.desktop` (Windows),
`~/.local/share/org.syncengine.desktop` (Linux, ou `$XDG_DATA_HOME`). Pour
essayer sans toucher à ses vrais profils, définir `SYNCENGINE_STATE_DIR` sur un
chemin absolu avant de lancer l'interface.

| Fichier | Contenu |
| --- | --- |
| `profiles.sqlite` | profils et préférences |
| `state.sqlite` | baselines, tombstones et historique |
| `scheduler.sqlite` | planifications autorisées |
| `desktop-runs.sqlite` | suivi des exécutions lancées depuis l'interface |
| `run-….jsonl` | journal de chaque exécution |
| `versions/` | copies des fichiers remplacés ou supprimés |
| `desktop-ui.json` | thème, groupes repliés, comportement du bouton × |
| `agent-live.json` | progression publiée par l'agent pour l'affichage (pas une source de vérité) |
| `cancel.request` | demande d'arrêt de l'agent, présente tant que les planifications sont en pause |
| `desktop.lock`, `window.lock` | verrous : commande en cours, fenêtre ouverte |

Aucun mot de passe ni jeton n'y est stocké. Le dossier d'état doit être sur un
disque local, hors des dossiers synchronisés. Supprimer le programme ne
supprime pas ces données.

Sauvegarde, restauration et purge de l'historique (applications fermées,
Python 3.12+) :

```sh
python scripts/state_maintenance.py backup ETAT NOUVELLE_SAUVEGARDE
python scripts/state_maintenance.py restore SAUVEGARDE NOUVEL_ETAT
python scripts/state_maintenance.py purge ETAT --keep 20 --days 90          # aperçu
python scripts/state_maintenance.py purge ETAT --keep 20 --days 90 --apply
```

La purge conserve baselines, tombstones, échecs et interruptions, et ne touche
jamais aux dossiers synchronisés.

## Limites

- Interface en français uniquement ; pas de lecteur d'écran (iced n'expose pas
  d'arbre d'accessibilité).
- Dates absolues affichées en UTC.
- Bidirectionnel limité à deux dossiers locaux.
- Pas de transaction globale : une exécution interrompue peut laisser un état
  partiel (toujours journalisé). Éviter qu'un autre programme écrive dans les
  mêmes dossiers pendant une exécution.
- Interface vérifiée sous Windows 11 ; Linux compilé et testé en CI
  (Ubuntu 24.04), affichage non vérifié manuellement.
