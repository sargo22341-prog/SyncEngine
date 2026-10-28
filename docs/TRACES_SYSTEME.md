# Traces laissées sur le système

SyncEngine est portable : il n’a pas d’installateur. Ce document est
l’**inventaire exhaustif** de ce qu’il écrit en dehors du dossier du programme.
Réglages › Données › **Supprimer toutes les données de SyncEngine** retire
exactement ces traces (code : `apps/desktop/src/purge.rs`) ; il ne reste
ensuite qu’à supprimer le dossier du programme.

**Règle (voir `AGENTS.md`)** : toute nouvelle trace — fichier, dossier, tâche,
clé de registre, unité — est ajoutée à cet inventaire **et** au nettoyage, dans
le même changement.

## Windows

| Trace | Emplacement | Créée par | Retrait |
| --- | --- | --- | --- |
| Dossier d’état | `%APPDATA%\org.syncengine.desktop\` | interface, CLI, agent | dossier entier |
| Verrous d’exécution | `%LOCALAPPDATA%\SyncEngine\job-locks\execution.lock` | toute exécution | dossier `SyncEngine` entier |
| Tâche de l’agent résident | `\SyncEngine-<SID>-<empreinte 16>-resident` | enregistrement d’un horaire | tâche |
| Anciennes tâches par dossier d’état | `\SyncEngine-<SID>-<empreinte 16>-clock`, `-logon` | versions antérieures | tâche |
| Anciennes tâches globales | `\SyncEngine-resident`, `\SyncEngine-clock`, `\SyncEngine-logon` | versions antérieures, installation manuelle | tâche |
| Démarrage de l’interface | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, valeur `SyncEngine` | Réglages › Démarrage | valeur |
| Exports temporaires de tâches | `<dossier d’état>\native-tasks-<pid>-<nanos>\` | enregistrement interrompu | avec le dossier d’état |
| Fichier d’envoi temporaire | `%TEMP%\.syncengine-….partial` | copie vers un serveur distant interrompue | fichier |

Une tâche n’est retirée que si son nom commence par `SyncEngine-`, si son
unique action lance `syncengine-agent.exe` et si son principal est le compte
courant (SID, ou nom `DOMAINE\utilisateur` résolu en SID). Le retrait demande
l’UAC seulement après un refus d’accès, comme l’enregistrement.

## Linux

| Trace | Emplacement | Créée par | Retrait |
| --- | --- | --- | --- |
| Dossier d’état | `$XDG_DATA_HOME/org.syncengine.desktop/` ou `~/.local/share/org.syncengine.desktop/` | interface, CLI, agent | dossier entier |
| Verrous d’exécution | `$XDG_STATE_HOME/SyncEngine/job-locks/` ou `~/.local/state/SyncEngine/job-locks/` | toute exécution | dossier `SyncEngine` entier |
| Unités systemd utilisateur | `$XDG_CONFIG_HOME/systemd/user/` (ou `~/.config/systemd/user/`) : `syncengine-clock.service`, `syncengine-clock.timer`, `syncengine-session.service` | installation manuelle depuis `syncengine-agent export … linux` | `systemctl --user disable --now`, fichiers, `daemon-reload` |
| Activation des unités | `timers.target.wants/syncengine-clock.timer`, `default.target.wants/syncengine-clock.service`, `default.target.wants/syncengine-session.service` | `systemctl --user enable` | liens (jamais suivis) |
| Démarrage de l’interface | `$XDG_CONFIG_HOME/autostart/` : `syncengine.desktop`, `syncengine-desktop.desktop`, `org.syncengine.desktop.desktop` | l’utilisateur (SyncEngine n’en crée pas) | fichier, seulement s’il lance `syncengine-desktop` |
| Fichier d’envoi temporaire | `$TMPDIR/.syncengine-….partial` (ou `/tmp`) | copie vers un serveur distant interrompue | fichier |

Une unité n’est retirée que si elle porte l’un de ces noms exacts et contient
`syncengine-agent` (services) ou `Description=SyncEngine schedule dispatcher`
(minuteur). Sans systemd, l’échec de `systemctl` est signalé dans le bilan ;
les fichiers sont retirés quand même.

## Contenu du dossier d’état

Voir [le guide](GUIDE.md#données-de-lapplication). Bases SQLite (et leurs
fichiers `-wal`, `-shm`, `-journal`), journaux `run-….jsonl`, `Corbeille/`,
`versions/` (anciennes versions), fichiers de l’agent (`agent-*.json`,
`agent.lock`, `*.request`), `desktop.lock`, `window.lock`, `desktop-ui.json`,
leurs remplacements temporaires `*.tmp` et les exports `native-tasks-…`.

## Dossier d’état personnalisé

`SYNCENGINE_STATE_DIR` choisit un autre dossier d’état. S’il est celui de
l’interface ouverte ou s’il est référencé par une tâche ou une unité de
l’agent, il est inclus : seuls les noms ci-dessus y sont supprimés, puis le
dossier lui-même s’il est vide (sinon il est conservé et le bilan l’indique).

## Déroulé de la suppression

1. Refus si une synchronisation est en cours (fenêtre, agent), puis liste
   exacte avec les tailles et la place des corbeilles perdues.
2. Confirmation en tapant `SUPPRIMER`.
3. Arrêt coopératif des agents (`agent-stop.request`), attente de leur sortie
   (20 s au plus) : sinon rien n’est supprimé.
4. Vérification qu’aucune commande ni exécution ne tient ses verrous ; retrait
   des tâches, de la valeur `Run`, des unités et fichiers isolés ; libération
   des verrous ; suppression des dossiers.
5. Bilan (supprimé / impossible avec la cause), puis fermeture du logiciel.

Garanties : aucune suppression récursive ne suit un lien symbolique ou une
jonction (le lien lui-même est retiré) ; un dossier qui contient une source ou
une destination locale d’un profil est refusé ; rien d’autre que les noms
exacts ci-dessus n’est touché.

## Limites

- Un dossier d’état personnalisé qui n’est ni ouvert ni référencé par une tâche
  ou une unité ne peut pas être retrouvé : le supprimer à la main.
- Les fichiers temporaires laissés **sur une destination** par une copie
  interrompue (`.syncengine-….partial`, réservations `.syncengine-…` et
  `.syncengine-lock` sur un serveur) ne sont pas recherchés : « Reprendre » une
  interruption retire ceux qui sont consignés dans son journal.
- Linux : ce parcours n’a été ni compilé ni testé sous Linux lors de son
  ajout (développement sous Windows) ; la suppression réelle des unités
  systemd reste à valider sur une machine Linux.
