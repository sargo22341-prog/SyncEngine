# Journal des modifications

Changements de comportement, par version. Les notes courtes de la prochaine
release vont dans [`RELEASE_NOTES.md`](RELEASE_NOTES.md).

## [Non publié]

### Actualisation des dépendances
- Migration vers rusqlite 0.41.1 (type explicite du callback de sauvegarde),
  globset 0.4.21 et actualisation des dépendances transitives dans Cargo.lock.
- CI sous Python 3.15, dépendances réseau centralisées et warnings Rust,
  rustdoc et Python traités comme des erreurs dans tous les jobs.
- Ajout des contrôles Ubuntu 26.04 ; audit et publication sur cette LTS,
  archives Linux toujours compilées sous 24.04 pour conserver leur plancher système.

### Supprimer toutes les données de SyncEngine
- Réglages › Données : liste exacte des traces (dossier d’état et corbeilles,
  `%LOCALAPPDATA%\SyncEngine` ou `~/.local/state/SyncEngine`, tâches
  planifiées du compte, valeur `Run`, unités systemd et autostart sous Linux,
  fichiers d’envoi temporaires `.syncengine-….partial`, dossier d’état
  personnalisé référencé par une tâche), confirmation par « SUPPRIMER »,
  arrêt de l’agent, retrait, bilan et fermeture.
- Refusé pendant une synchronisation ou si un agent ne s’arrête pas ; un
  dossier contenant une racine synchronisée est refusé ; aucun lien ni
  jonction n’est suivi ; un dossier d’état personnalisé ne perd que nos noms.
- Nouvel inventaire `docs/TRACES_SYSTEME.md` et règle correspondante dans
  `AGENTS.md`. Façade : `user_lock_root`, `hold_execution_lock`.

### Interruptions et fichiers en cours
- À l’ouverture, chaque exécution interrompue (plantage, coupure, fin de
  session, planifiée comprise) est présentée : opérations terminées sur le
  total, fichiers en cours au moment de l’arrêt, avec « Voir le détail »
  (lecture du journal ligne à ligne), « Reprendre » et « J’ai vérifié,
  ignorer ».
- « Reprendre » acquitte, retire les fichiers temporaires `.syncengine-….partial`
  consignés par le journal, puis lance une nouvelle simulation complète ; le
  journal n’est jamais rejoué.
- `acknowledge_interrupted` supprime désormais le journal après
  l’acquittement et renvoie les échecs de suppression (`maintenance_warnings`
  pour `syncengine-agent acknowledge`).
- Suivi en direct : jusqu’à quatre fichiers en cours sous la courbe de débit
  (chemin, barre, octets, débit), affichés après 0,5 s, et « + N petits
  fichiers/s ». Nouveaux événements `TransferProgress`/`TransferEnded`
  (option d’exécution `transfer_events`, désactivée par défaut) ;
  `agent-live.json` passe au format 2, le format 1 reste lu.
- `sync_journal::visit_journal` et `Recovery` : relecture d’un journal sans le
  charger entièrement en mémoire.

### Corbeille par profil et journaux éphémères
- Profils v4 : réglage `trash` (activée, taille maximale). Les nouveaux
  profils de l’assistant n’ont pas de corbeille ; les profils existants
  migrent vers une corbeille activée de 5 Go, sans réécriture de leurs lignes.
  Une version antérieure refuse une base v4.
- Les copies avant remplacement ou suppression vont dans
  `Corbeille/<identifiant du profil>/<exécution>/` au lieu de
  `versions/<exécution>/` (l’ancien dossier n’est plus alimenté). Corbeille
  désactivée : aucune copie.
- Rétention : 30 synchronisations par profil, les plus anciennes retirées
  d’abord au-delà de la taille maximale ; dernière synchronisation et
  interruptions gardées.
- Tout ou rien : la simulation calcule la taille à remplacer ou supprimer ;
  au-delà de la corbeille, aucune copie n’est faite, l’exécution manuelle exige
  une confirmation explicite (`Consent::permanent_loss`) et l’exécution
  planifiée ou `profile run` est refusée (`trash_insufficient`).
- « Voir la corbeille » : place utilisée, synchronisations, fichiers paginés,
  « Enregistrer sous… », « Vider la corbeille ». Retirer un profil supprime sa
  corbeille.
- Le journal `run-….jsonl` est supprimé dès que le bilan est validé en base,
  et conservé si cette écriture échoue ou après un plantage. À l’ouverture,
  les journaux des exécutions terminées sont supprimés.

