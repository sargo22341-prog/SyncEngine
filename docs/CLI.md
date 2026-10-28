# Ligne de commande

`syncengine` utilise les mêmes moteurs que l'interface. `scan`, `compare` et
`preview` ne modifient rien ; seul `execute` écrit, et seulement dans la
destination. `syncengine help` affiche l'aide complète.

## Commandes

```sh
syncengine scan ./dossier --exclude "*.tmp" --exclude-dir "**/node_modules"
syncengine compare ./gauche ./droite [--hash] [--all]
syncengine preview ./source ./destination [--mode backup|mirror] [--json]
syncengine execute ./source ./destination --confirm --journal ./run-01.jsonl
syncengine journal ./run-01.jsonl
```

| Commande | Rôle |
| --- | --- |
| `scan` | inventorie un dossier et signale les zones illisibles |
| `compare` | liste les différences : `<` gauche seule, `>` droite seule, `!=` différent, `??` incertain, `=` identique (avec `--all`) |
| `preview` | simulation : affiche le plan (une ligne par chemin et son motif) |
| `execute` | exécution réelle du plan |
| `journal` | diagnostique un journal d'exécution, terminée ou interrompue |
| `profile` | utilise les profils d'un dossier d'état (voir plus bas) |

## Options

Filtres et comparaison (toutes commandes de scan) :

| Option | Effet |
| --- | --- |
| `--include <MOTIF>` / `--exclude <MOTIF>` | garde / écarte les fichiers correspondants (répétable) |
| `--exclude-dir <MOTIF>` | ne parcourt pas les dossiers correspondants |
| `--ignore-case` | motifs insensibles à la casse |
| `--hash` | compare aussi le contenu (BLAKE3) |

Un motif sans `/` porte sur le nom seul, sinon sur le chemin relatif.

Plan (`preview`, `execute`) :

| Option | Effet |
| --- | --- |
| `--mode backup\|mirror` | sauvegarde (défaut) ou miroir |
| `--overwrite source-wins\|newer\|never` | traitement des fichiers différents ; `newer` (défaut sauvegarde) crée un conflit si la destination est plus récente |
| `--allow-delete` | miroir : autorise les suppressions |
| `--max-delete <N>` / `--max-delete-percent <P>` | plafonds de suppression (défaut 50 % des entrées de la destination, appliqué à partir de 10 suppressions) |
| `--allow-empty-source` | admet des suppressions depuis une source vide |
| `--json` | plan JSON versionné (`preview`) ou rapport JSON (`execute`) |

Exécution (`execute`) :

| Option | Effet |
| --- | --- |
| `--confirm` | obligatoire |
| `--journal <FICHIER>` | obligatoire : nouveau fichier, hors source et destination |
| `--confirm-delete` | obligatoire si le plan supprime |
| `--verify content\|size` | relit chaque fichier déposé (défaut `content`) ou contrôle sa taille seule |
| `--versions-dir <DOSSIER>` | conserve les fichiers remplacés ou supprimés (dossier existant, à part) |
| `--plan <FICHIER>` | exécute un plan produit par `preview --json` (moins d'une heure, sans autre option de plan) |
| `--progress` | progression sur la sortie d'erreur |

Exemple de miroir avec suppressions, à partir d'un plan enregistré :

```sh
syncengine preview ./source ./destination --mode mirror --allow-delete --hash --json > plan.json
syncengine execute ./source ./destination --plan plan.json --confirm --confirm-delete --journal ./run-02.jsonl
```

Sous Windows PowerShell 5.1, la redirection `>` écrit en UTF-16 : utiliser
`| Out-File -Encoding utf8 plan.json`.

## Codes de sortie

| Code | Sens |
| --- | --- |
| 0 | rien à signaler, rien à faire, ou exécution réussie |
| 1 | différences (`compare`) ou opérations prévues (`preview`) |
| 2 | erreur d'utilisation (dont destination imbriquée dans la source) |
| 3 | inventaire incomplet, incertitudes, conflits, refus ou exécution interrompue |
| 4 | échec : aucun plan, exécution refusée, en échec ou annulée |

`Ctrl+C` demande une annulation propre ; l'état partiel reste dans le journal.

## Profils

Les profils créés dans l'interface sont utilisables depuis la CLI en
indiquant le dossier d'état ([Guide](GUIDE.md#données-de-lapplication)) :

```sh
syncengine profile list ETAT
syncengine profile providers ETAT              # connecteurs compilés
syncengine profile save ETAT profil.json       # crée ou met à jour un profil
syncengine profile preview ETAT ID
syncengine profile run ETAT ID --confirm [--confirm-delete]
```

Les sorties sont en JSON. `profile run` suit les règles d'une exécution
planifiée : si les fichiers à remplacer ou supprimer dépassent la corbeille du
profil, il est refusé (`trash_insufficient`) ; lancez alors la synchronisation
depuis l'interface, qui demande une confirmation explicite.
