# Phase 08 — connecteur SFTP expérimental

Feature `sftp` désactivée par défaut. Crate indépendant, lié à la compilation,
implémentant `StorageConnector` en lecture seule par défaut.
`with_mutations()` active les mutations documentées ci-dessous. Pas de reprise
ou de rename public annoncé.
Pas de choix de stratégie, profil persistant, GUI ni cloud dans ce crate.

Exemple d'appel Rust (sur un thread bloquant ; les valeurs proviennent de
la configuration de l'appelant, aucun secret dans les options) :

```rust
use connector_sftp::{SftpConnector, SftpOptions};
use connector_api::StorageConnector;
use sync_core::{CancellationToken, ResourcePath};
# fn example() -> Result<(), Box<dyn std::error::Error>> {
let source = SftpConnector::new(SftpOptions {
    address: "192.0.2.1:22".parse()?,
    host: "backup.example".into(), username: "backup-reader".into(),
    known_hosts: "configuration/known_hosts".into(), root: "/backups".into(),
})?;
let cancel = CancellationToken::new();
let mut page = source.list(&ResourcePath::root(), None, &cancel)?;
while let Some(token) = page.next_page() {
    page = source.list(&ResourcePath::root(), Some(token), &cancel)?;
}
# Ok(()) }
```

Déclarer `connector-sftp = { path = "…/connector-sftp", features = ["sftp"] }`.
Ce fragment est un exemple d'intégration, pas une connexion effectuée en doctest.

`new` utilise l'agent SSH (Pageant selon libssh2 sous Windows ; la compatibilité
avec le service OpenSSH Windows n'est pas promise). `with_authentication` reçoit
un `Box<dyn SftpAuthentication>` externe : magasin système, clé ou authentification
interactive restent à la charge de l'application. Le port reçoit la session
**après** contrôle de la clé hôte via `known_hosts`, port compris. Aucun TOFU,
aucune modification du fichier de confiance. Renouveler les identifiants en
remplaçant le service ou l'instance ; aucune reconnexion implicite d'un flux.
Le compte source doit avoir uniquement listing/lecture ; pour une destination,
accorder les seules mutations requises, idéalement avec un chroot SFTP.

Chemins POSIX construits avec `/`, jamais avec `PathBuf::join` de Windows.
`lstat` contrôle les composants de la racine et les descendants ; liens finaux
rapportés par `stat`/listing, jamais ouverts ni parcourus. Ces contrôles séparés
ont une fenêtre TOCTOU : un compte distant concurrent hostile nécessite un
confinement côté serveur (chroot). SFTP v3 ne garantit pas un open-beneath/no-follow
atomique. Aucune garantie contre un serveur malveillant. SSH2 0.9.6 panique sous
Windows sur un nom non UTF-8 : un `catch_unwind` local transforme cette panique
de dépendance en `NotSupported`, abandonne le listing et la connexion. Le hook
de panique de Rust peut afficher le diagnostic UTF-8 ; aucun nom n'est recopié.
Les noms non représentables font échouer le listing, sans conversion avec perte.

Pages de 256 entrées sur un instantané trié de 10000 entrées maximum ; dépassement
refusé sans page complète. Une seule énumération active par instance, jetons
éphémères à usage unique liés au répertoire et à l'instance ; nouveau listing ⇒
ancien jeton périmé. Ce n'est pas une pagination serveur ni un snapshot atomique
du système distant. Casse/cohérence inconnues ; timestamps serveur UTC à la
seconde, sans garantie de justesse de son horloge. Pas de hash serveur.

Connexion : trois tentatives maximum pour erreurs transitoires, backoff 100/200 ms
annulable par pas de 20 ms. Timeout TCP/session SSH de 10 s par appel bloquant ;
les contrôles des composants multiples s'additionnent. L'agent d'authentification
externe doit lui-même borner ses appels. Pas de rejeu d'opération SFTP ou de flux
partiellement consommé. Annulation entre appels et blocs, pas d'interruption du
syscall. Flux tronqué (taille observée disponible) ⇒ `Disconnected`.
Les erreurs gardent opération/chemin et code numérique ; aucun texte serveur
arbitraire ou erreur d'authentification brute n'est exposé.

Tests : `cargo test -p connector-sftp --features sftp` ; contrat réseau explicite :
`python scripts/test_phase08_sftp.py` après `python -m pip install paramiko==5.1.0`.
Le script crée un serveur mémoire loopback et des clés jetables sous `target`,
exécute le test marqué ignoré par défaut et nettoie ses fichiers. Pas de compte
personnel. Compiler Windows nécessite MSVC/C et libssh2-sys ; Debian/Ubuntu :
`build-essential pkg-config libssl-dev zlib1g-dev`. Linux reste à valider en CI.

## Destinations et finalisation (clôture)

`with_mutations()` autorise explicitement mkdir, dépôt et delete non récursif.
Le compte doit être confiné et les écrivains externes exclus. Spool disque local
égal à la taille du fichier, mémoire bornée à 256 Kio, réservation distante unique.
`finish()` termine l'upload sans publication, permettant au moteur de relire le
staging avant commit. `sync-copy` finalise systématiquement et abandonne sur erreur.

CreateNew utilise un rename sans overwrite. Le remplacement SFTP v3 portable est
non atomique : ancien fichier déplacé sous `dépôt/previous`, nouveau publié ensuite,
jamais de suppression préalable de l'original. Une interruption peut laisser la
cible absente et l'original récupérable sous previous. Consulter dépôt/journal,
récupérer explicitement puis rescanner ; pas de reprise aveugle ni rollback global.
Un échec de cleanup est une erreur même si la cible existe déjà. Pas de fsync
physique serveur garanti. Les dates sont conservées à la seconde (SetModifiedTime).

Cohérence inconnue par défaut : aucune absence prouvée par le scanner. Utiliser
le registre et build_with_verified_strong_listing seulement après validation du
serveur. Tests isolés : création/remplacement, conflit, abandon, refus de publication
après déplacement de l'ancien fichier et récupération intégrale de l'original.