### Historique borné et démarrage rapide
- Historique desktop v3 : un résumé par tentative (`run_overview`) évite de
  relire tous les plans au démarrage et à chaque rafraîchissement. Mesure sur
  une copie réelle (2,4 Go) : ouverture 1,66 s → 7,7 ms après une migration
  unique de 12,3 s ; fichier ramené à 1,3 Mo.
- Plans enregistrés en JSON compact ; rétention bornée à 30 tentatives et
  512 Mio par profil (3 dernières tentatives toujours gardées).
- L’historique ne stocke plus que les changements, erreurs et avertissements :
  les entrées identiques sont comptées (`unchanged_omitted`), une exécution
  sans changement enregistre une liste d’opérations vide. La baseline
  bidirectionnelle n’est plus recopiée dans l’historique.
- À l’ouverture, les journaux `run-….jsonl` sans tentative associée sont
  supprimés ; à l’enregistrement d’une planification Windows, les dossiers
  temporaires `native-tasks-…` laissés par d’anciennes versions le sont aussi.
- Réglages : l’icône ouvre le dossier des données au lieu de copier son chemin.
- Les tentatives terminées des profils supprimés sont retirées à l’ouverture,
  les exécutions interrompues restent ; compaction si la base est surtout vide.
- Assistant : un partage réseau Windows exige un chemin UNC, et deux
  extrémités distantes sont refusées dès l’étape Destination. Message du moteur
  corrigé (accents perdus).

### Fusion de casse NAS vers Windows et journaux actifs
- La prévisualisation unidirectionnelle projette les variantes de dossiers sur
  une destination explicitement insensible à la casse. Les fichiers distincts
  en collision restent protégés individuellement, sans bloquer leurs voisins.
- Les plans projetés utilisent le schéma 3 et conservent les chemins source
  physiques. Les suppressions revérifient toutes les variantes de leurs parents,
  y compris après archivage ; les exclusions et scans partiels restent protégés.
- En miroir avec actualisation de la source, la copie est bornée à sa taille
  fraîche. Une croissance ou date modifiée exige une relecture BLAKE3 du préfixe
  copié ; une troncature ou réécriture de ce préfixe reste refusée.
- Le profil NAS réel a produit zéro saut/conflit en 43,69 s sans exécution.
  L’audit isolé a vérifié une copie de casse différente, un journal actif et la
  convergence du miroir en moins de 41 Mio. Détails : `docs/CASSE_CHEMINS_SMB.md`.

### Diagnostic initial des dossiers SMB différant par la casse
- Les collisions de dossiers protègent aussi tous leurs descendants : une
  énumération Windows ambiguë ne peut plus produire de suppression en miroir.
- Diagnostic en lecture seule `case_lookup_audit` et comparaison avec le
  rapprochement et la réutilisation des dossiers de FreeFileSync :
  `docs/CASSE_CHEMINS_SMB.md`. Cette protection intermédiaire a depuis été
  remplacée dans la façade unidirectionnelle par la fusion décrite ci-dessus.

### Absences SMB et conflits de noms
- Les listings natifs UNC/CIFS ne rendent plus tous les dossiers incomplets par
  défaut : seules une fin normale de toutes les pages et l’absence d’erreur
  permettent de conclure à une absence. Les autres connecteurs restent inchangés.
- Avant une suppression versionnée, la source est revérifiée après l’archivage :
  une réapparition ou déconnexion conserve la destination.
- Simulation et historique détaillent la cause des opérations ignorées : côté
  non vérifié, filtre, métadonnées absentes, type spécial ou collision de casse.
- Référence FreeFileSync, preuves NAS, limites de noms Windows et commandes :
  `docs/ABSENCE_SMB_ET_CONFLITS.md`.

### Stabilité des remplacements et consultation de l’historique
- Les remplacements avec versions utilisent le pool borné ; le connecteur du
  magasin borne aussi sa concurrence. Les créations concurrentes des dossiers
  de versions tolèrent seulement une autre création de dossier vérifiée.
- En miroir GUI/agent, les fichiers déjà sélectionnés sont copiés depuis une
  observation fraîche ; un changement pendant lecture reste refusé. Autres
  modes, conflits, destination et suppressions gardent leurs préconditions.
- Compteurs distincts pour transfert, relecture d’intégrité et conservation des
  versions ; aucun débit artificiel n’est ajouté au débit de copie.
- Historique desktop v2 : WAL, contrôles d’intégrité au démarrage, index dérivé
  des problèmes. Les lectures n’acquièrent plus de réservation d’écriture ; un
  cache historique occupé n’empêche pas de lire la projection temporaire.
