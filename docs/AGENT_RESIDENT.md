# Agent utilisateur résident Windows

## Choix et garanties

Une tâche Task Scheduler par compte et dossier d'état lance directement
`syncengine-agent resident ETAT`, sous `InteractiveToken` et `LeastPrivilege`.
Elle a un déclencheur de connexion, aucune répétition périodique, aucune limite
de durée, `IgnoreNew`, et trois tentatives de relance espacées d'une minute.
L'installation demande l'UAC uniquement après un refus d'accès ; le principal
reste le SID du compte initial, y compris si un autre administrateur autorise.
Une erreur métier de synchronisation produit un reçu et ne tue pas l'agent.
Une erreur de processus/persistance termine l'agent avec le code 1.

Comparaison des mécanismes :

| Mécanisme | Décision |
| --- | --- |
| Clé Run / dossier Démarrage | démarre à la connexion mais ne supervise pas les échecs |
| Service système SCM | contexte de service, installation et gestion de compte supplémentaires ; ne correspond pas à notre agent interactif ordinaire |
| Services Windows par utilisateur | infrastructure de modèles de services et cycle de vie supplémentaire ; non retenue pour cette migration portable |
| Task Scheduler | conserve l'installation existante et le jeton interactif ordinaire ; relances bornées et limites explicites |

`RestartOnFailure` n'est pas un watchdog : il ne détecte pas un processus
bloqué et n'offre aucune relance infinie. Après épuisement des tentatives,
réenregistrer un horaire ou rouvrir l'application relance la tâche ; la
prochaine connexion est aussi un nouveau déclencheur. Le comportement réel
de relance après un crash doit encore être validé en compte Windows jetable.
La déconnexion peut tuer le processus et interrompre les E/S. Aucune promesse
d'exécution après fermeture de session ou extinction n'est faite.

## Horaires, connexion et reprise

Le scheduler et l'application restent ouverts dans le processus ; il relit les
horaires au plus toutes les cinq secondes au repos, avec des attentes de
thread. Il ne rouvre pas les bases à chaque passage. Une exécution longue est
séquentielle : les modifications attendent sa fin pour être distribuées.
La prise d'une échéance compare à nouveau la configuration SQLite ; les
consentements et les préconditions sont encore vérifiés par l'application.
Les verrous d'exécution existants couvrent aussi l'interface et la CLI, même
avec plusieurs dossiers d'état du même compte.

La session provient de `WTSQuerySessionInformationW(WTSSessionInfo)` pour la
session courante : identifiant de session + `LogonTime` complet en FILETIME.
Il n'y a pas de requête administrateur sur les sessions d'autres utilisateurs.
PowerShell exécute un petit pont P/Invoke à structures marshalisées ; aucun
`unsafe` Rust ni dépendance binaire supplémentaire. La lecture est limitée à
20 secondes, et un échec WTS empêche le démarrage : aucun faux événement n'est
inventé. L'identité n'est pas affichée dans les journaux.

