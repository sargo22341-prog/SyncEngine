# Notes de la prochaine version

Écrire sous la ligne `<!-- notes -->
- Planification : une icône « i » explique simplement chaque option (échéances manquées, écritures, suppressions).
- Planification : option « Afficher la fenêtre de suivi » (Windows) ; la fenêtre suit en direct toute synchronisation planifiée (pourcentage, débit, erreurs) et peut l'annuler.
- Planification : un arrêt demandé à l'agent est signalé, avec un bouton « Reprendre les planifications ».
- Réduire la fenêtre la retire de la barre des tâches : elle reste accessible depuis la zone de notification.
- Miroir : une destination presque vide (ex. 1 fichier obsolète) n'est plus bloquée par le plafond de 50 %, appliqué désormais à partir de 10 suppressions.
- Simulation : la cause exacte d'un refus de suppressions est affichée, et un plafond dépassé peut être levé pour cette simulation uniquement.
- Débit d'exécution fiable : progression bloc par bloc, moyenne glissante sur 4 s, opérations/s, écriture vidée sur disque tous les 128 Mio.
- Release : seules les archives Windows et Linux sont publiées ; elles ne contiennent plus que les trois exécutables, la licence et les notices tierces.` les changements de la prochaine version, en Markdown (une
ligne `- …` par changement). À chaque push sur `main`, la CI publie ce texte comme description de
la GitHub Release, puis vide la liste dans le commit `Version X.Y.Z`. Sans notes, GitHub génère la
liste des commits. Détails : [docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

<!-- notes -->
- SMB Windows : inventaire directement par poignée, sans renormaliser chaque chemin ni relire l’identité de la racine déjà épinglée.
- Inventaires réseau : lecture Windows par lots, moins de contrôles redondants, métadonnées FTP/WebDAV/OneDrive et sessions SFTP mieux exploitées.
- Réseau : parcours SMB optimisé, sessions SFTP/FTP/FTPS réutilisées et avertissements de cohérence distingués des erreurs de lecture.
- Windows : correction des chemins UNC des partages réseau signalés à tort comme introuvables.
- Exécutions : boutons Pause/Reprendre pour l’analyse et les transferts, dans l’application et la fenêtre de suivi de l’agent.
- Suivi de l’agent : graphe de débit, annulation et arrêt accessibles dès l’ouverture, file des profils en attente et flèches de synchronisation redessinées.
- Planification : diagnostic explicite des transferts interrompus depuis l’interface et badges de profil répartis sur plusieurs lignes.
- Windows : l’ouverture de l’interface suit la progression et reprend automatiquement si l’agent occupe l’état ; l’agent attend aussi une opération manuelle en cours.
- Windows : la page Planification affiche désormais la cause précise d’un échec de démarrage de l’agent.
- Windows : correction du refus de planification lorsque le compte de la tâche est renvoyé par son nom plutôt que son SID.