- Analyse du journal réel, comparaison FreeFileSync et mesures plafonnées :
  `docs/STABILITE_COPIE_HISTORIQUE.md`.

### Copies NAS et diagnostic des opérations
- Exécution concurrente bornée des copies et dossiers indépendants ; dépendances,
  versions, vérification du contenu, journal durable et suppressions finales préservés.
- Moins de métadonnées Windows redondantes lors de la résolution confinée des
  chemins, sans cache de chemins pendant les mutations ni suivi des jonctions.
- Historique filtrable : problèmes, erreurs, refus, opérations inachevées et
  éléments ignorés ; recherche et pagination appliquées aux résultats réels.
- Bilan de copie avec premiers problèmes et accès à leur historique détaillé.
- CLI : `execute --workers 1..16` ; quatre workers demandés par défaut,
  bornés par les capacités des deux connecteurs. Bidirectionnel séquentiel.
- Mesures plafonnées et limites : `docs/AUDIT_COPIE_NAS.md`.

### Simulation NAS mesurée sur le partage de test
- La prévisualisation Windows demande FileFullDirectoryInfo, sans identifiants
  de fichiers non nécessaires au plan, accepté par le NAS qui refusait la
  classe étendue précédente. Les points reparse sont classés par une lecture
  de métadonnées confinée sans suivi ; son échec rend le dossier incomplet.
- L'identité de la racine courante et son attribut répertoire sont vérifiés
  en une requête, avant chaque dossier et à la dernière page multipage.
- Le scanner utilise jusqu'à quatre workers persistants quand le connecteur
  annonce des curseurs indépendants ; les autres restent séquentiels.
  Budget d'entrées partagé, annulation drainée, erreurs et plans préservés.
- Mesures NAS, égalité des inventaires et contrôles : docs/AUDIT_NAS_REEL.md.

### Complément réseau après mesure de 205 secondes
- Les inventaires Windows lisent des lots de 64 Kio sur la poignée de dossier
  déjà confinée, sans GetFinalPathNameByHandle ni recherche FindFirst séparée.
  Les attributs des liens, noms UTF-16, tailles et dates sont préservés.
- Le lecteur précédent reste le repli si le serveur refuse explicitement la
  classe d'information, avant toute entrée publiée. Ce refus de capacité est
  mémorisé pour éviter une nouvelle sonde sur chaque dossier.
- L'identité du dossier racine épinglé est lue une fois ; celle du chemin
  actuellement désigné est toujours revérifiée avant chaque nouveau dossier
  et avant acceptation de la dernière page d'un dossier multipage.
- Audit ciblé, mesures comparatives et limites : docs/AUDIT_SMB_205S.md.

### Inventaires réseau sans empreintes
- Les prévisualisations Windows utilisent un listing natif avec LARGE_FETCH,
  repli borné si le serveur refuse ce drapeau, sans ouverture par fichier.
- Les ouvertures no-follow remplacent un contrôle de lien redondant dans le
  cache de dossiers ; les pages intermédiaires réutilisent le curseur et une
  racine indisponible ou remplacée bloque la dernière page. Les contrôles de
  mutation restent complets. Le garde SMB redondant est évité sous Windows
  uniquement ; la vérification du montage CIFS Linux est conservée.
- FTP/FTPS utilise MLST pour les métadonnées ciblées, puis un listing complet
  du parent si MLST n'est pas disponible ou répond 550 ambigu ; aucune absence
  n'est déduite directement de ce code. Les dates MLSD/MLST sont conservées.
- Les lectures stat SFTP réutilisent la session de métadonnées ; les erreurs
  invalident la session et seules les lectures transitoires sont rejouées.
- Les dates déjà retournées par WebDAV et OneDrive sont exploitées sans requête
  par fichier. Les dates malformées sont des erreurs, jamais des valeurs devinées.
- Analyse détaillée et limites : docs/AUDIT_PERFORMANCE_RESEAU.md.

### Audit des lectures réseau
- L’analyse SMB réutilise jusqu’à 64 poignées de dossiers déjà vérifiés, comme
  l’analyse locale ; l’exécution garde ses parcours complets et contrôles de racine.
- Les listings SFTP et FTP/FTPS conservent une session par connecteur après succès,
  l’abandonnent après erreur et recommencent au plus une fois un listing transitoire
  avec une session neuve. Aucun dépôt, renommage ou retrait n’est rejoué.
- FTP/FTPS réapplique la racine absolue à chaque listing pour ne pas conserver
  un répertoire courant devenu obsolète ; SMB distingue les permissions refusées.
