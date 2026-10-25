# Audit des lectures et contrats réseau

25 octobre 2026 — Windows x86_64, code de SyncEngine et sources FreeFileSync
fournies dans `F:\Project\referance\syncengine\FreeFileSync`. Aucun accès au NAS
réel pendant l’audit, aucun profil utilisateur modifié, aucune copie réelle
lancée. Les mutations de tests sont limitées aux stockages mémoire et aux
répertoires temporaires des serveurs de test sur `127.0.0.1`.

## Référence FreeFileSync effectivement examinée

Il s’agit d’une comparaison de mécanismes, pas d’une incorporation de code ni
d’un benchmark FreeFileSync. La version fournie de `zen/file_traverser.cpp`
utilise `opendir/readdir/lstat`, et `afs/native.cpp` les API POSIX. Elle ne
permet pas de déduire l’implémentation Windows de leurs listings SMB.

| Source dans la référence | Observation | Application dans SyncEngine |
| --- | --- | --- |
| `zen/file_traverser.cpp:29` | Distingue fin normale et erreur de parcours ; recommande de recommencer le parcours après erreur | Un listing transitoire SFTP/FTP est recommencé avec une session neuve avant publication de ses pages ; jamais un résultat partiel présenté comme complet |
| `FreeFileSync/Source/base/parallel_scan.cpp:386` | Conserve les lectures échouées et les décisions de retry ; un worker par périphérique, opérations parallèles limitées | Inventaire incomplet conservé, garde-fous miroir inchangés ; pas de parallélisme réseau illimité ajouté |
| `FreeFileSync/Source/afs/sftp.cpp:740` | Gestionnaire de sessions, réutilisation des sessions saines, abandon des sessions suspectes | Une session de listing SFTP par connecteur, remise en cache après succès seulement |
| `FreeFileSync/Source/afs/sftp.cpp:487` | Délais SSH et attentes non bloquantes, plusieurs canaux possibles | Délais existants de 10 s conservés ; pause entre lectures ; le multiplexage avancé reste une limite identifiée |
| `FreeFileSync/Source/afs/ftp.cpp:970` | Sessions FTP réutilisées, contrôle de santé et nettoyage | Une connexion de listing FTP/FTPS conservée par instance ; réancrage CWD absolu avant chaque listing ; abandon après erreur |
| `FreeFileSync/Source/afs/ftp.cpp:383` | Délais de connexion, faible débit et réponse serveur | Délais existants de socket/connexion conservés ; pas de relâchement TLS ni de retry de mutation |

## Diagnostic du ralentissement et du compteur

Le scan additionne les deux extrémités. La fin d’un côté local rapide peut
faire chuter le débit récent alors que le NAS continue. Cela ne suffit pas
à prouver une régression. Aucun débit NAS n’a été mesuré dans cette session.

Deux défauts vérifiables ont été corrigés :

1. Le scan SMB passait par le constructeur d’exécution, sans réutilisation
   des poignées des ancêtres. Une arborescence profonde imposait des ouvertures
   répétées. Le preview utilise désormais le cache local borné à 64 dossiers.
   Les vérifications de racine et les parcours d’exécution restent actifs.
2. Chaque dossier sur un provider de cohérence `Unknown` ajoutait une anomalie
   de type `UnreliableConsistency`. L’interface mélangeait ces avertissements
   avec les échecs de lecture. Les statistiques séparent désormais ces deux
   nombres ; les inventaires conservent **toutes** leurs anomalies et restent
   incomplets. La capture seule ne permet pas de classer les 4 610 anomalies.

La nouvelle interface précise « entrées analysées (2 côtés) », « erreurs de
lecture » et « dossiers à cohérence incertaine ». Un texte explique que
l’incertitude ne signifie pas une lecture échouée et ne permet pas de déduire
une suppression miroir. Les anciennes observations restent lisibles grâce à
la valeur par défaut du nouveau champ du snapshot.

## Revue par composant

