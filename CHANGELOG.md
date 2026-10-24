# Journal des modifications

Changements de comportement, par version. Les notes courtes de la prochaine
release vont dans [`RELEASE_NOTES.md`](RELEASE_NOTES.md).

## [Non publié]

### Planification
- Option de présentation `Schedule::show_window` (« Afficher la fenêtre de
  suivi pendant la synchronisation », `--show-window` côté agent). Ce n'est pas
  un consentement : elle est reprise de la planification existante et ne change
  ni l'autorisation ni l'exécution. Windows seulement : l'agent lance
  `syncengine-desktop --suivre-agent` à côté de lui, sauf si une fenêtre est
  déjà ouverte (verrou `window.lock` du dossier d'état) ; cette fenêtre revient
  alors au premier plan. Sous Linux, une unité systemd n'a pas de session
  graphique garantie et arrêterait la fenêtre avec elle : option non proposée.
- Base des planifications en v3 : les configurations enregistrées sont
  re-sérialisées pendant la migration (sinon la comparaison texte de la prise
  de créneau ne correspondrait plus et la planification ne partirait jamais).
  Un binaire antérieur refuse une base v3 (schéma inconnu), sans la modifier.
- Progression partagée : pendant `tick`, l'agent publie environ deux fois par
  seconde son `JobSnapshot` dans `agent-live.json` (fichier remplacé en bloc,
  format versionné `LIVE_FORMAT`, présentation uniquement). Une instance qui
  n'a démarré aucun travail n'écrit rien. `JobSnapshot` porte désormais
  `planned_operations` et `planned_bytes` pour un pourcentage honnête sans la
  simulation. API : `Application::publish_live`, `agent_live`,
  `agent_cancel_request` / `agent_cancel_clear` / `agent_cancel_pending`.
- `Application::schedule_save` prend un paramètre `show_window` (changement
  d'API interne au dépôt ; tous les appelants sont mis à jour).

### Interface
- La fenêtre suit en direct une synchronisation planifiée de l'agent (page
  Activité, carte latérale, page du profil) : analyse puis écritures,
  pourcentage, débit, refus ou échecs. Un instantané actif non rafraîchi depuis
  10 s est considéré comme perdu (« Suivi interrompu »). En fin de tâche, une
  notification donne l'issue et l'historique est relu.
- « Annuler » sur une tâche de l'agent dépose la demande d'arrêt
  (`cancel.request`), retirée automatiquement à la fin de cette tâche si la
  fenêtre est restée ouverte ; sinon la page Planification signale la pause
  et propose « Reprendre les planifications ».
- Icône « i » (infobulle) après chaque option de planification, avec une
  explication pour non-spécialistes (`ui::info_tip`).
- Réduire la fenêtre (bouton, × en mode « réduire », barre des tâches, Win+D)
  la masque (`window::Mode::Hidden`) : elle quitte la barre des tâches et reste
  dans la zone de notification. Sans icône de notification, simple réduction.
  Sous Windows, la réduction par le système est détectée par le
  redimensionnement à 0 × 0 que signale la fenêtre.
- Fenêtre sans décorations système (`decorations: false`) : barre de titre
  intégrée (`apps/desktop/src/titlebar.rs`) aux couleurs du thème, sans cadre
  ni bordure Windows. Déplacement par la boucle native (`window::drag`, ancrage
  conservé), double-clic agrandir/restaurer, clic droit menu système, boutons
  réduire/agrandir/fermer au gabarit Windows 11 (fermer rouge au survol), titre
  atténué quand la fenêtre perd le focus, poignées de redimensionnement
  invisibles de 5 px sur les bords et coins (`window::drag_resize`), retirées
  une fois la fenêtre agrandie. Le bouton fermer suit le même chemin que la
  demande de fermeture système (réglage « réduire au lieu de quitter »).
  Les coordonnées du curseur et la taille utilisées par les surcouches sont
  désormais relatives à la zone sous la barre.

### Corrections
- Progression d'exécution : un seul calcul (`Job::progress`) pour l'anneau et
  la carte latérale, pondéré comme FreeFileSync
  (`(octets + opérations) / (octets prévus + opérations prévues)`), au lieu des
  seuls octets avec repli sur les opérations. Le pourcentage est tronqué (jamais
  100 % avant la fin), avec une décimale sous 10 % (`model::percent`), et
  l'anneau précise « du volume ». La statistique « volume validé » affiche
  « copié / prévu ». Constat à l'origine : 9 906 opérations sur 41 099 pour
  1 % affiché, valeur exacte (831 Mo sur 58,4 Go) mais illisible.
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