- OneDrive réessaie au plus trois fois les échecs transitoires de connexion GET.
  Les lectures JSON, DAV, FTP et SFTP consultent la pause entre leurs blocs.
- Les jetons de pagination cycliques échouent explicitement dès leur répétition.
- L’interface sépare les erreurs de lecture des dossiers dont le fournisseur ne
  garantit pas la cohérence. L’incertitude continue de bloquer les suppressions.

### Partages réseau Windows
- Correction de la conversion des chemins UNC étendus retournés pour les
  handles : la racine réseau reste absolue. Correctif local de
  `cap-primitives 4.0.3`, licences et provenance incluses, régressions testées
  sur des chemins fictifs sans accéder à un partage réel. Aucun échec de lecture
  n’est converti en dossier vide ni en autorisation de suppression.

### Pause et reprise
- Boutons Pause/Reprendre dans la fiche profil, le suivi d’activité, le contrôle
  latéral et la fenêtre de suivi de l’agent, dès l’ouverture de session.
- Pause coopérative de l’inventaire, des empreintes et des transferts par blocs.
  La reprise conserve le même travail et les verrous ; l’annulation réveille
  les workers en pause. Les appels système et les phases CPU de comparaison
  et de planification déjà engagés peuvent finir avant l’attente.
- Les demandes à l’agent ciblent une commande unique : une ancienne requête
  ne bloque pas une commande ultérieure. La pause ne survit pas au processus.

### Planification
- Le suivi avant ouverture des bases reprend le graphe, le pourcentage et les
  commandes de l’interface principale. Annuler vise le transfert courant ; arrêter
  l’agent suspend les suivants jusqu’à reprise explicite. Une file bornée publiée
  par le scheduler montre le profil actif et les échéances en attente, dans l’ordre
  séquentiel réel. Les deux vues partagent le dessin corrigé des flèches.
- Après un arrêt forcé, le profil propose un diagnostic explicite qui conserve
  les fichiers et les journaux, marque l’exécution en échec et acquitte sa prise
  de planification sans la rejouer. Les refus de modification et de retrait sont
  expliqués en français. Les badges du profil utilisent toute la largeur et
  passent sur plusieurs lignes sur les petites fenêtres.
- L’ouverture de l’interface pendant un travail de l’agent affiche une attente
  avec progression et réessaie automatiquement chaque seconde, sans contourner
  le verrou ni migrer les bases pendant l’occupation. Au démarrage, le résident
  attend aussi un verrou occupé, avec arrêt coopératif ; les autres erreurs
  restent fatales et diagnostiquées.
- Les erreurs du processus résident conservent leur étape de démarrage et un
  diagnostic caviardé borné dans `agent-status.json`, affiché dans Planification.
  Une nouvelle observation remplace ce diagnostic ; aucun journal ne grossit.
- Windows : résolution du compte des tâches en SID avant vérification d’appartenance,
  pour accepter le nom renvoyé par Task Scheduler sans autoriser un autre compte,
  un autre dossier d’état ou des privilèges différents.
- Agent Windows résident : une tâche utilisateur de connexion, trois relances
  espacées d'une minute après échec, horaires SQLite relus toutes les cinq
  secondes au repos, connexion WTS persistée et délais ancrés au logon réel.
  Migration contrôlée des tâches ponctuelles, exclusion des doubles instances,
  commandes `resident`, `status`, `stop`, `resume` et présence vérifiée par verrou
  OS dans la GUI. Linux conserve ses passages systemd ponctuels. La relance
  d'un processus bloqué n'est pas garantie. Les journaux des exécutions terminées
  suivent la rétention existante de 100 exécutions par profil ; les interruptions
  et les anciens journaux orphelins restent conservés pour diagnostic.
- Windows : l'enregistrement depuis un profil, un groupe ou l'assistant
  réconcilie automatiquement les deux tâches de l'agent. Le principal et le
  déclencheur de session ciblent explicitement le compte initial ; seule une
  erreur d'accès déclenche une demande UAC, sans élever l'exécution de l'agent.
  Les noms isolent compte et dossier d'état. Retirer la dernière autorisation
  active supprime les tâches partagées ; les erreurs ne sont plus présentées
  comme une indisponibilité du stockage. Le bouton d'activation disparaît.
- L'édition d'un horaire conserve les consentements valides. Une modification
  du profil les invalide ; les consentements d'un groupe hétérogène ne sont
  jamais hérités de son premier membre.
- La barre de titre affiche l'icône RGBA embarquée, identique à celle de la
  fenêtre et de la zone de notification, au lieu de l'ancien dessin vectoriel.
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
