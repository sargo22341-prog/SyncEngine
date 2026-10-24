# Notes de la prochaine version

Écrire sous la ligne `<!-- notes -->` les changements de la prochaine version, en Markdown (une
ligne `- …` par changement). À chaque push sur `main`, la CI publie ce texte comme description de
la GitHub Release, puis vide la liste dans le commit `Version X.Y.Z`. Sans notes, GitHub génère la
liste des commits. Détails : [docs/RELEASE.md](docs/RELEASE.md).

<!-- notes -->

- Chaîne Rust 1.99.1 et dépendances mises à jour (cap-std 4, reqwest 0.13.5, roxmltree 0.21.2).
- Archives portables Windows (.zip) et Linux (.tar.gz) publiées automatiquement à chaque version.
- Nouvelle icône de l'application (exécutable Windows, fenêtre, barre des tâches, zone de notification).
- Logo retiré du haut de la barre de navigation.
