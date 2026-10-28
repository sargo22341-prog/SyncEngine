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
- Réglages : « Supprimer toutes les données de SyncEngine » efface toute trace du logiciel portable (données, corbeilles, tâches planifiées, démarrage automatique), après confirmation.
- Au démarrage, une synchronisation interrompue (plantage, coupure) est signalée avec « Voir le détail », « Reprendre » (nouvelle simulation) ou « J’ai vérifié, ignorer ».
- Suivi en direct : les fichiers en cours de copie s’affichent sous la courbe de débit, avec leur progression.
- Corbeille par profil (Sécurité) : désactivée pour les nouveaux profils, 5 Go pour les anciens, 30 synchronisations au maximum, avec « Voir la corbeille » au clic droit.
- Une synchronisation plus grosse que la corbeille est annoncée par la simulation, demande « Je comprends, supprimer définitivement » et reste bloquée en planification.
- Les journaux d’exécution sont supprimés dès que le bilan est enregistré ; retirer un profil supprime sa corbeille.
- Démarrage nettement plus rapide : l’historique ne relit plus tous les plans enregistrés, reste borné en taille et oublie les profils supprimés.
- Historique allégé : 30 exécutions par profil, et seuls les changements, erreurs et avertissements sont enregistrés (les fichiers identiques ne le sont plus).
- Nettoyage automatique du dossier des données (journaux orphelins, dossiers temporaires de planification) ; Réglages ouvre directement ce dossier.
- Assistant : un « Partage réseau » Windows exige un chemin \\serveur\partage (un disque de ce PC se choisit en « Dossier local ») ; message d’erreur sans accents perdus.
