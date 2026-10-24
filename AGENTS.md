# AGENTS.md — règles permanentes des agents de développement

Ce fichier s’applique à la racine de SyncEngine et à tous les sous-dossiers.
Priorité : sécurité des données, interfaces stables, modularité, honnêteté des tests.

## MISSION

- Développer SyncEngine comme un produit Rust modulaire et multiplateforme.
- Ne travailler que sur la phase demandée.
- Lire CONTEXTE.md avant toute modification.
- Lire docs/CONTRATS.md et docs/ETAT_PROJET.md.
- Respecter l’option A : crates liés à la compilation, pas de plugins dynamiques.
- Le comportement de sauvegarde doit privilégier l’intégrité des données.
- Ne jamais annoncer une fonctionnalité non implémentée.

## PÉRIMÈTRE

- Identifier la phase ciblée et ses critères d’acceptation.
- Lister les fichiers/crates que la phase est autorisée à modifier.
- Limiter les changements aux fichiers nécessaires.
- Ne pas faire de refactoring global non demandé.
- Ne pas anticiper une phase ultérieure par du code factice.
- Documenter les dépendances et les étapes bloquées.
- Demander arbitrage si deux contrats sont incompatibles.

## CARGO WORKSPACE

- Un crate par responsabilité métier clairement définie.
- Éviter les dépendances circulaires.
- Utiliser workspace.dependencies lorsque pertinent.
- Employer une édition stable de Rust et la documenter.
- Figer la chaîne Rust via rust-toolchain.toml après décision.
- Garder Cargo.lock versionné pour les applications.
- Utiliser feature flags pour les connecteurs facultatifs.
- Éviter les crates vides créés uniquement pour remplir une arborescence.
- Chaque crate doit avoir une description de sa responsabilité.
- Chaque crate doit exposer une API publique minimale.

## FRONTIÈRES

- sync-core ne dépend pas de l’interface graphique.
- Aucun moteur métier ne dépend de Tauri.
- Aucun connecteur ne choisit les règles de synchronisation.
- Aucun scanner ne supprime ou ne modifie de fichiers.
- La comparaison fournit des faits, pas des décisions.
- Les stratégies produisent des plans, pas des mutations.
- La copie ne décide pas des suppressions.
- L’exécuteur valide le plan et applique les mutations.
- Le scheduler déclenche un profil via l’API applicative.
- Les logs et événements ne sont pas des sources de vérité.
- Isoler la plateforme Windows dans son adaptateur.
- Isoler la plateforme Linux dans son adaptateur.
- Ne pas importer les implémentations concrètes dans sync-core.

## QUALITÉ RUST

- Interdire les warnings dans les vérifications CI.
- Exécuter cargo fmt --all -- --check.
- Exécuter cargo clippy --workspace --all-targets --all-features -- -D warnings.
- Exécuter cargo test --workspace --all-features.
- Exécuter cargo check --workspace --all-targets --all-features avec RUSTFLAGS="-D warnings".
- Ne pas masquer les warnings avec allow global.
- Toute exception de lint doit être locale, motivée et documentée.
- Ne pas introduire de unsafe sans justification et revue.
- Préférer des types explicites aux chaînes magiques.
- Utiliser Result pour les erreurs récupérables.
- Ne pas utiliser unwrap/expect dans les chemins de production critiques.
- Ne jamais ignorer silencieusement un Result.
- Éviter panic! dans le flux d’exécution normal.
- Définir des erreurs publiques structurées et contextualisées.
- Utiliser des newtypes pour éviter les mélanges d’identifiants.
- Conserver les invariants dans les constructeurs.
- Éviter les copies en mémoire complètes pour fichiers volumineux.
- Expliquer les choix de synchronisation/async et leurs limites.
- Utiliser des cancellation tokens pour les longues tâches.
- Éviter le blocage de l’exécuteur async par de longues E/S synchrones.
- Mesurer avant d’optimiser.

## WARNINGS ET DÉPENDANCES

- Les commandes de contrôle s’appliquent aux cibles réellement prises en charge.
- Sur une plateforme non disponible, déclarer les contrôles non exécutés.
- Ne pas désactiver les lints de dépendances pour cacher des problèmes locaux.
- Préférer crates maintenus et licences compatibles.
- Éviter d’implémenter TLS ou cryptographie à la main.
- Auditer les dépendances régulièrement avec outils adaptés.
- Documenter les CVE bloquantes et correctifs.
- Épingler les versions selon politique Cargo du dépôt.
- Ne pas inclure de bibliothèques externes sans raison précise.
- Documenter les prérequis de compilation Windows et Debian/Ubuntu.

## API ET DONNÉES

