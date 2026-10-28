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
- Profils : modifier les filtres, le nom, le groupe, la corbeille ou la comparaison ne décoche plus les autorisations de la planification.
- Filtres : sous Windows, les motifs ignorent toujours la casse (Thumbs.db exclut aussi thumbs.db).
- Copie : débit nettement plus élevé et courbe stable (vidage disque en arrière-plan, un seul gros fichier à la fois).
- Groupes : « Synchroniser tout le groupe » au clic droit simule puis exécute chaque profil à la chaîne, avec un bilan.
- Simulation, historique et corbeille : affichage par dossiers repliables avec le nombre d'éléments par action.
