# SyncEngine — Kit de contexte pour sessions IA

SyncEngine fournit une CLI Rust et une interface Tauri 2/TypeScript pour les
sauvegardes, miroirs et synchronisations bidirectionnelles locales. Un agent
sans GUI exécute les profils programmés ; les connecteurs réseau/cloud restent
à implémenter.

Phase 06 : profils persistants, assistant, simulation, exécution confirmée,
annulation, progression agrégée, historique et paramètres d’affichage.
Construction et exemple : [README desktop](apps/desktop/README.md).

La phase 05 ajoute la synchronisation bidirectionnelle locale : baseline
SQLite, historique, tombstones et conflits conservés en place. Entrée réelle :
`cargo run -p sync-session --example local -- SOURCE DESTINATION ETAT`
(simulation par défaut, `--execute` pour appliquer). Voir le
[README de phase 05](crates/application/sync-session/README.md) pour les dossiers
requis, les protections, les versions et la reprise après interruption.
La CLI historique conserve ses commandes backup/mirror.

## Entrée rapide
1. Lire `CONTEXTE.md`, puis `AGENTS.md`, puis `docs/CONTRATS.md`.
2. Choisir **un seul** fichier `prompts/PHASE_XX_*.md` et le transmettre à une session IA avec les trois documents précédents.
3. Faire travailler chaque session dans sa branche Git ou son répertoire isolé ; ne pas fusionner deux modifications concurrentes du même contrat.
4. Demander à l'IA de générer du code réel, des tests et un compte rendu, sans prétendre avoir testé ce qu'elle n'a pas exécuté.
5. En fin de phase, exécuter les contrôles et compléter `docs/ETAT_PROJET.md`.

## Images de contexte
- `visuels/01_profils.svg`, `02_differences.svg`, `03_profil.svg`, `04_progression.svg` : **maquettes originales** schématiques, non captures SyncBack.
- `visuels/REFERENCES_SYNCBACK.md` : liens vers **captures et documentations authentiques** publiées par 2BrightSparks. Les images tierces ne sont pas redistribuées dans ce kit.

## Conventions
- Noms du produit (`SyncEngine`) et de l'interface : provisoires.
- Cibles : Windows 64 bits, Ubuntu et Debian 64 bits, support Linux plus large ultérieur.
- Crates intégrés à la compilation (option A), pas de plugins DLL/SO chargeables à chaud.
- UI Tauri 2 + frontend TypeScript ; tout le métier, les transferts et les connecteurs en Rust.
- `AGENTS.md` s'applique au dépôt et reste sous 400 lignes.

## CLI des phases 01 à 04 (exécution sécurisée locale)
Le workspace comprend les contrats, la découverte, les stratégies sauvegarde/miroir et les moteurs `sync-copy`, `sync-verify`, `sync-versioning`, `sync-journal`, `sync-executor`. `scan`, `compare` et `preview` restent sans mutation ; **`execute` modifie réellement la destination**, après confirmation. La source est ouverte en lecture seule. Les contrats de phase 04 sont soumis à revue (ADR 0004).

Prérequis : `docs/BUILD.md`. Exemples :
```sh
cargo run -p syncengine-cli -- scan ./mon-dossier --exclude "*.tmp" --exclude-dir "**/node_modules"
cargo run -p syncengine-cli -- compare ./source ./destination          # différences seulement
cargo run -p syncengine-cli -- compare ./source ./destination --hash --all
cargo run -p syncengine-cli -- preview ./source ./destination           # plan de sauvegarde
cargo run -p syncengine-cli -- preview ./source ./destination --mode mirror --allow-delete --max-delete 100
cargo run -p syncengine-cli -- preview ./source ./destination --mode mirror --json > plan.json
cargo test --workspace --all-features
```
Lecture de `compare` : `<` gauche seulement, `>` droite seulement, `!=` différent, `??` incertain (dossier illisible, date non fiable, collision de casse…), `=` équivalent (avec `--all`). Codes de sortie : 0 rien à signaler, 1 différences, 2 usage, 3 inventaire incomplet ou constats incertains, 4 échec (un dossier inaccessible n'est jamais traité comme vide).

Lecture de `preview` : une ligne par chemin — `COPIER`, `REMPLACER`, `CONSERVER`, `SUPPRIMER`, `CONFLIT` (rien n'est écrit), `IGNORER` (incertitude, lien, suppression refusée…) — avec son motif. Une sauvegarde ne supprime jamais ; un miroir ne supprime qu'avec `--allow-delete`, et jamais si un inventaire est incomplet, si la source est vide ou au-delà de 50 % des entrées de la destination (réglable). Codes : 0 rien à faire, 1 opérations prévues, 2 usage (dont destination imbriquée dans la source), 3 conflits, chemins ignorés ou inventaire incomplet, 4 échec (aucun plan).

Contrats compilés : `docs/CONTRATS.md` ; décisions : `docs/adr/` ; état de passation : `docs/ETAT_PROJET.md`.

### Exécuter et diagnostiquer (phase 04)
Les dossiers source/destination doivent exister et ne pas être imbriqués.
Chaque exécution exige `--confirm` et un **nouveau** fichier `--journal`,
hors de ces deux arbres. Exemple de sauvegarde puis diagnostic :

```sh
cargo run -p syncengine-cli -- execute ./source ./destination --confirm --journal ./execution-01.jsonl --progress
cargo run -p syncengine-cli -- journal ./execution-01.jsonl
```

Pour conserver les fichiers remplacés/supprimés, créer un dossier séparé
`./versions`, puis ajouter `--versions-dir ./versions`. Un miroir avec
suppressions exige la politique `--mode mirror --allow-delete` **et** la
confirmation `--confirm-delete` ; les plafonds de preview restent applicables.

Exécuter un plan enregistré (dans l'heure, sans changer les arbres) :

```sh
cargo run -p syncengine-cli -- preview ./source ./destination --hash --json > plan.json
cargo run -p syncengine-cli -- execute ./source ./destination --plan ./plan.json --confirm --journal ./execution-02.jsonl
```

Le fichier doit être du JSON UTF-8 (sous Windows PowerShell 5.1, utiliser
une écriture UTF-8 explicite plutôt que la redirection UTF-16 par défaut).
Les options de planification et de filtrage ne se combinent pas avec `--plan`.
Chaque précondition est revérifiée avant mutation. `--verify content`
(défaut) relit le contenu déposé et vérifie BLAKE3 ; `--verify size` ne
détecte pas une corruption de même taille. `--json` produit le rapport.
Codes `execute` : 0 succès, 2 usage, 3 réserves/refus, 4 échec/annulation ;
`journal` : 0 succès, 3 réserves ou interruption, 4 échec.

Limites : pas de verrou contre deux exécutions concurrentes sur les mêmes
arbres, fenêtre résiduelle entre contrôle des chemins et mutation, repli FAT
non atomique, pas de rétention des versions. Ctrl+C ne déclenche pas encore
l'annulation coopérative ; après interruption, lire le journal puis refaire
une preview. Un temporaire `.syncengine-….partial` peut rester sans être
consigné si l'arrêt survient pendant le transfert. Linux et CI non validés
dans cette session ; détails des tests et limites dans l'état du projet.

## Phase 07 — scheduler et agent

Agent local sans GUI, programmations UTC et adaptateurs Windows/Linux :
[utilisation, installation et reprise](apps/agent/README.md).
Contrats et limites : [ADR 0007](docs/adr/0007-scheduler-agent.md).