Ce choix évite de persister seulement `AuthenticationId`, LUID dont l'unicité
est locale au démarrage. Deux contextes d'authentification dans la même session
interactive utilisent la même identité WTS. Un déverrouillage, une reconnexion
RDP et une sortie de veille ne changent pas `LogonTime`. La réutilisation d'un
numéro de session après redémarrage est distinguée par l'horodatage complet.
Une collision d'identité au même instant en secondes est refusée ; une session
plus ancienne ne remplace pas une observation plus récente (choix conservateur
en cas de recul de l'horloge système).

La migration SQLite v4 ajoute une seule ligne de session, sans effacer les
profils, consentements, reçus, historiques, baselines ou versions. Le délai
après connexion part de **la véritable connexion**, pas de l'arrivée de
l'agent. La connexion et les délais restent distribuables sur les passages
d'horloge après une relance, sans nouvel événement. Un horaire créé après
cette connexion attend la suivante. Les anciens reçus de connexion arrondis
à la minute sont raccordés pour empêcher un rejeu lors de la migration.
Les prises interrompues restent bloquées jusqu'au diagnostic explicite.

Après veille ou saut d'horloge, une seule échéance récente est retenue par
profil, sans rejouer la file des occurrences. `Skip` ignore les échéances
retardées de 60 secondes ou plus. Un recul d'horloge ne rejoue pas les reçus.
Les horaires quotidiens restent en UTC. Linux conserve ses unités systemd et
sa sémantique `manager-start` : aucun événement Windows n'y est simulé.

## Présence, pause et arrêt

Le suivi ouvert pendant un transfert affiche le même graphe de débit que la page
Activité (moyenne sur quatre secondes, trente secondes de courbe), le pourcentage,
les opérations et le nom du profil. Les mesures commencent à l’ouverture du suivi :
aucun historique antérieur n’est inventé. Les observations périmées arrêtent le suivi.
**Annuler** demande l’annulation coopérative du transfert courant ; les planifications
reprennent ensuite. **Arrêter l’agent** demande l’arrêt coopératif du processus et
reste persistant jusqu’à **Reprendre les planifications**. Aucune pause avec reprise
au même octet n’est proposée. Une E/S OS bloquée reste non interruptible de force.

Si plusieurs profils sont dus à la même connexion, ils passent l’un après l’autre,
dans l’ordre stable des identifiants de profil, sans priorité configurable.
Le panneau **File d’attente**, présent dans le suivi initial et dans Activité,
affiche le profil actif avec un indicateur animé puis les profils encore en attente.
Il observe `agent-queue.json` (format v1, au plus 1000 profils et 2 Mio) ; il ne décide
ni des échéances ni des mutations. Chaque prise revérifie la configuration persistée :
un profil modifié ou retiré pendant l’attente peut donc être écarté. Une erreur métier
d’un profil ne déclenche pas tous les autres en parallèle ; l’annulation interrompt
la distribution et laisse les profils non pris disponibles pour le passage suivant.
Exemple : programmer trois profils à la connexion ; A termine avant le démarrage de B,
puis C. L’interface conserve uniquement un historique borné de mesures en mémoire.

Après un arrêt forcé, l’agent peut être présent alors que le profil reste bloqué
sur une prise interrompue. Dans le profil, choisir **Diagnostiquer l’interruption**,
vérifier l’historique et les fichiers, puis confirmer **J’ai vérifié : lever le
blocage**. L’ancienne exécution reste un échec ; les fichiers temporaires et la
corbeille sont conservés, le journal est supprimé après l'acquittement. La
fenêtre « Synchronisation interrompue » affichée au démarrage offre aussi
**Reprendre** (retrait des fichiers temporaires consignés puis nouvelle
simulation). La prise correspondante est acquittée sans être rejouée. Il devient ensuite possible de retirer le profil ou revoir son horaire.
Une exécution réellement active reste protégée par les verrous de commande et
d’exécution. L’action de diagnostic ne lance pas elle-même un transfert ; les
échéances futures autorisées restent actives et repartent d’une nouvelle analyse.

Ouvrir la fenêtre pendant une synchronisation planifiée peut rencontrer le
verrou de commande. La fenêtre affiche alors l’attente et la progression récente
de l’agent, sans ouvrir ni migrer les bases. Elle réessaie automatiquement chaque
seconde et ouvre l’interface complète après libération du verrou. Une observation
périmée n’est jamais affichée comme un transfert actif. En sens inverse, le
résident attend si une commande manuelle occupe l’état à son démarrage ; un arrêt
ou une demande de retrait reste pris en compte pendant cette attente. Les erreurs
de permissions, d’intégrité et de schéma ne sont pas réessayées silencieusement.
Exemple : lancer une synchronisation puis ouvrir la fenêtre ; le transfert
continue, son avancement est visible et la fenêtre s’ouvre après sa fin.

`agent.lock` est un verrou OS gardé ouvert : une seconde instance, même dans
une autre session du même compte, est refusée. La première session possède
l'agent de ce dossier jusqu'à sa sortie ; un second bureau ne déclenche pas
ses propres horaires de connexion en parallèle. Des dossiers d'état distincts
ont des résidents distincts et partagent l'exclusion d'exécution du compte.

`agent-status.json` est une observation bornée remplacée en entier. La GUI et
`status` vérifient le verrou actuel : après crash, un ancien fichier ne prouve
jamais la présence. L'âge de l'observation est affiché comme suspect après
30 secondes sans suivi de tâche. Le résultat réel est dans les reçus SQLite et
l'historique, jamais déduit de la seule présence du processus.

| Commande | Effet |
| --- | --- |
| `cancel ETAT` | annule coopérativement le travail et suspend les suivants ; résident maintenu |
| `clear-cancel ETAT` | reprend les planifications dans le résident existant |
| `stop ETAT` | annule puis sort proprement ; marqueur persistant, même à la prochaine connexion |
| `resume ETAT` | lève l'arrêt/la pause et exécute le résident au premier plan ; à utiliser sans résident actif |
| `status ETAT` | présence et dernière phase ; ne signifie pas « sauvegarde réussie » |

Pour retrouver la supervision native après un arrêt explicite : ouvrir la page
Planification puis **Reprendre les planifications**. Ce parcours lève le marqueur
et demande le lancement de la tâche ordinaire. `resume` lancé directement reste un processus de terminal, sans la
supervision de la tâche Windows. La fermeture de la GUI n'envoie aucun arrêt.
L'annulation est observée toutes les 100 ms pendant une distribution, entre
les opérations coopératives des moteurs. Une E/S OS bloquée ne peut pas être
interrompue de force sans risque ; le résident n'est jamais tué automatiquement
au milieu d'une mutation pour prétendre débloquer une sauvegarde.

## Migration et installations portables

À l'ouverture avec des horaires actifs, et dans les parcours profil, groupe
et assistant, l'application prépare automatiquement la tâche commune. Les
horaires restent dans SQLite : aucun objet natif par profil. Le nom comprend
SID et empreinte du chemin d'état canonique.

La nouvelle définition est enregistrée désactivée (`TASK_CREATE_OR_UPDATE |
TASK_DISABLE`), puis les anciennes tâches vérifiées sont désactivées. Cela
empêche une connexion simultanée de démarrer la nouvelle architecture trop tôt.
La tâche résidente n'est activée qu'après le retrait des anciennes tâches.
Une ancienne tâche encore en cours bloque le retrait et le
lancement immédiat, avec diagnostic invitant à réessayer après sa fin. Les
tâches désactivées restent récupérables ; une nouvelle tentative reprend la
migration. Les noms historiques globaux et les noms par état sont examinés.
L'appartenance exige une seule action `syncengine-agent.exe`, les arguments
exacts du dossier d'état, le SID attendu et un principal interactif à privilèges
ordinaires. Le compte renvoyé par Windows (SID ou nom `DOMAINE\utilisateur`)
est résolu en SID avant comparaison ; un compte non résolu reste refusé.
Après une mise à jour corrigeant ce refus, réenregistrer le même horaire
suffit : ne pas supprimer manuellement la tâche.
Les instances actives sont interrogées avec `GetInstances(0)`,
indépendamment du statut désactivé de la définition. Une tâche étrangère sous notre nom
isolé provoque un refus ; une tâche historique globale étrangère est conservée.
Les erreurs d'UAC et installations partielles restent des erreurs de
supervision, jamais des succès ni de simples erreurs de stockage.

Changer l'emplacement du dossier d'état crée une autre installation : retirer
l'ancienne depuis l'ancien état. Changer seulement le répertoire des binaires
actualise l'action à la prochaine préparation ; si l'ancien résident tourne,
l'arrêter proprement avant de réenregistrer. Garder les trois exécutables
ensemble. Aucun installateur n'est ajouté. La mise à jour depuis Réglages
(voir [le guide](GUIDE.md#mises-à-jour)) remplace les fichiers au même
endroit : la tâche, dont le nom ne dépend pas du chemin de l'exécutable, est
conservée. L'agent est arrêté avant le remplacement puis relancé par la tâche
ordinaire ; un arrêt explicite antérieur reste en place.
Retirer la dernière autorisation demande l'arrêt coopératif via
`agent-retire.request` et retire uniquement les tâches vérifiées. Un nouvel
horaire enlève ce marqueur automatique, jamais le marqueur d'arrêt explicite.

## Exemple complet

1. Clic droit sur un profil (ou un groupe) → **Planifier** → intervalle de
   15 minutes → autoriser les écritures → enregistrer. Le consentement aux
   suppressions reste séparé et les garde-fous existants s'appliquent.
2. Fermer la fenêtre. Le résident continue sous le compte ordinaire.
3. Rouvrir → **Planification** : vérifier la présence, le dernier reçu, puis
   **Historique** pour le résultat. `syncengine-agent status ETAT` et
   `syncengine-agent list ETAT` fournissent aussi ces deux vues séparées.
4. Revoir l'horaire, choisir 60 minutes et enregistrer. Un consentement valide
   sur le profil inchangé reste conservé ; le résident prend en compte ce
   changement au prochain passage libre, sans redémarrage manuel.
5. Retirer l'horaire. Le résident ne le distribue plus. Après le dernier
   horaire, son arrêt et le retrait natif sont demandés ; profils et données
   sauvegardées restent conservés. Une prise en cours exige d'attendre sa fin.

## Journaux et validation

En cas d’« Erreur de l’agent », la page Planification affiche le diagnostic
du processus et son étape : `scheduler_open`, `windows_session`,
`session_observation`, `application_open` ou `scheduler_dispatch`.
Le même diagnostic est conservé dans le champ facultatif `diagnostic` de
`agent-status.json` et retourné par `syncengine-agent status ETAT`.
Il est caviardé et limité à 2048 caractères. Les anciens fichiers d’état
restent lisibles. Un nouveau démarrage publie `starting` et retire l’ancienne
erreur ; les résultats des sauvegardes restent dans leurs reçus et historiques.
Par exemple, transmettre cette observation après un échec permet de distinguer
un verrou occupé (`application_open [busy]`) d’une base illisible, sans compiler
une version debug ni enregistrer des logs continus.

Le résident n'écrit aucun journal à chaque passage : présence, session et
dernier reçu sont de taille bornée. `agent-live.json` conserve seulement la
dernière observation. Les sorties de commande sont JSON ; les erreurs métier
restent dans l'historique applicatif (30 exécutions terminées par profil,
moins si leurs plans dépassent 512 Mio, les 3 dernières étant toujours gardées).
Les journaux durables `.jsonl` associés aux exécutions terminées suivent
maintenant cette même rétention de 30 exécutions par profil. Seules les
identités terminées encore présentes en base autorisent leur retrait ; les
exécutions interrompues non diagnostiquées et les fichiers étrangers sont conservés. Une erreur de
nettoyage est signalée et bloque la validation de fin, sans succès silencieux.
Les anciens journaux déjà orphelins avant cette migration restent conservés
pour diagnostic manuel ; aucune suppression n'est déduite de leur seul nom.
La politique de conservation des versions sauvegardées reste inchangée.

Les tests utilisent horodatages imposés, deux connexions SQLite, un superviseur
en mémoire et des processus dans des répertoires temporaires. Les validations
COM utilisent `TASK_VALIDATE_ONLY` et ne créent aucune tâche. La lecture WTS
de la session courante est testée nativement. Installation réelle, redémarrage
automatique après crash, acceptation/refus UAC et déconnexion/reconnexion sont
à vérifier dans un compte ou une VM jetable ; aucune de ces actions n'est
testée sur le compte personnel. Linux n'est pas validé sur cette machine.
La surveillance des fichiers n'est pas implémentée.

Sources Microsoft : [RestartOnFailure](https://learn.microsoft.com/en-us/windows/win32/taskschd/taskschedulerschema-restartonfailure-settingstype-element),
[requête WTS et permissions](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw),
[WTSINFO](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ns-wtsapi32-wtsinfow),
[TOKEN_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-token_statistics),
[drapeaux de création des tâches](https://learn.microsoft.com/en-us/windows/win32/api/taskschd/ne-taskschd-task_creation),
[instances actives](https://learn.microsoft.com/en-us/windows/win32/api/taskschd/nf-taskschd-iregisteredtask-getinstances),
[services par utilisateur](https://learn.microsoft.com/en-us/windows/application-management/per-user-services-in-windows).
