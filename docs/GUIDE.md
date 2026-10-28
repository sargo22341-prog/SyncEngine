# Guide d'utilisation

SyncEngine synchronise des dossiers selon des **profils**. Chaque exécution suit
le même cycle : **simulation** (rien n'est écrit) → lecture du plan →
**exécution confirmée** de ce plan exact.

## Mettre une exécution en pause

Pendant une simulation ou une exécution, cliquez sur **Pause** dans la fiche du
profil, le suivi d’activité ou le contrôle latéral. Le même bouton est disponible
dans la fenêtre de suivi de l’agent, y compris dès l’ouverture de session.
Cliquez sur **Reprendre** pour continuer le même travail.

La pause est coopérative : elle attend le prochain point sûr de l’inventaire,
du calcul d’empreinte ou du transfert par blocs. « Pause demandée » indique la
demande, pas que tous les appels système se sont déjà arrêtés. Un appel réseau,
une opération de publication ou une phase CPU de comparaison/planification déjà
engagée peut finir avant l’attente. Les verrous restent détenus : un autre job
ne peut pas prendre la même destination pendant la pause. **Annuler** reste
disponible et réveille les workers ; **Arrêter l’agent** conserve son sens
d’arrêt des planifications.

Exemple : pendant la copie d’un gros fichier, cliquez sur **Pause**, puis sur
**Reprendre** ; le transfert poursuit le même flux temporaire sans recommencer
le fichier. Une pause ne survit pas à l’arrêt du processus et n’est pas une
reprise après redémarrage. Avec l’agent, comptez le délai d’observation (environ
0,5 s) pour voir le bouton et la progression actualisés.

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

## Progression de l’analyse réseau

Pendant l’analyse, « entrées analysées (2 côtés) » additionne source et destination.
Le débit est récent : si le côté local finit avant le NAS, il peut baisser
brusquement alors que l’analyse réseau continue normalement.

« Dossiers à cohérence incertaine » désigne une limite déclarée du connecteur,
comptée par dossier, pas une lecture échouée. SMB et les autres fournisseurs non
caractérisés gardent cette incertitude ; une absence ne permet donc pas de
suppression miroir. « Erreurs de lecture » compte séparément les problèmes réels
(listing, noms non représentables, métadonnées ou empreintes indisponibles).
Exemple : 500 dossiers à cohérence incertaine et 0 erreur signifie qu’aucune
anomalie de lecture n’a été rapportée, sans garantir un instantané cohérent du NAS.

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
- Si la **corbeille** du profil est activée, les fichiers remplacés ou
  supprimés y sont gardés (voir [Corbeille](#corbeille)).
- Un journal durable est tenu pendant l'exécution, puis supprimé dès que le
  bilan est enregistré dans l'historique : après une coupure, il reste, l'état
  partiel est connu et la prochaine simulation repart d'un nouveau scan.
- Les liens symboliques ne sont jamais suivis.
- Une seule exécution à la fois par compte utilisateur (interface, CLI et agent
  partagent un verrou).

## Corbeille

Chaque profil a sa corbeille, réglée dans l'étape **Sécurité** de l'assistant :
« Corbeille : activée / désactivée » et, si elle est activée, « Taille maximale
(Go) » (5 par défaut). Elle concerne les **remplacements** autant que les
suppressions, quel que soit le mode.

- **Nouveaux profils** : corbeille désactivée. Les fichiers remplacés ou
  supprimés ne sont alors pas récupérables ; les autres garde-fous (plafond de
  suppressions, confirmations) restent identiques.
- **Profils créés avant cette version** : ils gardaient toujours une copie ;
  ils passent à une corbeille activée de 5 Go.
- **Rangement** : `Corbeille/<identifiant du profil>/<exécution>/…` dans le
  dossier d'état (jamais sur la destination). L'identifiant, et non le nom,
  désigne le profil.
- **Rétention** : au plus 30 synchronisations par profil, comme l'historique.
  Au-delà de la taille maximale, les synchronisations les plus anciennes sont
  retirées d'abord ; la dernière et une synchronisation interrompue sont
  toujours gardées. Une synchronisation retirée de l'historique perd aussi sa
  corbeille.
- **Synchronisation plus grosse que la corbeille** : la simulation l'annonce,
  par exemple « Cette synchronisation va remplacer ou supprimer 1,0 To. La
  corbeille (5,0 Go) ne peut pas les garder : ces fichiers seront supprimés
  définitivement. » C'est tout ou rien : aucune copie n'est faite pour cette
  synchronisation. Lancée à la main, elle exige de cocher « Je comprends,
  supprimer définitivement » ; planifiée (ou `syncengine profile run`), elle
  est **bloquée** avec le code `trash_insufficient` (« corbeille insuffisante,
  lancez-la manuellement »).
- **Voir la corbeille** (clic droit sur un profil) : place utilisée sur la
  limite, synchronisations avec date, nombre de fichiers, taille et bouton
  « Ouvrir le dossier » ; pour chaque fichier, son chemin, « remplacé » ou
  « supprimé », sa taille et « Enregistrer sous… » (copie vers un emplacement
  choisi). « Vider la corbeille » demande une confirmation. Il n'y a pas de
  restauration en place : un miroir supprimerait de nouveau le fichier.
- **Retirer un profil** supprime sa corbeille (la confirmation l'indique).

## Après une interruption

Si une synchronisation a été arrêtée brutalement (plantage, coupure de
courant, fermeture de session, y compris une synchronisation planifiée), une
fenêtre s'ouvre au démarrage suivant :

```
⚠ Synchronisation interrompue
« Photos » — 28/10/2026 21:14 UTC
1 204 opérations terminées sur 3 018
2 fichiers en cours au moment de l'arrêt (état inconnu)
[Voir le détail] [J'ai vérifié, ignorer] [Reprendre]
```

- **Voir le détail** : vue lisible du journal de l'exécution (jamais le
  fichier brut) : d'abord les fichiers commencés mais jamais terminés, avec le
  fichier temporaire qui peut subsister, puis les refus et échecs et les
  dernières opérations terminées.
- **Reprendre** : acquitte l'interruption, retire les fichiers temporaires de
  la copie interrompue (seulement ceux nommés `.syncengine-….partial` et
  consignés dans le journal ; les autres sont listés pour vérification), puis
  lance une **nouvelle simulation complète**. Le journal n'est jamais rejoué :
  les fichiers déjà copiés apparaissent identiques et sont sautés. Il reste à
  confirmer l'exécution comme d'habitude.
- **J'ai vérifié, ignorer** : acquitte l'interruption sans rien toucher
  d'autre ; l'exécution reste signalée en échec.

Dans les deux cas, le journal est supprimé après l'acquittement. Tant que
l'interruption n'est pas acquittée, le profil reste bloqué et son journal est
conservé, même après plusieurs redémarrages. Si plusieurs exécutions sont
interrompues, elles sont présentées l'une après l'autre.

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

Sous la courbe, jusqu'à quatre lignes montrent les fichiers en cours de copie
(une par copie parallèle) : chemin, barre de progression du fichier, octets
copiés sur sa taille et débit moyen de ce fichier. Un fichier n'apparaît qu'au
bout d'environ 0,5 s, pour éviter le clignotement ; les copies plus courtes
sont résumées par « + N petits fichiers/s ». Ces informations viennent du
suivi en mémoire (et de `agent-live.json` pour une synchronisation planifiée),
jamais du journal.

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
| `desktop-runs.sqlite` | 30 dernières exécutions par profil : seulement leurs changements, erreurs et avertissements |
| `run-….jsonl` | journal d'une exécution en cours ou interrompue ; supprimé dès que son bilan est enregistré, ou après l'acquittement d'une interruption (à l'ouverture, ceux des exécutions terminées le sont aussi) |
| `Corbeille/<profil>/<exécution>/` | corbeille des profils : anciennes versions des fichiers remplacés ou supprimés |
| `versions/` | copies faites par les versions antérieures de SyncEngine ; plus alimenté |
| `desktop-ui.json` | thème, groupes repliés, comportement du bouton × |
| `agent-live.json` | progression publiée par l'agent pour l'affichage, fichiers en cours compris (format 2 ; le format 1 reste lu) — pas une source de vérité |
| `agent.lock` | verrou OS de l'agent par dossier d'état, libéré après la mort du processus |
| `agent-status.json` | dernière phase du résident ; sa présence seule ne prouve pas que le processus tourne |
| `agent-stop.request` | arrêt explicite persistant du processus résident |
| `agent-retire.request` | arrêt automatique après retrait du dernier horaire, levé au prochain enregistrement |
| `cancel.request` | demande d'arrêt de l'agent, présente tant que les planifications sont en pause |
| `desktop.lock`, `window.lock` | verrous : commande en cours, fenêtre ouverte |

Aucun mot de passe ni jeton n'y est stocké. Le dossier d'état doit être sur un
disque local, hors des dossiers synchronisés. Supprimer le programme ne
supprime pas ces données. Réglages → icône dossier ouvre ce dossier dans
l'explorateur de fichiers.

**Tout supprimer** : Réglages › Données › « Supprimer toutes les données de
SyncEngine… » liste exactement ce qui sera supprimé (dossier d'état avec
corbeilles, verrous, tâches planifiées, démarrage automatique, unités
systemd), avec les tailles. Il faut taper `SUPPRIMER` pour confirmer ; l'agent
est arrêté d'abord, un bilan s'affiche, puis SyncEngine se ferme. Il ne reste
qu'à supprimer le dossier du programme. Refusé pendant une synchronisation ;
les dossiers synchronisés ne sont jamais touchés. Inventaire et limites :
[traces laissées sur le système](TRACES_SYSTEME.md).

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
