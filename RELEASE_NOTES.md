# Notes de la prochaine version

Écrire sous la ligne `<!-- notes -->
- Conflits : nouveau choix « Garder les deux » (la version de la destination est conservée à côté, sous « nom (conflit destination).ext »).
- Réglages › Performances : transferts simultanés, gros fichiers simultanés et limite de débit.
- Planification : vue « Semaine » en calendrier.
- Assistant de création en cinq étapes (mode, source et destination sur la même page).
- Polices Inter, Space Grotesk et JetBrains Mono embarquées.
- Bidirectionnel : la courbe de débit distingue ce qui part vers la destination et ce qui part vers la source.
- Bidirectionnel : les conflits entre deux fichiers se résolvent depuis le plan (garder la source ou la destination) ; la version remplacée va dans la corbeille et rien n'est écrit avant l'exécution confirmée.
- Nouveau design « Flux » : fond neutre clair ou sombre, couleurs réservées aux signaux (lime = sauvegarde, mandarine = miroir, violet = bidirectionnel), navigation à libellés, accueil avec cartes « En direct » et « À surveiller », profils en lignes à tuile colorée, boutons à encre pleine sans lueur. Dans le plan d'une simulation, chaque dossier se retire d'un clic (« Exclure ») : le filtre est ajouté au profil et la simulation est relancée. La fenêtre ouverte par l'agent devient un mini-suivi (« Agrandir » / « Réduire ») puis l'interface complète s'ouvre à la fin. Nouvelle page « Corbeille » dans la navigation et carte « suppressions à revoir » dans le plan.
- Apparence : refonte clair/sombre façon iOS et macOS (fond noir ou blanc, cartes très arrondies, boutons en pastille avec lueur colorée, halo coloré en haut à droite selon le mode du profil), liste des profils en lignes, assistant de création avec barre d'étapes, carte de transfert aux chiffres colorés et anneau à point lumineux, icônes sans pastille ; le thème « AMOLED » est fusionné dans le sombre. Coins de fenêtre arrondis sous Windows 11.
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
- Liste des profils : le nom long est coupé par « … » et l'icône de planification et la pastille de résultat restent fixes à droite.
- Vue d'ensemble : le libellé de planification est raccourci (« Connexion + 2 min », « Toutes les 2 h ») et n'est plus tronqué.
- Historique : la vue globale liste les dernières exécutions de tous les profils, de la plus récente à la plus ancienne, 30 à la fois (« Afficher plus »).

