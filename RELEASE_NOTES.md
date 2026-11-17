# Notes de la prochaine version

Écrire les changements de la prochaine version, en Markdown (une ligne `- …`
par changement), **sous la ligne de marqueur seule ci-dessous** (jamais
dans cet en-tête). À chaque push sur `main`, la CI publie ce texte comme
description de la GitHub Release, puis vide la liste dans le commit
`Version X.Y.Z`. Sans notes, GitHub génère la liste des commits.
Détails : [docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

<!-- notes -->
- Linux : la sauvegarde vers une clé USB ou un disque exFAT/FAT fonctionne (chaque fichier échouait en « accès refusé »).
- Sauvegarde vers SFTP, FTP, FTPS ou WebDAV : les fichiers sont enfin copiés (aucune suppression déduite d’un serveur distant).

