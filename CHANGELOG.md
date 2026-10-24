# Journal des modifications

Changements de comportement, par version. Les notes courtes de la prochaine
release vont dans [`RELEASE_NOTES.md`](RELEASE_NOTES.md).

## [Non publié]

### Corrections
- Connecteur local (Windows) : la publication d'un fichier (renommage du
  fichier temporaire sur la cible, retrait du temporaire) est retentée environ
  1,3 s quand un antivirus ou l'indexeur tient brièvement un fichier ouvert
  (erreurs 5, 32, 33). Un refus persistant reste signalé en échec.
- `scripts/state_maintenance.py backup` canonicalise le dossier d'état avant le
  contrôle d'imbrication : une destination à l'intérieur de l'état, désignée par
  un chemin court Windows (8.3) ou un lien, est refusée comme les autres.

## [0.1.0]

Première version.

### Fonctionnalités
- Modes sauvegarde, miroir et bidirectionnel (deux dossiers locaux, baseline,
  tombstones, conflits conservés en place).
- Simulation obligatoire ; seul le plan affiché est exécutable. Comparaison
  par taille et date (défaut) ou par contenu BLAKE3.
- Exécution vérifiée : fichier temporaire, contrôle d'empreinte ou de taille,
  versions des fichiers remplacés ou supprimés, journal durable, annulation.
- Garde-fous : aucune suppression sur inventaire incomplet ou source vide,
  plafond de suppressions (50 % par défaut), destination imbriquée refusée,
  verrou d'exécution commun à l'interface, la CLI et l'agent.
- Interface native iced : profils et groupes, assistant en sept étapes,
  historique, planification, thèmes clair et sombre, zone de notification.
- CLI `syncengine` (`scan`, `compare`, `preview`, `execute`, `journal`,
  `profile`) et agent planifié `syncengine-agent` (Planificateur de tâches
  Windows, systemd utilisateur).
- Connecteurs expérimentaux SFTP, WebDAV HTTPS, FTP, FTPS, SMB et source
  OneDrive en lecture seule ; secrets lus dans `SYNCENGINE_SECRET_<réf>`.
- Archives portables Windows et Linux publiées par la CI.

### Formats persistés
- `profiles.sqlite` v3 (réglage de comparaison par profil),
  `scheduler.sqlite` v2, `state.sqlite` et `desktop-runs.sqlite` v1. Une base plus récente est
  refusée par un exécutable plus ancien.
- Plans et journaux d'exécution : JSON versionné.