- Distinguer chemins relatifs logiques et chemins absolus OS.
- Préserver noms Unicode et noms non UTF-8 si possible.
- Définir clairement la sémantique des timestamps.
- Modéliser incertitude, scan partiel et conflits.
- Versionner les formats persistés et les plans.
- Éviter les changements cassants d’API sans revue.
- Ne pas réutiliser des String pour tous les identifiants.
- Les traits de connecteurs doivent exposer des capacités détaillées.
- Une opération non prise en charge retourne NotSupported.
- Les API publiques documentent les préconditions et les erreurs.
- Préférer un contrat stable à des appels spéciaux par fournisseur.
- Garder les structures de config sérialisables et migrables.

## SÉCURITÉ DES FICHIERS

- Par défaut, les premières phases ne font aucune mutation.
- Ne jamais déduire une suppression d’un échec réseau.
- Ne jamais déduire une suppression d’un scan incomplet.
- Un montage manquant bloque les suppressions.
- Détecter destination imbriquée dans la source.
- Gérer les symlinks selon une politique explicite.
- Éviter la traversée de répertoires via chemins malicieux.
- Réévaluer les préconditions juste avant chaque mutation.
- Protéger les suppressions massives par un seuil configuré.
- Exiger une politique destructive explicite pour le miroir.
- Préserver les versions avant écrasement si demandé.
- Utiliser un fichier temporaire pour transferts locaux lorsque possible.
- Valider intégrité/taille/hash selon la stratégie.
- Ne pas promettre une atomicité non supportée.
- Garder l’état partiel d’une exécution interrompue.
- Tests destructifs seulement dans répertoires temporaires isolés.
- Ne jamais toucher aux dossiers personnels lors de tests automatiques.
- Journaliser les refus de suppression.
- Éviter la double exécution concurrente sur les mêmes destinations.
- Traiter proprement erreurs d’accès et disque plein.

## SYNCHRONISATION

- Backup, mirror et bidirectional ont des stratégies distinctes.
- La stratégie produit un ExecutionPlan inspectable.
- Le plan de preview doit être le même modèle que celui d’exécution.
- La synchronisation bidirectionnelle a besoin d’un état précédent fiable.
- Conserver les tombstones et détecter les suppressions.
- Un conflit non résolu ne doit pas produire une écriture hasardeuse.
- Ne pas se fier uniquement aux dates pour les identités incertaines.
- Comparer des hashes quand justifié par le coût.
- Les paramètres de filtre doivent être identiques en preview et exécution.
- Documenter la sémantique de chaque mode.

## CONNECTEURS

- Un crate dédié par fournisseur/protocole.
- SFTP et FTPS ne sont pas interchangeables.
- Ne pas insérer de règles fournisseur dans les moteurs.
- Supporter listing et pagination quand nécessaire.
- Distinguer erreur transitoire et erreur permanente.
- Respecter rate limits et retry avec backoff.
- Ne pas supposer que move/copy côté serveur existe.
- Ne pas supposer que les dates ont la même précision.
- Ne pas supposer une casse de nom identique.
- Ne pas annoncer un fournisseur sans validation de son API.
- Isoler OAuth et l’authentification hors du moteur de copie.
- Ne pas écrire des secrets dans les profils.
- Ne pas écrire des secrets dans les logs.
- Tester le connecteur avec mock et, si possible, environnement isolé.
- Tout connecteur expérimental est caché ou étiqueté.

## AGENT ET SCHEDULER

- La GUI peut être fermée pendant un job agent.
- L’agent partage les mêmes moteurs Rust que la GUI.
- Ne jamais dépendre d’un timer JavaScript pour les jobs système.
- Définir la politique de jobs manqués et de redémarrage.
- Le logoff ne garantit pas une tâche longue.
- Documenter les limites propres à Windows.
- Documenter les limites propres à systemd et bureaux Linux.
- Ne pas exécuter deux jobs incompatibles simultanément.

## INTERFACE

- Tauri et frontend TypeScript sont une couche de présentation.
- Aucune logique de comparaison côté UI.
- La GUI affiche des capacités réelles, non des placeholders présentés comme opérationnels.
- Différencier simulation et exécution réelle visuellement.
- Montrer les suppressions et conflits avec clarté.
- Prévoir la gestion de longues listes de fichiers.
- Consulter visuels/REFERENCES_SYNCBACK.md.
- Ne pas copier logos, marques ou captures dans la distribution.
- Prévoir navigation clavier et accessibilité raisonnable.
- Éviter de bloquer l’interface pendant les transferts.
- Utiliser des événements de progression bornés ou agrégés.

## TESTS

