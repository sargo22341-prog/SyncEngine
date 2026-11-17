# Notes de la prochaine version

Écrire les changements de la prochaine version, en Markdown (une ligne `- …`
par changement), **sous la ligne de marqueur seule ci-dessous** (jamais
dans cet en-tête). À chaque push sur `main`, la CI publie ce texte comme
description de la GitHub Release, puis vide la liste dans le commit
`Version X.Y.Z`. Sans notes, GitHub génère la liste des commits.
Détails : [docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

<!-- notes -->
- FTP/FTPS : un serveur sans MLSD (vsftpd) est refusé avec une explication claire au lieu de « non pris en charge () ».
- La page d’un profil distant signale, avant toute simulation, la variable de secret ou l’agent SSH absent ; le guide explique comment les fournir sous Linux.
- « Reprendre » après un arrêt brutal retire aussi les fichiers temporaires que l’arrêt n’avait pas eu le temps de consigner.
- Linux : enregistrer une planification installe et active l’agent systemd ; son état réel est affiché.
- Linux : « Supprimer toutes les données » arrête l’agent systemd avant d’en retirer les fichiers.
- Linux : une destination qui est une vue « mount --bind » de la source est refusée comme une destination imbriquée.
- Linux : les permissions des fichiers et dossiers sont conservées dans la sauvegarde ; un fichier privé n’y devient plus lisible par tous.
- WebDAV (Apache) : les sous-dossiers sont de nouveau lus et créés.
- Linux : la sauvegarde vers une clé USB ou un disque exFAT/FAT fonctionne (chaque fichier échouait en « accès refusé »).
- Sauvegarde vers SFTP, FTP, FTPS ou WebDAV : les fichiers sont enfin copiés (aucune suppression déduite d’un serveur distant).

