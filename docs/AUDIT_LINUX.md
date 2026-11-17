# Audit Linux de SyncEngine 0.1.13

Audit fonctionnel de bout en bout de tous les modules sous Linux, réalisé
contre de **vrais serveurs isolés** (OpenSSH, proftpd, vsftpd, Apache
mod_dav, Samba) et des systèmes de fichiers réels (ext4, tmpfs, exFAT via
FUSE). Il recense les bogues propres à Linux, ceux qui touchent toutes les
plateformes, et les écarts avec le comportement attendu sous Windows.
Ce document sert de cahier de corrections : chaque point a un identifiant,
une reproduction, une cause localisée et une piste de correction.

Aucun code applicatif n'a été modifié pour cet audit.

## Environnement

| Élément | Valeur |
| --- | --- |
| OS | Ubuntu 24.04.5 LTS, noyau 6.18, x86_64, 4 vCPU, 15 Gio |
| Chaîne Rust | 1.99.1 (`rust-toolchain.toml`) |
| Commit audité | `c5bc0e0` (version 0.1.13) |
| SFTP | OpenSSH `sshd` (port 2222), authentification par agent SSH, `known_hosts` strict |
| FTP / FTPS | vsftpd 3.0.5 (2121 / 2990) puis proftpd 1.3.8 + mod_tls (2122 / 2991), CA privée |
| WebDAV | Apache 2.4 + mod_dav_fs en HTTPS (8443), jeton Bearer |
| SMB | Samba 4 (port 4450) ; **montage CIFS impossible** (module noyau absent du conteneur) |
| Systèmes de fichiers | ext4, tmpfs (plein / lecture seule), exFAT via `exfat-fuse` (image boucle), bind mount |
| Interface | Xvfb `:99` 1400×900, rendu logiciel Mesa, session D-Bus |

Les comptes, mots de passe, jetons et certificats sont des valeurs de test
jetables, propres à ce conteneur.

## Synthèse

| Domaine | État |
| --- | --- |
| Qualité (fmt, clippy, tests, check `-D warnings`) | ✅ tout passe |
| Scripts de contrat réseau (`test_phase08_*`, `test_phase09_onedrive`) | ✅ passent (serveurs Python de test) |
| Tests en utilisateur non-root (accès refusé réellement exécuté) | ✅ passent |
| Copie locale ext4 → ext4 (sauvegarde, miroir, conflits, versions) | ✅ correcte, sauf permissions (M2) |
| Garde-fous de suppression (source illisible, absente, vide) | ✅ zéro suppression |
| Bidirectionnel local (propagation, suppression, conflits, baseline) | ✅ correct |
| Interruption SIGINT / SIGTERM | ✅ annulation propre ; SIGKILL : voir M6 |
| Verrou d'exécution concurrente | ✅ la seconde exécution est refusée |
| Disque plein | ✅ échec propre, temporaires retirés |
| Performance du scan (200 000 fichiers) | ✅ 1,3 s, 66 Mo (build debug) |
| **Destination distante (SFTP, FTP, FTPS, WebDAV)** | ❌ **aucune copie n'est jamais faite** (C1) |
| **Destination exFAT / FAT (clé USB)** | ❌ **toutes les écritures échouent** (C2) |
| **WebDAV sur Apache** | ❌ **sous-dossiers illisibles et non créables** (C3) |
| Source distante SFTP / FTP / FTPS | ⚠️ fonctionne, avec réserves (m2, m4) |
| FTP / FTPS sur vsftpd | ❌ refusé (MLSD exigé, M3) |
| Planification Linux depuis l'interface | ❌ ne s'exécute jamais, l'interface affiche « Agent actif » (M1) |
| SMB sous Linux | ⚠️ non testable ici (CIFS absent) ; revue de code (m7) |
| Archive portable Linux (`release.py`), installation, mise à jour intégrée 0.1.13 → 0.1.14 depuis GitHub | ✅ empreintes OK, binaires remplacés, interface relancée, aucun résidu |

Gravité : **C** critique (fonction principale inutilisable ou perte de
données possible), **M** majeur, **m** moyen, **l** mineur.

## Constats critiques

