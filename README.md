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

## État actuel : phase 01 (fondations)
Le workspace compile des contrats minimaux (`sync-core`, `sync-errors`, `sync-events`, `connector-api`) et une CLI squelette. **Aucune** fonction de sauvegarde, de miroir, de synchronisation ou de simulation n'est encore disponible ; rien ne lit ni ne modifie de fichier.

Prérequis : `docs/BUILD.md`. Exemple :
```sh
cargo run -p syncengine-cli -- version   # syncengine 0.1.0 (windows-x86_64, debug)
cargo run -p syncengine-cli -- help
cargo test --workspace --all-features
```
Contrats compilés : `docs/CONTRATS.md` ; décisions : `docs/adr/` ; état de passation : `docs/ETAT_PROJET.md`.
