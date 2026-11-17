# Connecteurs réseau et cloud

Inventaires sans empreintes : les optimisations Windows, sessions de métadonnées
et horodatages distants sont détaillés dans
[AUDIT_PERFORMANCE_RESEAU.md](AUDIT_PERFORMANCE_RESEAU.md). Un scan incomplet
continue de bloquer toute suppression déduite de ses absences.

En plus des dossiers locaux, un profil peut utiliser un emplacement distant.
Ces connecteurs sont **expérimentaux** (étiquetés comme tels dans
l'interface) : ils ont été testés contre des serveurs de test isolés, pas
contre des fournisseurs réels.

Les versions publiées les incluent (`--features network,onedrive`). Une
compilation par défaut ne contient que les dossiers locaux ; l'assistant
affiche alors les autres connecteurs grisés.

| Connecteur | Source | Destination | Authentification |
| --- | --- | --- | --- |
| SFTP | oui | oui | agent SSH (Pageant sous Windows), clé d'hôte vérifiée par `known_hosts` |
| WebDAV (HTTPS) | oui | oui | jeton Bearer |
| FTPS (TLS explicite) | oui | oui | mot de passe |
| FTP (non chiffré) | oui | oui | mot de passe, consentement explicite au clair |
| SMB | oui | Windows seulement | celle du système |
| OneDrive | oui | non | jeton d'accès Microsoft Graph |

Le bidirectionnel reste limité à deux dossiers locaux.

Les simulations SMB réutilisent un cache borné de 64 dossiers ; les contrôles
de chemin de l’exécution sont conservés. Les listings SFTP et FTP/FTPS réutilisent
une session par connecteur, abandonnée après erreur ; un listing transitoire peut
être recommencé une fois avec une nouvelle session. Aucune mutation n’est rejouée.
Le compteur « dossiers à cohérence incertaine » est distinct des erreurs de
lecture : il ne permet jamais de conclure qu’un fichier absent doit être supprimé.
Un dossier de ce type listé jusqu’au bout sans erreur ni entrée omise donne une
absence **non vérifiée** (`Presence::AbsentUnverified`) : elle autorise seulement
la **copie** d’un élément de la source vers une destination distante. L’exécuteur
revérifie la cible (`stat`) juste avant d’écrire puis publie sans écrasement
(SFTP : `rename` v3 qui refuse une cible existante ; WebDAV : `Overwrite: F` ;
FTP/FTPS : `stat` sous le verrou de destination et `exclusive_writers`). Une cible
apparue entre-temps est un refus « destination apparue », jamais un écrasement.
Elle ne justifie ni suppression miroir, ni décision bidirectionnelle.
Exemple : sauvegarde d’un dossier local vers un serveur SFTP vide → la simulation
prévoit toutes les copies et zéro suppression ; l’exécution se termine « réussie
avec réserves » car l’inventaire distant reste non garanti.
Exemple : pour une sauvegarde NAS vers PC, choisir le partage SMB comme source
et le dossier du PC comme destination, lancer une simulation et examiner séparément
les erreurs et les incertitudes avant toute confirmation.
Périmètre testé et limites : [audit réseau](AUDIT_RESEAU.md).

## Secrets

Aucun mot de passe ni jeton n'est enregistré dans les profils. Le profil
contient une **référence** (par exemple `NAS`) et la valeur est lue au moment de
l'exécution dans la variable d'environnement `SYNCENGINE_SECRET_<référence>`
(`SYNCENGINE_SECRET_NAS`) du processus qui exécute : interface, CLI ou agent.
Une tâche planifiée doit donc recevoir cette variable dans son propre
environnement. Si elle manque, l'exécution échoue avec une erreur
d'authentification qui **nomme** la variable attendue ; la page du profil
l'annonce déjà avant toute simulation (« Identifiant absent de ce processus »).
SFTP s'authentifie par l'agent SSH : sous Linux, `SSH_AUTH_SOCK` doit être
présent dans le processus.

### Secrets sous Linux (interface du menu, agent systemd)

Une interface lancée depuis le menu (fichier `.desktop`) et l'agent lancé par
le minuteur systemd **n'héritent pas** des variables d'un terminal. Voies
documentées, sans jamais écrire le secret dans une unité :

1. Fichier `~/.config/environment.d/60-syncengine.conf`, droits `0600`
   (`chmod 600`), une ligne par référence :
   `SYNCENGINE_SECRET_NAS=…`. Il est lu par le gestionnaire systemd
   utilisateur à son démarrage (prochaine ouverture de session) et transmis
   à ses unités, donc à l'agent ; les sessions de bureau démarrées par
   systemd (GNOME, Plasma) le transmettent en général aussi aux applications
   du menu. À vérifier sur le bureau utilisé : ouvrir la page du profil,
   l'avertissement disparaît quand la variable est reçue.
2. Agent SSH : après son démarrage dans la session,
   `systemctl --user import-environment SSH_AUTH_SOCK` le rend visible aux
   unités systemd (agent planifié). Un trousseau de bureau qui fournit
   l'agent SSH le fait souvent déjà.
3. Sans systemd : lancer `syncengine-agent tick ETAT clock` depuis cron avec
   les variables dans la ligne de commande d'un script privé (`0700`).

SyncEngine ne génère ni `Environment=` ni `EnvironmentFile=` dans ses unités :
un secret n'y est jamais écrit. Limite : un fichier `environment.d` est lisible
par tout processus du compte ; préférer un trousseau quand le bureau en offre
un compatible.

## Par connecteur

- **SFTP** : adresse IP et port, nom d'hôte attendu, utilisateur, fichier
  `known_hosts` et racine distante absolue. La clé de l'hôte doit déjà figurer
  dans `known_hosts` : SyncEngine ne l'ajoute jamais. Dates conservées à la
  seconde.
- **WebDAV** : URL HTTPS de la collection (sans identifiants ni paramètres).
  HTTPS obligatoire, certificats vérifiés. Les collections (dossiers) sont
  adressées avec la barre finale. Seule redirection suivie, une fois : `301`
  ou `308` vers **exactement** la même URL suivie de `/` (Apache
  `DirectorySlash`) ; toute autre redirection est une erreur et le jeton n'est
  jamais renvoyé ailleurs. Exemple : `PROPFIND /dav/docs` → `301
  /dav/docs/` → requête réémise vers `/dav/docs/`. Les
  remplacements exigent un serveur qui fournit des ETag forts. Les dates sont
  gérées par le serveur : préférer la comparaison par contenu.
- **FTPS** : adresse IP, nom TLS du serveur, utilisateur, racine. Certificat
  vérifié ; une autorité privée peut être fournie en fichier PEM sans toucher
  au magasin système. Jamais de repli en FTP clair.
- **FTP** : identifiants et fichiers circulent en clair ; préférer FTPS ou SFTP.
- **SMB** : chemin UNC (`\\serveur\partage`) sous Windows ; sous Linux, un
  montage CIFS existant, en lecture seule. SyncEngine ne monte rien et ne
  demande aucun privilège.
- **OneDrive** : identifiant du drive, hôtes de téléchargement Microsoft
  approuvés (liste exacte, sans joker) et référence du jeton. Le jeton est
  obtenu et renouvelé en dehors de SyncEngine (pas de connexion OAuth
  intégrée).

Les adresses SFTP/FTP/FTPS sont des IP : l'interface ne résout pas les noms DNS.

## Comportement à connaître

- **Pas de suppression depuis une source distante** : un listing réseau ne
  prouve pas qu'un fichier est absent ; le miroir ne supprime donc rien sur la
  base d'une source distante.
- **Écritures en deux temps** : le fichier est déposé dans un dossier
  temporaire distant, vérifié, puis publié. Sur SFTP et FTP(S), un remplacement
  déplace d'abord l'ancien fichier dans ce dossier (`…/previous`) : une
  coupure peut laisser la cible momentanément absente, mais l'original reste
  récupérable.
- **FTP/FTPS** : un dossier `.syncengine-lock` à la racine empêche deux
  SyncEngine d'écrire en même temps. Il n'est jamais supprimé automatiquement
  après une coupure : le retirer à la main après vérification.
- Aucun autre programme ne doit écrire dans la destination pendant
  l'exécution ; il n'existe pas de transaction globale sur un serveur.
- Erreurs temporaires : quelques nouvelles tentatives courtes en lecture ;
  une écriture n'est jamais rejouée automatiquement.
- Les erreurs n'affichent jamais de mot de passe, jeton ou URL signée.