### C1 — Une sauvegarde vers une destination distante ne copie rien (toutes plateformes)

- **Observé** : profils `backup` local → SFTP (OpenSSH), FTP et FTPS
  (proftpd), WebDAV (Apache). La simulation et l'exécution donnent
  `skip / uncertain` pour **chaque** fichier ; 0 copie. Destination vide ou
  non vide, même résultat. Dans l'interface : « Ignorer 12 », bouton
  « Rien à appliquer ».
- **Cause** : `crates/discovery/sync-scanner/src/scan.rs:414` marque chaque
  dossier listé `Incomplete(UnreliableConsistency)` si le connecteur
  n'annonce pas `ListingConsistency::Strong`. Seuls `local` et `smb`
  l'annoncent ; SFTP, FTP, FTPS, WebDAV restent `Unknown`. Ensuite
  `Inventory::presence` (`sync-scanner/src/inventory.rs:330`) rend
  `Presence::Unknown` pour tout chemin absent, `sync-compare` produit
  `PresenceUnknown` et `sync-decisions` en fait un `skip`.
- **Pourquoi non détecté** : les tests de la façade
  (`crates/application/sync-orchestrator/tests/wire_profiles.rs`) ne couvrent
  que `remote_source`, jamais `remote_destination` ; les scripts
  `test_phase08_*` testent le connecteur seul.
- **Reproduction** : profil JSON avec `"remote_destination": {"provider":"sftp",…}`,
  `syncengine profile preview ETAT ID` → `plan.destination.complete=false`,
  toutes les opérations `skip/uncertain`.
- **Piste** (contrat partagé : **arbitrage requis**, cf. AGENTS.md) :
  une incertitude de *cohérence* (pas une erreur de lecture) doit continuer
  à interdire les **suppressions**, mais peut autoriser une **copie** vers un
  chemin absent avec précondition `destination_absent` : l'exécuteur revérifie
  par `stat` et la publication `CreateNew` refuse une cible existante. Deux
  options : (a) nouvelle cause `Presence::AbsentUnverified` traitée comme
  absence pour les copies et comme inconnue pour les suppressions ; (b) les
  connecteurs SFTP/FTP sur serveur POSIX déclarent `Strong` (listing avec fin
  explicite, comme SMB). Ajouter un test de façade `remote_destination` pour
  chaque protocole.
- **Critère d'acceptation** : *Étant donné* une destination SFTP vide et une
  source de 3 fichiers, *quand* on simule une sauvegarde, *alors* 3 copies sont
  prévues, 0 suppression, et l'exécution publie les 3 fichiers vérifiés.

### C2 — Écriture impossible vers exFAT / FAT32 (clés USB) et FS sans liens physiques (Linux)

- **Observé** : `syncengine execute SRC /media/usb/bk` (exFAT monté par
  `exfat-fuse`) : les dossiers sont créés, **chaque fichier** échoue avec
  `[local] write : accès refusé`. Statut ÉCHEC.
- **Cause** (strace) : la publication `CreateNew` du connecteur local fait
  `linkat(".syncengine-….partial", "a.txt")` pour garantir la non-écrasement,
  puis `unlink` du temporaire. exFAT, FAT32 et de nombreux montages FUSE/CIFS
  n'ont pas de liens physiques → `EPERM`, traduit en « accès refusé ».