| Composant | Constats et correctifs | Protections conservées / limites |
| --- | --- | --- |
| Scanner | Budget d’entrées/pages existant ; détection immédiate des jetons cycliques ajoutée ; erreurs et avertissements séparés | Root inaccessible => erreur ; sous-dossier échoué => couverture inconnue ; aucune absence déduite d’un échec |
| Local/SMB | Cache de dossiers réservé au preview SMB ; refus d’accès SMB distingué d’une déconnexion ; correctif UNC précédent conservé | Contrôle racine/identité et refus des liens conservés ; aucun timeout applicatif ne peut interrompre un appel SMB natif engagé |
| SFTP | Handshake/authentification autrefois répétés pour chaque dossier ; connexion de listing maintenant réutilisée ; erreur => abandon ; un seul restart transitoire | `known_hosts` strict, authentification externe, `lstat` des composants avant chaque accès ; flux fichiers et mutations utilisent toujours leurs connexions propres |
| FTP/FTPS | Même correction de sessions de listing ; réapplication de la racine absolue avant chaque lecture pour éviter un CWD renommé ; réponse de fin de transfert vérifiée avant réutilisation | FTP clair distinct de FTPS ; TLS/nom/CA vérifiés ; IP passive égale au serveur de contrôle ; MLSD, limites mémoire, verrou serveur et consentement exclusif inchangés |
| WebDAV | Client HTTP déjà réutilisé ; pause ajoutée entre blocs de réponse PROPFIND | HTTPS obligatoire, redirections/proxy implicite désactivés, analyse stricte du multistatus, href confinés, erreurs partielles refusées ; pas de DELETE récursif des collections |
| OneDrive | Les retries existants ne couvraient pas les erreurs de connexion GET ; maintenant trois tentatives maximales pour connect/timeout transitoires ; lecture JSON par blocs avec pause/annulation | Pagination avec contrôle des URLs, IDs et doublons ; jetons externes ; téléchargement autorisé seulement sur hôtes explicitement approuvés, sans Authorization transmis ; reste expérimental |
| Exécution/dépôts | Revue des préconditions, temporaires, publication et nettoyage dans les connecteurs | Les nouvelles reprises concernent seulement les listings et GET, jamais une écriture, publication, suppression ou réponse de mutation perdue |
| Observabilité | Nombres séparés, compatibilité des anciens snapshots testée | L’UI ne fournit pas encore de liste détaillée paginée des chemins en erreur ; ne pas diagnostiquer une panne précise avec le compteur seul |

Les sessions sont bornées à une par instance pour les listings, pas mises en
cache globalement entre profils/comptes. Les mutex concernés ne sont pas les
verrous de présentation. Un listing en erreur ne remet pas sa session en
cache ; les données accumulées avant erreur ne sont pas publiées comme page
complète. Les tentatives de connexion internes restent limitées à trois,
avec backoff annulable. Le restart de listing est limité à une fois.

## Validations réalisées

