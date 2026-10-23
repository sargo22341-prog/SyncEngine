# SyncEngine — Kit de contexte pour sessions IA

Ce kit constitue une **spécification de démarrage** ; il ne contient pas encore d'application fonctionnelle.

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

## État actuel : phase 02 (scan et comparaison, lecture seule)
Le workspace compile les contrats (`sync-core`, `sync-errors`, `sync-events`, `connector-api`), un connecteur local **en lecture seule** (`connector-local`), les moteurs de découverte (`sync-filter`, `sync-scanner`, `sync-compare`) et une CLI qui inventorie et compare des dossiers locaux. **Aucune** fonction de sauvegarde, de miroir ou de synchronisation n'existe encore : rien n'est jamais copié, modifié ni supprimé.

Prérequis : `docs/BUILD.md`. Exemples :
```sh
cargo run -p syncengine-cli -- scan ./mon-dossier --exclude "*.tmp" --exclude-dir "**/node_modules"
cargo run -p syncengine-cli -- compare ./source ./destination          # différences seulement
cargo run -p syncengine-cli -- compare ./source ./destination --hash --all
cargo test --workspace --all-features
```
Lecture de `compare` : `<` gauche seulement, `>` droite seulement, `!=` différent, `??` incertain (dossier illisible, date non fiable, collision de casse…), `=` équivalent (avec `--all`). Codes de sortie : 0 rien à signaler, 1 différences, 2 usage, 3 inventaire incomplet ou constats incertains, 4 échec (un dossier inaccessible n'est jamais traité comme vide).

Contrats compilés : `docs/CONTRATS.md` ; décisions : `docs/adr/` ; état de passation : `docs/ETAT_PROJET.md`.