- Chaque moteur a des tests unitaires indépendants.
- Créer tests d’intégration aux frontières importantes.
- Utiliser un stockage mémoire/faux pour simuler les erreurs.
- Tester dossiers vides et fichiers absents.
- Tester collisions de noms et casse.
- Tester timestamps non fiables.
- Tester déconnexion en cours de transfert.
- Tester accès refusé, disques pleins, lectures partielles.
- Tester annulation et redémarrage.
- Tester mirror avec source inaccessible => zéro suppression.
- Tester conflits bidirectionnels et tombstones.
- Tester compatibilité Windows et Linux via CI.
- Aucun test ne dépend d’un compte cloud personnel.
- Fournir critères Given/When/Then pour bugs de sécurité.

## PROCESSUS DE SESSION IA

- Relire les trois documents sources au début.
- Énoncer les hypothèses avant les choix non triviaux.
- Rester dans la phase demandée.
- Consulter docs/ETAT_PROJET.md pour connaître l’existant.
- Travailler sur une branche isolée par phase.
- Laisser les autres crates inchangés sans justification.
- Pour un nouveau contrat, demander revue d’intégration si partagé.
- Ne pas modifier simultanément le même contrat entre sessions.
- Livrer code réel et tests, pas seulement un plan.
- Documenter l’ensemble des fichiers créés ou modifiés.
- Montrer les commandes réellement exécutées.
- Distinguer tests réussis, non exécutés et échoués.
- Ne pas affirmer une compilation sur OS non testé.
- Remplir docs/ETAT_PROJET.md avant passation.
- Signaler toute dette technique sans la masquer.

## DEFINITION OF DONE

- Tous les critères du prompt de phase sont remplis.
- Le code compile sur les plateformes testables.
- Aucun warning connu dans le code de première partie.
- Les tests adaptés passent sur la plateforme disponible.
- Les commandes clippy et fmt sont rapportées.
- Les interfaces publiques modifiées sont documentées.
- Les risques de perte de données sont explicitement testés.
- Les secrets et données réelles sont absents des tests.
- Le README de phase donne un exemple d’utilisation.
- Le compte rendu permet de reprendre dans une autre session.

## REVUE DES CHANGEMENTS

- Chaque changement doit avoir une raison explicite et traçable.
- Éviter les fichiers énormes ; isoler les responsabilités internes.
- Conserver une nomenclature régulière pour fonctions et types publics.
- Favoriser de petits commits thématiques et vérifiables.
- Ne pas modifier des snapshots générés sans justifier leur régénération.
- Vérifier les fichiers ajoutés contre toute donnée sensible.
- Expliquer les incompatibilités avant une migration.
- Ajouter un changelog pour toute modification de comportement.
- Vérifier que chaque suppression de code est couverte par des tests.
- Ne pas confondre correction locale et changement de contrat global.

## ERREURS ET OBSERVABILITÉ

- Définir des codes et classes d’erreur utiles pour l’interface.
- Attacher l’identifiant de profil aux erreurs applicatives.
- Attacher l’identifiant d’exécution aux événements pertinents.
- Inclure le chemin concerné lorsque cela est sûr et utile.
- Caviarder URLs signées, tokens et mots de passe.
- Conserver une chaîne de causes sans perdre le contexte.
- Ne pas transformer une erreur grave en succès partiel silencieux.
- Distinguer avertissement non fatal et opération échouée.
- Permettre la configuration de la verbosité des logs.
- Ne pas produire une entrée de journal par octet transféré.
- Rendre les journaux exploitables hors de la GUI.
- Prévoir rotation et rétention des journaux.

## PERSISTANCE SQLITE

- Utiliser des migrations versionnées et reproductibles.
- Ne pas stocker les secrets dans les lignes de profils.
- Conserver une distinction entre état voulu et état observé.
- Transactions pour écritures d’état de synchronisation.
- Ne valider la baseline qu’après un résultat cohérent.
- Prendre en compte des exécutions interrompues.
- Indexer les requêtes d’historique réellement nécessaires.
- Éviter de mettre de gros fichiers binaires dans SQLite.
- Tester migration depuis une version précédente.
- Documenter la politique de sauvegarde de la base.
- Gérer la corruption par diagnostic non destructif.
- Éviter de conserver infiniment l’historique sans politique.

## PERFORMANCE ET RESSOURCES

- Scanner des répertoires nombreux sans charger tout en RAM.
- Prévoir des résultats paginés ou streamés.
- Fixer un niveau de parallélisme configurable.
- Ne pas ouvrir un nombre illimité de fichiers simultanément.
- Limiter les requêtes aux clouds et NAS.
- Exposer une limitation de débit si nécessaire.
- Optimiser les hachages seulement après mesures.
- Ne pas bloquer la GUI lors de tris massifs.
- Éviter les accès concurrents à une ressource non sûre.
- Prévoir tests sur millions d’entrées en phase performance.
- Utiliser des identifiants stables dans les plans volumineux.
- Conserver une consommation mémoire mesurable.