- **Impact** : la sauvegarde sur clé USB ou disque externe formaté
  exFAT/FAT (cas d'usage majeur) échoue à 100 % sous Linux. Windows n'est pas
  concerné (autre chemin de publication).
- **Piste** : publier par `renameat2(…, RENAME_NOREPLACE)` (atomique, sans
  lien physique, géré par ext4, xfs, btrfs, exFAT et FAT noyau, FUSE ≥ 7.23) ;
  en repli sur `EINVAL`/`ENOSYS`/`EPERM`/`EOPNOTSUPP` : `linkat`, puis en
  dernier recours `stat` absent + `rename` sous le verrou de destination, avec
  la limite documentée (fenêtre de course). Classer `EPERM` à part de
  `EACCES`. Ajouter un test d'intégration sur une image exFAT/FAT montée par
  FUSE quand l'environnement CI le permet.
- **Critère** : *Étant donné* une destination exFAT vide, *quand* on sauvegarde
  10 fichiers, *alors* les 10 sont publiés et vérifiés, aucun `.partial` ne reste.

### C3 — WebDAV sur Apache : aucun sous-dossier lu ni créé (toutes plateformes)

- **Observé** : source WebDAV (Apache mod_dav) : seuls les fichiers de la
  racine sont vus ; `docs/`, `sub/`, `vide/` échouent en
  `stat : échec définitif`. Destination WebDAV (sonde directe du connecteur) :
  `MKCOL /root/dir1` répond 201, puis toute opération sur `dir1` échoue ;
  le dossier reste sur le serveur et ne peut être supprimé (`NotSupported`).
- **Cause** : le connecteur envoie `PROPFIND /root/docs` **sans barre finale** ;
  Apache (`mod_dir`, `DirectorySlash`) répond `301 → /root/docs/`, que le
  connecteur ne suit pas (choix de sécurité : aucune redirection) et classe
  `Permanent`. Journal Apache : `PROPFIND /root/docs 301`.
- **Pourquoi non détecté** : le serveur WebDAV de `test_phase08_webdav.py` ne
  redirige pas.
- **Piste** : adresser toute collection connue (type `collection` du listing
  parent, ou après `MKCOL`) avec une barre finale ; accepter un 301/308 dont la
  cible est **exactement** la même URL + `/` (même origine), sans jamais suivre
  d'autre redirection ni réémettre `Authorization` ailleurs. Ajouter au serveur
  de test un mode « redirection des collections » et un test sur Apache si
  possible.

## Constats majeurs

### M1 — Planifications Linux créées dans l'interface : jamais exécutées, « Agent actif » affiché

- **Observé** : un horaire « toutes les 1 min » enregistré depuis l'interface
  apparaît dans le calendrier ; la barre latérale affiche
  « Agent actif — Prochain job à 08:11 UTC » ; aucune exécution n'a lieu (aucun
  timer systemd n'est installé). La page Planification titre « Exécutée par
  l'agent, même fenêtre fermée » ; seule une note grise renvoie au guide.
- **Cause** : `apps/desktop/src/native.rs:161` `prepare_agent` ne fait rien
  hors Windows ; `apps/desktop/src/shell.rs:387` calcule `agent_on` comme
  « aucun arrêt demandé », sans vérifier que le superviseur natif existe.
- **Contraire à** : « La GUI affiche des capacités réelles, non des
  placeholders présentés comme opérationnels ».
- **Piste** : sous Linux, détecter l'état réel
  (`systemctl --user is-enabled/is-active syncengine-clock.timer`, ou présence
  des fichiers d'unité dans `$XDG_CONFIG_HOME/systemd/user`) ; afficher
  « Agent non installé » sinon ; proposer un bouton qui écrit les unités
  (`platform-linux`), `daemon-reload`, `enable --now`, et les ajoute à
  `TRACES_SYSTEME.md` / `purge.rs` (déjà inventoriées). Prévoir le repli
  documenté sans systemd (cron).

### M2 — Permissions POSIX non conservées (fuite de confidentialité)

- **Observé** : `docs/b.md` en `600` → `644` à la destination ; script
  `755` → `644` ; dossier `700` → `755`. Les fichiers privés deviennent
  lisibles par tous dans la sauvegarde.
- **Cause** : aucune gestion des modes dans `connector-local` (le temporaire
  est créé en `0666 & ~umask`, aucun `fchmod`) ; seul `app-update` utilise
  `PermissionsExt`. Non documenté.
- **Contraire à** : « Traiter permissions POSIX et liens symboliques
  explicitement ».
- **Piste** : politique explicite et documentée ; par défaut en local→local
  Unix, appliquer le mode source (bits `0o7777` hors setuid/setgid si refusés)
  au temporaire **avant** publication et aux dossiers créés ; en SFTP, `setstat`
  du mode ; signaler `NotSupported` ailleurs. Ne jamais élargir les droits
  d'un fichier existant remplacé.
- **Critère (sécurité)** : *Étant donné* un fichier source `0600`, *quand* il
  est copié vers une destination locale, *alors* le fichier publié n'est jamais
  lisible par le groupe ou les autres, y compris pendant le dépôt temporaire.

### M3 — FTP/FTPS refusés par vsftpd (MLSD obligatoire, non documenté)

- **Observé** : contre vsftpd 3.0.5 (serveur par défaut de Debian, Ubuntu,
  RHEL et de nombreux NAS) : `racine inaccessible : list : non pris en charge ()`.
  `FEAT` de vsftpd n'annonce ni `MLSD` ni `MLST`.
- **Cause** : `connector-ftp-transport/src/transport.rs:330` n'utilise que
  `MLSD`/`MLST`.
- **Piste** : message explicite (« le serveur ne prend pas en charge MLSD »)
  et vérification de `FEAT` à la connexion ; documenter l'exigence dans
  `docs/CONNECTEURS.md` ; éventuellement repli `LIST` (format `ls -l`) traité
  de façon conservatrice (dates imprécises → incertitude).

### M4 — Secrets et agent SSH absents des exécutions planifiées et de l'interface lancée depuis le menu

- **Observé** : l'interface lancée sans `SYNCENGINE_SECRET_DAVTOKEN` affiche
  « identifiant runtime indisponible ». Les unités générées
  (`platform-linux/src/lib.rs`) n'ont ni `Environment`, ni `EnvironmentFile`,
  ni `SSH_AUTH_SOCK` : un profil SFTP, FTP, FTPS, WebDAV ou OneDrive planifié
  sous Linux échoue toujours à l'authentification. Un lanceur `.desktop`
  n'hérite pas non plus des variables d'un terminal.
- **Piste** : documenter précisément les voies Linux
  (`~/.config/environment.d/*.conf` en `0600`, `systemctl --user
  import-environment SSH_AUTH_SOCK`, `EnvironmentFile=` optionnel non généré
  par défaut) ; signaler dans l'interface, avant simulation, la référence
  manquante et la variable attendue ; ne jamais écrire le secret dans l'unité.

### M5 — Purge Linux : ordre des étapes systemd inversé

- **Revue de code** (`apps/desktop/src/purge.rs:679` puis `:684`) : les
  fichiers d'unité sont supprimés **avant** `systemctl --user disable --now`.
  `disable` échoue alors (« unit file does not exist ») et ne stoppe rien ;
  le timer déjà chargé peut continuer de lancer `syncengine-agent tick` chaque
  minute, qui peut recréer le dossier d'état supprimé.
- **Non exécuté** : pas de gestionnaire systemd utilisateur dans le conteneur.
- **Piste** : `systemctl --user disable --now <unités>` d'abord, puis retrait
  des fichiers et liens, puis `daemon-reload` ; tester avec un faux
  `systemctl` placé dans le `PATH` qui journalise l'ordre des appels.

### M6 — Fichiers `.partial` orphelins après arrêt brutal, jamais nettoyés

- **Observé** : `SIGKILL` pendant une copie parallèle de 41 fichiers : 4
  `.syncengine-<pid>-…-N.partial` restent ; le journal n'en référence qu'un
  (3 créés après la dernière entrée journalisée). Le scanner masque ces noms :
  ni la reprise (`resume_interrupted`, qui ne retire que les dépôts
  consignés) ni un miroir ne les supprimeront jamais. Ici jusqu'à 4 × la
  taille des gros fichiers.
- **Cause** : l'entrée de journal du dépôt est écrite après sa création
  (`sync-executor/src/run.rs:725`).
- **Piste** : journaliser l'intention (nom prédéterminé) avant création, ou,
  à la reprise d'une exécution interrompue, retirer les seuls
  `.syncengine-<pid de l'exécution>-*.partial` des dossiers touchés (sans
  suivre de lien) ; signaler en simulation les `.partial` présents.

### M7 — Détection « destination dans la source » contournée par un bind mount

- **Observé** : `mount --bind SRC VUE` puis `syncengine preview SRC VUE/inner`
  → plan de copie de `SRC` dans son propre sous-dossier ; `preview SRC VUE`
  (même dossier) accepté. Un miroir avec suppressions sur une vue imbriquée
  pourrait effacer une partie de la source.
- **Cause** : comparaison de chemins canonisés seulement
  (`apps/cli/src/preview.rs:142`, `sync-orchestrator/src/lib.rs:617`).
- **Windows** : même faiblesse probable avec `subst`, un lecteur mappé ou
  `\\localhost\c$\…` (à vérifier sur Windows).
- **Piste** : comparer aussi l'identité des dossiers (`st_dev`/`st_ino` sous
  Unix, numéro de série de volume + index de fichier sous Windows) en
  remontant les ancêtres de chaque racine.
- **Critère (sécurité)** : *Étant donné* une destination qui est une vue bind
  d'un sous-dossier de la source, *quand* on simule un miroir, *alors* aucun
  plan n'est produit (erreur « imbriqué »).

### M8 — Spool d'envoi distant complet dans `/tmp` (tmpfs en RAM)

- **Revue de code** : `connector-staging/src/lib.rs:66` dépose tout le
  fichier dans `std::env::temp_dir()` avant l'envoi. Sur Debian 13, Fedora,
  Arch, `/tmp` est un tmpfs : un envoi de plusieurs Gio consomme la RAM ou
  échoue en `ENOSPC`. (Masqué aujourd'hui par C1.)
- **Piste** : dossier de spool configurable, par défaut sous
  `$XDG_CACHE_HOME/SyncEngine` (et l'inscrire dans `TRACES_SYSTEME.md` et la
  purge) ; vérifier l'espace libre avant ; documenter.

### M9 — Précision des dates fixée par OS, pas par système de fichiers

- **Observé** : fichiers identiques copiés par `cp -p` sur exFAT (dates
  tronquées à la seconde) : la simulation prévoit `REMPLACER … source plus
  récente` pour tous, à chaque passage (mode date, défaut de la CLI et des
  profils « Taille et date »).
- **Cause** : `connector-local/src/connector.rs:89` déclare `Nanosecond` sous
  Linux (`HundredNanoseconds` sous Windows) quel que soit le volume. Même défaut
  sous Windows avec une clé FAT32 (2 s).
- **Piste** : détecter le type de volume (`statfs` `f_type` : msdos, exfat,
  fuseblk, cifs… → `TwoSeconds`/`Unknown` ; `GetVolumeInformationW` sous
  Windows) ou mesurer l'aller-retour d'une date après écriture.

### M10 — Correctif `libssh2-sys` vendored devenu inutilisé (risque Windows)

- **Observé** : `cargo update` affiche « patch `libssh2-sys v0.3.3
  (vendor/libssh2-sys)` was not used in the crate graph ». Le commit `c5bc0e0`
  (`cargo update`) a remplacé la copie corrigée 0.3.3 par `libssh2-sys 0.3.4`
  de crates.io ; `Cargo.lock` porte un `[[patch.unused]]`.
- **Analyse** : la 0.3.4 semble embarquer le correctif amont libssh2#2583
  (`wcng_bn_normalize` dans `libssh2/src/wincng.c`), ce qui remplirait la
  condition de retrait de `vendor/libssh2-sys/SYNCENGINE-PATCH.md`. Mais rien
  ne l'a validé sous Windows, et le dépôt garde un vendoring mort, une entrée
  `[patch.crates-io]` inopérante et une documentation fausse (`Cargo.toml`,
  `SYNCENGINE-PATCH.md`, notices).
- **Piste** : vérifier sous Windows (`scripts/test_phase08_sftp.py` répété
  ≥ 50 fois, critère de `SYNCENGINE-PATCH.md`) ; si OK, retirer
  `vendor/libssh2-sys`, l'entrée `[patch]`, l'exclusion du workspace, et
  mettre à jour notices et documentation ; sinon, réaligner le patch sur 0.3.4
  ou épingler `=0.3.3`. Ajouter une vérification CI qui échoue sur un
  `[[patch.unused]]`.

## Constats moyens

| ID | Constat | Où | Piste |
| --- | --- | --- | --- |
| m1 | Un nom Linux légal contenant `\`, ou non UTF-8, marque **tout le dossier parent** incomplet : le fichier n'est jamais sauvegardé et les suppressions miroir de ce dossier sont bloquées à vie | `sync-scanner` / `ResourcePath` | Issue par entrée sans invalider les autres absences ; accepter `\` quand source et destination sont POSIX |
| m2 | Depuis une source SFTP/FTP/WebDAV, `Readme.TXT` et `readme.txt` sont toujours ignorés (`case_collision`) même vers une destination Linux sensible à la casse | capacités `case_sensitivity: Unknown` | Déclarer la sensibilité réelle (SFTP POSIX) ou la rendre configurable ; décider selon la destination |
| m3 | SFTP/OpenSSH : `CreateNew` sur une cible existante → `Permanent` « original conservé sous le dépôt/previous » (faux : aucun `previous` en `CreateNew`) au lieu de `Conflict` | `connector-sftp/src/mutations.rs:156` | Après échec du rename, `lstat` de la cible → `Conflict` ; message selon le mode |
| m4 | FTP/FTPS : nom avec espace final publié mais signalé en conflit ; suppression impossible ; dossier parent non supprimable (`PermissionDenied` au lieu de « non vide ») | `connector-ftp-transport` | Refuser ces noms au plan pour FTP ou vérifier par listing du parent ; classer 550 non vide |
| m5 | SFTP : toute racine contenant un lien symbolique est refusée (ex. `/var/services/homes` Synology, `/home` → `/usr/home`) | `connector-sftp/src/sftp.rs:249` | `realpath` de la racine une fois à la connexion, liens refusés seulement en dessous |
| m6 | SFTP et FTP : plafond de 10 000 entrées par dossier (8 Mio de listing FTP) ; un seul nom non UTF-8 fait échouer tout le listing | `sftp.rs:443`, `transport.rs:346-371` | Pagination en flux ; issue par entrée |
| m7 | SMB Linux : seuls `cifs`/`smb3` noyau acceptés ; les montages GVfs (`fuse.gvfsd-fuse`, `/run/user/UID/gvfs`) et KIO-FUSE, voie habituelle d'un bureau, sont refusés avec « connexion perdue » ; une **destination** SMB est acceptée à l'enregistrement puis échoue au lancement | `connector-smb/src/mounted.rs:153`, `sync-profiles` | Refus explicite à l'enregistrement sous Linux ; messages dédiés ; documenter GVfs (ou l'accepter en lecture seule prudente) |
| m8 | `profile preview` (JSON) n'expose pas les causes d'un inventaire incomplet : impossible de comprendre C1 depuis la CLI | `apps/cli/src/profiles.rs` | Ajouter la liste des issues (type, chemin) |
| m9 | Codes de sortie : `profile run` rend 1 pour « réussie avec réserves », `execute` rend 3 ; la table de `docs/CLI.md` dit 1 = différences | `apps/cli/src/profiles.rs:52` | Aligner sur 3 et documenter |
| m10 | Interface sans `DISPLAY`/`WAYLAND_DISPLAY`, ou sans `libxkbcommon-x11.so` (chargée dynamiquement, invisible pour `ldd`) : **panic** avec backtrace | `apps/desktop/src/main.rs:76` | Message clair et code de sortie ; documenter les dépendances d'exécution réelles |
| m11 | Liens symboliques et fichiers spéciaux (très courants : `node_modules/.bin`, `~/.local`) ⇒ chaque exécution planifiée finit en « attention » (`report_requires_attention`) indéfiniment | `sync-orchestrator/src/scheduled.rs:171` | Distinguer « ignoré par politique » (information) d'un avertissement |
| m12 | La simulation n'anticipe pas une destination insensible à la casse ni les noms interdits (exFAT : `Dir`/`dir`, `q?.txt`, `a:b`) ; refus seulement à l'exécution | `connector-local` | Détecter la sensibilité du volume (exFAT/FAT/NTFS, ext4 casefold) et signaler en simulation |

## Constats mineurs

| ID | Constat | Où |
| --- | --- | --- |
| l1 | Caractères corrompus : « non qualifi? » (message utilisateur), « ?crivains » (doc) | `sync-profiles/src/lib.rs:156`, `connector-ftp/src/lib.rs:32`, `connector-ftps/src/lib.rs:34` |
| l2 | Parenthèses vides dans les messages : « connexion perdue ou extrémité injoignable () », « non pris en charge () » | rendu des `StorageError` sans chemin |
| l3 | Message anglais : « another SyncEngine execution owns the destinations » | CLI `execute` |
| l4 | `EROFS` rendu « échec définitif », `EPERM` rendu « accès refusé » | classement des erreurs locales |
| l5 | Aucune alerte d'espace libre en simulation (8 Mo prévus vers 5 Mo libres) | façade / CLI |
| l6 | Code brut `(report_requires_attention)` et libellé tronqué dans la page Planification | `apps/desktop/src/schedules.rs` |
| l7 | `syncengine-clock.service` a une section `[Install] WantedBy=default.target` inutile (l'activer lancerait un passage à chaque connexion) | `platform-linux/src/lib.rs` |
| l8 | Horaires affichés uniquement en UTC ; un passage par minute ajoute deux lignes au journal systemd utilisateur par minute | interface, unités |
| l9 | L'archive s'extrait dans `SyncEngine_0.1.13_x86_64-unknown-linux-gnu/` ; après mise à jour intégrée, le dossier garde l'ancien numéro | `scripts/release.py` |
| l10 | L'archive Linux ne fournit ni lanceur `.desktop` ni icône : aucune entrée de menu, installation non vérifiable par l'utilisateur | packaging |

## Écarts Windows / Linux relevés

| Sujet | Windows | Linux |
| --- | --- | --- |
| Publication sans écrasement | chemin propre Windows | `linkat` → échec sur exFAT/FAT (C2) |
| Agent planifié | tâche résidente enregistrée par l'interface | rien d'installé par l'interface, état affiché faux (M1) |
| Secrets planifiés | variables utilisateur persistantes faciles | aucune voie documentée pour systemd / lanceur `.desktop` (M4) |
| Permissions | ACL héritées du dossier | modes POSIX perdus (M2) |
| SMB | UNC, lecture et écriture | CIFS noyau seul, lecture seule, GVfs refusé (m7) |
| Spool distant | `%TEMP%` sur disque | `/tmp` souvent en RAM (M8) |
| Noms | `\` séparateur | `\` légal mais refusé (m1) |

## Non testé ou bloqué ici

- Montage CIFS réel (module noyau absent) : SMB Linux vérifié par lecture du code seulement.
- ext4 *casefold* et vfat noyau (non pris en charge par ce noyau) ; exFAT
  uniquement via FUSE (le pilote noyau a une précision de 10 ms).
- Gestionnaire systemd utilisateur : unités validées par `systemd-analyze
  verify`, non exécutées ; M5 par revue de code.
- Session Wayland, icône de notification (`ksni`), thème système.
- OneDrive réel (serveur simulé uniquement).
- Toute vérification Windows (aucun poste Windows disponible).

## TODO priorisée (cahier de corrections)

Chaque correction suit AGENTS.md : test qui échoue d'abord, correction,
documentation `docs/` concernée, ligne `RELEASE_NOTES.md` pour un changement
visible, `CHANGELOG.md` pour un changement de comportement.

1. [x] **C1** Copies vers destinations distantes (option A arbitrée : copies seules ; façade FTP/FTPS et WebDAV testées, SFTP : voir compte rendu) (arbitrage du contrat
   scanner/compare d'abord) + tests de façade `remote_destination` par protocole.
2. [x] **C2** Publication locale sans lien physique (`renameat2 RENAME_NOREPLACE`
   + replis) + test exFAT/FAT FUSE.
3. [x] **C3** WebDAV : barre finale des collections, redirection « même URL + / »
   seulement + test serveur redirigeant.
4. [x] **M2** Politique de permissions POSIX (tests `0600`, `0755`, dossiers `0700`).
5. [x] **M7** Imbrication par identité de dossier (bind mount ; vérifier `subst` sous Windows).
6. [x] **M1** État réel de l'agent Linux et installation des unités depuis l'interface.
7. [x] **M5** Ordre `disable --now` → fichiers → `daemon-reload` + test à faux `systemctl`.
8. [x] **M6** `.partial` orphelins : journal d'intention ou nettoyage par pid.
9. [x] **M4** Secrets/agent SSH pour systemd et lanceur ; message avant simulation.
10. [x] **M3** FTP sans MLSD : message, `FEAT`, documentation (repli `LIST` optionnel).
11. [x] **M8** Spool distant configurable hors `/tmp`, contrôle d'espace.
12. [x] **M9** (Linux ; Windows à faire) Précision des dates selon le volume (Linux et Windows).
13. [ ] **M10** Statuer sur `libssh2-sys` (validation Windows puis retrait du vendoring, ou réalignement) + contrôle CI `[[patch.unused]]`.
14. [ ] **m1–m12** dans l'ordre du tableau.
15. [ ] **l1–l10**.
16. [ ] Documentation : dépendances d'exécution Linux réelles
    (`libxkbcommon-x11-0`, `libxcursor1`, `libxi6`, `libxrandr2`,
    `libvulkan1` ou Mesa, `libwayland-client0`), exigence MLSD, permissions,
    planification Linux.

## Reproduire l'audit

Serveurs (root, conteneur jetable) :

```sh
apt-get install -y openssh-server proftpd-core proftpd-mod-crypto apache2 samba exfat-fuse exfatprogs xvfb imagemagick xdotool libxkbcommon-x11-0
```

- SFTP : `sshd -f sshd_config` (`Port 2222`, `Subsystem sftp internal-sftp`),
  clé dans un `ssh-agent`, `ssh-keyscan -p 2222 127.0.0.1 > known_hosts`.
- FTP/FTPS : proftpd `PassivePorts`, `TLSEngine on`, `TLSRequired on`, CA
  privée passée en `root_certificate`.
- WebDAV : Apache `DAV On` sur `/root`, `<If "%{HTTP:Authorization} != 'Bearer …'"> Require all denied`.
- Secrets : `SYNCENGINE_SECRET_<REF>` ; `SSL_CERT_FILE` vers un paquet incluant la CA de test.

Profils (format de `sync_profiles::Profile`) :

```json
{"id":"sftp1","name":"sftp1","group":"","mode":"backup","source":"/abs/src","destination":"/unused",
 "remote_destination":{"provider":"sftp","address":"127.0.0.1:2222","host":"127.0.0.1",
 "username":"synctest","known_hosts":"/abs/known_hosts","root":"/home/synctest/sftproot"},
 "filter":{},"allow_delete":false,"trash":{"enabled":false,"max_bytes":1073741824}}
```

```sh
syncengine profile save ETAT profil.json
syncengine profile preview ETAT sftp1
syncengine profile run ETAT sftp1 --confirm
```

Les écritures des connecteurs ont été sondées par un petit programme hors
dépôt appelant `open_write` / `commit(CreateNew|ReplaceExisting)` /
`create_directory` / `delete` / `open_read` sur chaque connecteur avec
`with_mutations()`.

## Commandes de contrôle exécutées

| Commande | Résultat |
| --- | --- |
| `cargo fmt --all -- --check` | OK |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | OK |
| `cargo test --workspace --all-features` (root) | OK |
| binaires de test rejoués en utilisateur non-root | OK (hors faux échecs d'environnement : binaires d'application sans argument, test d'architecture appelant `cargo` absent du `PATH`) |
| `RUSTFLAGS="-D warnings" cargo check --workspace --all-targets --all-features` | OK |
| `scripts/test_phase08_{ftp,sftp,webdav}.py`, `test_phase09_onedrive.py` | OK |
| `cargo update` | aucun paquet modifié ; avertissement `patch libssh2-sys … not used` (M10) |
| `cargo audit` et `cargo audit --deny warnings` (exceptions de `ci.yml`) | aucun avis |
| `python scripts/release.py prepare / portable / collect` (x86_64-unknown-linux-gnu) | OK, `SHA256SUMS` vérifié |
| `scripts/test_{versioning,release,state_maintenance}.py`, `test_signals.py --binary target/debug/syncengine`, `test_state_lock.py --binary target/debug/syncengine` | OK |