Windows uniquement :

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
$env:RUSTFLAGS = "-D warnings"
cargo check --workspace --all-targets --all-features --locked
```

Résultat workspace : **513 réussis, 0 échoué, 9 ignorés**. Les neuf contrats
ignorés nécessitent des serveurs dédiés et ont été exécutés séparément par :

```powershell
target/network-audit-env/Scripts/python.exe scripts/test_phase08_sftp.py
target/network-audit-env/Scripts/python.exe scripts/test_phase08_ftp.py
target/network-audit-env/Scripts/python.exe scripts/test_phase08_webdav.py
target/network-audit-env/Scripts/python.exe scripts/test_phase09_onedrive.py
```

Ces scripts passent : SFTP ; FTP et FTPS (contrat transport, réutilisation de
connexion et profil applicatif pour chacun) ; WebDAV HTTPS ; Graph HTTPS.
Le script FTP vérifie exactement **une authentification pour deux listings
complets de 601 entrées**, puis injecte une réponse 421 et vérifie une unique
reconnexion pour réussir ces lectures. Le test SFTP vérifie une authentification
pour plusieurs dossiers, puis une nouvelle authentification après une erreur,
et injecte un statut de connexion perdue pour vérifier la reprise du listing.
Les fixtures couvrent aussi clés hôte/CA inconnues, erreurs et accès refusés,
lectures tronquées, annulation, pagination, secrets absents des logs,
conflits de publication et préservation du contenu existant.

Les refus d’accès de la sandbox ont imposé la relance des tests avec fichiers
temporaires/ACL et serveurs localhost hors sandbox ; aucune protection de
production n’a été désactivée. Les messages de connexion réinitialisée des
serveurs HTTPS/SFTP correspondent notamment aux tests de refus et à leurs
connexions fermées ; les résultats des commandes Rust font foi.

Mesure reproductible, sans réseau :

```powershell
cargo run -p sync-orchestrator --example scan_handles_timing --release --locked
```

Arborescence temporaire : 16 branches × 12 niveaux × 8 fichiers, **1 744
entrées**. Une paire d’échauffement, puis trois passages par mode, ordre
alterné ; inventaires complets identiques après échauffement, aucun hash.
Mesure observée : **90,8 ms sans cache / 41,6 ms avec cache**, soit 2,18× sur
ce poste local. Ce n’est ni un benchmark NAS, ni une comparaison de vitesse
avec FreeFileSync. Un premier essai comparait les dates des dossiers dès leur
création et a échoué ; le benchmark garde une paire d’échauffement avant les
comparaisons et mesures. Aucun résultat différent n’est accepté pendant les
passages mesurés.

## Limites restantes et suite priorisée

- **P1, annulation SMB** : Windows gère les délais des appels natifs en cours.
  Une interruption forcée sans contrôle de l’état des E/S n’est pas implémentée.
  Il faut un travail spécifique d’adaptateur Windows et des tests SMB isolés.
- **P1, tailles de dossiers** : les listings SFTP et FTP/FTPS matérialisent
  le dossier avant leurs pages de 256 éléments et refusent plus de 10 000
  entrées ; DAV limite le XML à 8 Mio et 10 001 réponses. Ils échouent
  explicitement, sans dossier vide ni suppression. Une pagination réellement
  streamée est nécessaire pour les gros dossiers, avec tests mémoire dédiés.
- **P2, FTP stat** : la validation d’un fichier liste son parent pour établir
  correctement une absence, car FTP 550 ne prouve pas NotFound. Dans un gros
  dossier, les préconditions peuvent rester coûteuses. Une optimisation MLST
  doit préserver ce contrat et être testée sur les réponses ambiguës.
- **P2, empreintes distantes** : les connexions des flux fichiers SFTP/FTP ne
  sont pas mutualisées par cette correction ; les quatre workers de hash
  peuvent encore refaire des handshakes. Ne pas partager naïvement un flux
  actif ou la connexion de contrôle FTP d’un transfert.
- **P2, cohérence NAS** : conserver `Unknown` tant que le fournisseur n’a pas
  été caractérisé. Un « ignorer les avertissements » ne doit jamais activer
  les suppressions. Les timestamps et règles de casse SMB restent inconnus.
- **P2, diagnostics UI** : ajouter une consultation bornée/paginée des erreurs
  avec côté et chemin ; pas une entrée de log par fichier ni par octet.
- Linux et NAS réel non testés dans cette session ; aucune promesse de débit,
  de compatibilité universelle ou d’absence totale de panne réseau.

## Fichiers du changement

- `apps/desktop/src/profile.rs` : compteurs et explications.
- `crates/application/sync-orchestrator/src/{endpoints,lib}.rs` : constructeur
  de preview SMB, agrégation des compteurs, tests de compatibilité.
- `crates/application/sync-orchestrator/examples/scan_handles_timing.rs` : mesure.
- `crates/adapters/connector-registry/src/lib.rs` : fabrique SMB de scan.
- `crates/connectors/network/connector-smb/src/mounted.rs` : cache de scan et erreurs.
- `crates/connectors/network/connector-sftp/src/sftp.rs` et `tests/contract.rs` :
  sessions de listing, reprise bornée et vérification des authentifications.
  Son `README.md` décrit également la durée de vie de ces sessions.
- `crates/connectors/network/connector-ftp-transport/src/transport.rs` : sessions
  de listing FTP/FTPS, réancrage de racine et reprise bornée.
- `crates/connectors/network/connector-{ftp,ftps}/tests/wire.rs`,
  `tests/support/network_contract.rs`, `scripts/test_phase08_{ftp,sftp}.py` : contrats de
  réutilisation et décompte des connexions côté serveur.
- `crates/connectors/network/connector-webdav/src/webdav.rs` : pause du listing.
- `crates/connectors/cloud/connector-onedrive/src/transport.rs` et
  `src/graph/tests.rs` : GET transitoires, lecture JSON, retry borné testé.
- `crates/discovery/sync-scanner/src/{inventory,scan}.rs` et `tests/scan.rs` :
  statistiques et pagination cyclique, couverture incertaine conservée.
- `CHANGELOG.md`, `RELEASE_NOTES.md`, `docs/{ARCHITECTURE,CONNECTEURS,GUIDE,AUDIT_RESEAU}.md`.

La fabrique de preview, les statistiques de scan et le snapshot partagé sont
additifs et documentés pour revue d’intégration ; aucun format de profil,
plan d’exécution ou baseline n’a été changé. Aucune nouvelle dépendance Rust.
