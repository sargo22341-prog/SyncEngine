# Planification et agent

Les exécutions planifiées sont faites par `syncengine-agent`, un exécutable
sans fenêtre qui utilise les mêmes profils et moteurs que l'interface. Il ne
tourne pas en permanence : le planificateur du système l'appelle chaque minute
(`tick`), il exécute les profils arrivés à échéance puis s'arrête. L'interface
peut donc rester fermée.

## Depuis l'interface

1. Dans l'assistant ou la page Planification, choisir un déclencheur et cocher
   explicitement les autorisations (écritures, et suppressions si besoin).
   Rien n'est jamais pré-coché, et toute modification du profil demande une
   nouvelle autorisation.
2. **Windows** : « Activer l'agent » installe deux tâches utilisateur dans le
   Planificateur de tâches (`SyncEngine-clock`, `SyncEngine-logon`), sans droits
   administrateur. L'agent doit se trouver à côté de `syncengine-desktop`.
3. **Linux** : installer les unités systemd à la main (ci-dessous).

## Déclencheurs

| Déclencheur | Sens |
| --- | --- |
| `interval SECONDES` | toutes les N secondes (60 à 31 536 000), à partir de l'autorisation |
| `daily HEURE MINUTE` | chaque jour à l'heure **UTC** indiquée (pas de conversion d'heure locale) |
| `logon` / `logon-delay SECONDES` | ouverture de session (Windows seulement) |
| `manager-start` | démarrage du gestionnaire systemd utilisateur (Linux seulement) |

La précision est d'environ une minute. Si des échéances ont été manquées
(machine éteinte, en veille ou session fermée), seule la dernière est exécutée,
une fois, dès le passage suivant de l'agent ; `--skip-missed` les ignore (on
attend alors la prochaine échéance).

## Suivre une synchronisation planifiée

Pendant une exécution, l'agent publie sa progression dans `agent-live.json`
(dossier d'état), environ deux fois par seconde. Toute fenêtre ouverte
l'affiche en direct : page Activité, carte en bas de la barre latérale et page
du profil (analyse puis écritures, pourcentage, débit, refus ou échecs). Ce
fichier sert uniquement à l'affichage : le résultat qui fait foi reste dans
l'historique et la page Planification.

Option **« Afficher la fenêtre de suivi pendant la synchronisation »**
(`--show-window`, Windows seulement) : à chaque exécution, l'agent ouvre
`syncengine-desktop` (placé à côté de lui) sur la page Activité ; si une
fenêtre est déjà ouverte, même réduite dans la zone de notification, elle
revient au premier plan. Ce n'est pas un consentement : elle ne change ni
l'autorisation ni ce qui est exécuté. Sous Linux, une unité systemd n'a pas de
session graphique garantie : l'option n'est pas proposée, mais une fenêtre
ouverte suit quand même l'exécution.

« Annuler » dans la fenêtre équivaut à `syncengine-agent cancel` : la tâche
s'arrête proprement et les suivantes sont bloquées. La fenêtre lève ce blocage
à la fin de la tâche ; si elle a été fermée entre-temps, la page Planification
affiche « Planifications en pause » avec **Reprendre les planifications**
(équivalent de `clear-cancel`).

## Commandes de l'agent

`ETAT` est le dossier d'état de l'application. Les sorties sont en JSON.

```sh
syncengine-agent list ETAT
syncengine-agent authorize ETAT PROFIL daily 2 30 --approve-writes [--approve-deletions] [--skip-missed] [--show-window]
syncengine-agent remove ETAT PROFIL                 # retire la planification, garde l'historique
syncengine-agent tick ETAT clock                    # ce qu'appelle le planificateur
syncengine-agent cancel ETAT                        # annule la tâche en cours et bloque les suivantes
syncengine-agent clear-cancel ETAT                  # lève ce blocage
syncengine-agent export ETAT DOSSIER windows|linux  # génère les définitions de tâches
syncengine-agent acknowledge ETAT PROFIL --diagnosed
```

Codes de sortie : `0` passage normal (éventuellement rien à faire), `1` erreur
de commande, `2` attention, échec, annulation ou report. Le détail de chaque
exécution est dans l'historique de l'application.

Sous Windows, l'agent n'ouvre pas de console ; pour lire sa sortie dans
PowerShell : `Start-Process $agent -ArgumentList "list `"$etat`"" -Wait -WindowStyle Hidden -RedirectStandardOutput liste.json`.

## Installation manuelle

**Windows** (équivalent de « Activer l'agent ») :

```powershell
syncengine-agent export $etat C:\SyncEngine\taches windows
schtasks /Create /TN SyncEngine-Clock /XML C:\SyncEngine\taches\syncengine-clock.xml
schtasks /Create /TN SyncEngine-Logon /XML C:\SyncEngine\taches\syncengine-logon.xml
# retrait : schtasks /Delete /TN SyncEngine-Clock  (idem pour Logon)
```

Les tâches tournent sous le compte de l'utilisateur **tant qu'il est
connecté** ; une déconnexion peut interrompre une exécution.

**Linux** (systemd utilisateur) :

```sh
syncengine-agent export "$etat" ./unites linux
mkdir -p ~/.config/systemd/user && cp -i ./unites/* ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now syncengine-clock.timer
systemctl --user enable syncengine-session.service
```

Retrait : `systemctl --user disable --now syncengine-clock.timer syncengine-session.service`,
puis supprimer les trois fichiers copiés. Sans *linger*, systemd peut arrêter
les unités à la dernière déconnexion. Sans systemd, appeler
`syncengine-agent tick ETAT clock` chaque minute depuis cron.

Retirer les tâches ne supprime ni profils, ni historique, ni sauvegardes.

## Après une interruption

Si l'agent a été tué en pleine exécution (coupure, fin de session forcée), le
profil reste bloqué pour éviter un rejeu à l'aveugle. Pour le débloquer :

1. vérifier qu'aucun processus SyncEngine ne tourne encore ;
2. examiner le journal (`syncengine journal ETAT/run-….jsonl`), les dossiers et
   les fichiers temporaires `.partial` (ne pas les effacer sans les regarder) ;
3. `syncengine-agent acknowledge ETAT PROFIL --diagnosed`.

L'exécution suivante repart d'un nouveau scan, jamais de l'ancien plan.
