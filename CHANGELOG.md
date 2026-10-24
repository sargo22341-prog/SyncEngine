# Journal des modifications

Changements de comportement, par version. Les notes courtes de la prochaine
release vont dans [`RELEASE_NOTES.md`](RELEASE_NOTES.md).

## [Non publié]

### Corrections
- Garde-fou de suppression : le plafond relatif (50 % des entrées de la
  destination par défaut) ne s'applique qu'à partir de 10 suppressions
  (`MIN_DELETIONS_FOR_PERCENT`, seuil de FreeFileSync). Un miroir vers une
  destination ne contenant qu'un fichier obsolète supprimait 1 sur 1 = 100 % et
  était refusé. Source vide, inventaire incomplet et plafond absolu restent
  vérifiés sans seuil. Les plans enregistrés gardent le même format.
- Interface : la simulation affiche la cause exacte du refus (source vide,
  inventaire incomplet, plafond) au lieu d'une liste générique, et propose de
  lever le plafond pour cette seule simulation (`DeletionCeiling::Lifted`,
  `Application::preview_paged_with`). Source vide et inventaire incomplet ne
  sont jamais levables ; l'agent planifié garde toujours les plafonds ; le plan
  enregistre le garde-fou réellement appliqué ; la confirmation des
  suppressions reste exigée à l'exécution.
- Débit : `JobSnapshot::bytes_transferred` suit la copie bloc par bloc (événements
  de progression de l'exécuteur tous les 1 Mio, désormais branchés par
  l'orchestrateur, y compris en bidirectionnel via `SessionRequest::events`).
  Le compteur précédent n'avançait qu'à la validation de chaque fichier :
  0 o/s pendant un gros fichier puis un pic de plusieurs Go/s. La courbe est une
  moyenne glissante sur 4 s (méthode `SpeedTest` de FreeFileSync), la première
  seconde n'est pas estimée, et les opérations/s sont affichées.
- Connecteur local : le fichier temporaire est vidé sur le disque
  (`sync_data`) tous les 128 Mio pendant l'écriture, afin que la progression
  reflète le débit réel du disque et non le remplissage du cache suivi d'un
  long `sync_all`. Mesuré sans surcoût sur un fichier de 2 Go (Windows 11).

### Livraison
- La GitHub Release ne publie plus que les deux archives. Manifeste,
  inventaires, notices séparées, `LICENSE.txt` et `SHA256SUMS` ne sont plus des
  fichiers de release ; les empreintes SHA-256 sont ajoutées à la description.
- L'archive ne contient plus que `syncengine-desktop`, `syncengine`,
  `syncengine-agent`, `LICENSE.txt` et `THIRD_PARTY_NOTICES.txt` (plus de
  `notices/license-inventory.json`, `tools/state_maintenance.py`, ni
  `syncengine.png`, qui n'était utilisé par rien ; l'outil de maintenance reste
  dans `scripts/` du dépôt).
- Connecteur local (Windows) : la publication d'un fichier (renommage du
  fichier temporaire sur la cible, retrait du temporaire) est retentée environ
  1,3 s quand un antivirus ou l'indexeur tient brièvement un fichier ouvert
  (erreurs 5, 32, 33). Un refus persistant reste signalé en échec.
- `scripts/state_maintenance.py backup` canonicalise le dossier d'état avant le
  contrôle d'imbrication : une destination à l'intérieur de l'état, désignée par
  un chemin court Windows (8.3) ou un lien, est refusée comme les autres.

- SFTP (Windows) : environ une connexion sur 256 échouait à l'échange de clés
  (`Session(-8)`) quand le secret Diffie-Hellman commençait par un octet nul
  (backend WinCNG de libssh2). Correctif upstream libssh2#2583 appliqué dans
  `vendor/libssh2-sys` en attendant une version publiée.

### CI
- Serveur SFTP de test : une connexion coupée par le client sous Windows
  (`ConnectionResetError`) n'arrête plus le serveur.
- Tests agent : relance bornée (5 s) d'une commande refusée `busy` à cause
  d'un verrou hérité brièvement par le processus lancé par un autre test.
- Les tests de refus d'accès sont déclarés `NON EXÉCUTÉ` quand l'OS ignore le
  refus (administrateur Windows avec privilège de sauvegarde, comme sur les
  runners GitHub) au lieu d'échouer ; ils restent exécutés sous Ubuntu.
- `cargo audit --deny warnings` devient bloquant : seuls six avis transitifs
  documentés (iced 0.14, `zbus` 5) sont tolérés, tout nouvel avis bloque.

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
