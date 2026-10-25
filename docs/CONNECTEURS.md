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
d'authentification.

## Par connecteur

- **SFTP** : adresse IP et port, nom d'hôte attendu, utilisateur, fichier
  `known_hosts` et racine distante absolue. La clé de l'hôte doit déjà figurer
  dans `known_hosts` : SyncEngine ne l'ajoute jamais. Dates conservées à la
  seconde.
- **WebDAV** : URL HTTPS de la collection (sans identifiants ni paramètres).
  HTTPS obligatoire, certificats vérifiés, aucune redirection suivie. Les
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
