# Provenance et correctif local

Source : crate crates.io `suppaftp 12.2.1`, dépôt
https://github.com/veeso/suppaftp ; hash Git source dans `.cargo_vcs_info.json`.
Licence upstream : MIT OR Apache-2.0, textes inclus dans ce répertoire.

Copie des sources, manifestes et exemples officiels, exclue des membres du
workspace (dépendance tierce). Aucun client FTP/TLS réécrit ici.

Correctif : dans `src/sync_ftp/control.rs`, remplacer les traces de commandes
et réponses brutes par `[redacted]` ; dans `src/sync_ftp.rs`, retirer le texte
serveur du diagnostic de bienvenue. Sans cela, TRACE journalisait notamment
`PASS mot-de-passe` et les éventuels secrets répétés par un serveur.
Les niveaux de log de l'application et des autres dépendances restent inchangés.
Les API async non utilisées par SyncEngine ne sont pas couvertes par ce correctif.

À retirer lors de l'adoption d'une version upstream offrant une protection
équivalente, après répétition du test de capture des logs FTP.

`src/sync_ftp/transfer_stream.rs` : masquer aussi l'erreur brute de finalisation
dans Drop, car elle peut contenir une réponse serveur répétant des identifiants.

SHA-256 archive crates.io originale : `2b58fcf921745cca6f0127c9f822069947da8c6dbeff52e9004750bcc448171f`.