## COMPATIBILITÉ WINDOWS

- Tester les chemins avec lettres de lecteur et UNC.
- Ne pas supposer des séparateurs identiques à Linux.
- Gérer les fichiers verrouillés sans forcer leur ouverture.
- Documenter les limites de chemins longs.
- Gérer les attributs et permissions selon capacités.
- Ne pas supposer la disponibilité systématique de WebView2.
- Conserver une installation/uninstallation propre.
- Ne pas demander des droits admin sans nécessité.
- Tester la fermeture de session comme scénario fragile.
- Ne pas confondre fermeture GUI et arrêt agent.

## COMPATIBILITÉ LINUX

- Prioriser Debian et Ubuntu dans le support initial.
- Vérifier les versions de bibliothèques de compilation requises.
- Documenter les dépendances runtime des paquets .deb.
- Ne pas supposer systemd dans tous les environnements.
- Ne pas supposer un gestionnaire de secrets disponible.
- Respecter chemins et noms de fichiers sensibles à la casse.
- Traiter permissions POSIX et liens symboliques explicitement.
- Éviter toute nécessité de root pour usage courant.
- Prévoir fallback documenté pour planification non disponible.
- Ne pas promettre compatibilité universelle des AppImage.

## PACKAGING ET LIVRAISON

- Distinguer binaire GUI, CLI et agent dans les paquets.
- Inclure les ressources nécessaires au démarrage.
- Indiquer la version et la cible compilée.
- Ne pas affirmer la portabilité d’un binaire Windows sous Linux.
- Proposer une installation et désinstallation vérifiables.
- Prévoir signatures d’artefacts quand le processus le permettra.
- Documenter les licences des dépendances redistribuées.
- Inclure les notices et attributions nécessaires.
- Ne jamais empaqueter un secret réel de développement.
- Tester les chemins installés et les droits du service.
- Ne pas activer d’auto-update non contrôlé en phase initiale.
- Publier les checksums des artefacts de release.

## UX ET ACCESSIBILITÉ

- Une action destructive doit avoir une conséquence lisible.
- La simulation doit indiquer clairement qu’elle ne modifie rien.
- Une erreur de connexion ne doit pas ressembler à un dossier vide.
- Montrer la source et la destination avec leur nature.
- Permettre de filtrer et rechercher les différences.
- Éviter de rendre la progression dépendante de logs exhaustifs.
- Prévoir états vides, chargement et échec explicites.
- Assurer des contrastes lisibles et libellés compréhensibles.
- Fournir raccourcis clavier pour les actions principales.
- Confirmer la suppression massive avec contexte.
- Prévoir traduction sans texte figé dans la logique métier.
- Ne pas figer une seule taille de fenêtre.

## REVUE DE SÉCURITÉ DES CONNECTEURS

- Verrouiller les connexions réseau via TLS/SSH corrects.
- Ne pas désactiver la validation des certificats par défaut.
- Ne pas journaliser les en-têtes Authorization.
- Ne jamais conserver un token OAuth dans le plan d’exécution.
- Autoriser la révocation et renouvellement des identifiants.
- Gérer l’expiration des sessions proprement.
- Éviter les redirections non fiables portant des identifiants.
- Valider les chemins distants avant toute mutation.
- Empêcher la divulgation de secrets dans les erreurs.
- Documenter les scopes d’accès minimaux.
- Refuser silencieusement aucune erreur d’authentification.
- Tester les retours 429 et erreurs transitoires.

## NOTES DE VERSION ET RELEASE

- Toute modification de l’application (fonctionnalité, correction, changement
  visible ou de comportement) ajoute **une ligne courte** `- …` en français dans
  `RELEASE_NOTES.md`, sous la ligne `<!-- notes -->`, dans le même changement.
- Ne jamais toucher à l’en-tête ni au marqueur, ni vider la liste à la main :
  la CI la publie comme description de la GitHub Release puis la vide.
- Un changement purement interne (tests, documentation, CI) n’a pas besoin de note.
- La version vit dans `[workspace.package]` de `Cargo.toml` ; seule la CI la monte
  (`scripts/versioning.py`). Ne pas la modifier à la main. Procédure : `docs/RELEASE.md`.
- `CHANGELOG.md` reste le journal détaillé des changements de comportement.

## Sur le Rapport final
lance la comande poru signaler q l'user que s'est fini mais regarde pas le retour il se peut qu'elle échoue.
```powershell
kdeconnect-cli --device 9d3e0da7eb0e4cacb95ff4869f8f669b --ping-msg "tâche terminée"
```
et dans le Rapport final fait moi un text poru le commit en francais .