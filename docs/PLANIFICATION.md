# Planification et agent

Les exécutions planifiées sont faites par `syncengine-agent`, un exécutable
sans fenêtre qui utilise les mêmes profils et moteurs que l'interface. Il ne
tourne pas en permanence : le planificateur du système l'appelle chaque minute
(`tick`), il exécute les profils arrivés à échéance puis s'arrête. L'interface
peut donc rester fermée.

## Depuis l'interface

1. Dans l'assistant ou la page Planification, choisir un déclencheur et cocher
   explicitement les autorisations (écritures, et suppressions si besoin).
   Un nouvel horaire n'a aucune autorisation pré-cochée. En édition, les
   autorisations existantes sont conservées si le profil est inchangé ; une
   modification du profil les invalide. Un groupe ne reprend les cases que si
   tous ses membres ont les mêmes réglages et une autorisation valide.
2. **Windows** : enregistrer l'horaire installe ou actualise automatiquement
   les deux tâches partagées (horloge et ouverture de session). Aucun bouton
   d'activation supplémentaire. L'agent doit se trouver à côté de
   `syncengine-desktop.exe`. Windows PowerShell et le service Planificateur
   de tâches doivent être disponibles. Une demande UAC apparaît seulement
   si l'enregistrement retourne « accès refusé » ; la refuser empêche de
   confirmer la nouvelle planification. Les tâches exécutent toujours l'agent
   avec les droits ordinaires du compte initial, même si un autre compte
   administrateur autorise l'UAC.
3. **Linux** : installer les unités systemd à la main (ci-dessous).

Les noms Windows incluent le SID du compte et une empreinte du chemin absolu
du dossier d'état : deux dossiers d'état ne remplacent pas leurs tâches.
Les horaires restent dans SQLite, sans tâche Windows individuelle par profil.
Modifier un horaire actualise ces tâches ; retirer un profil ou un horaire
retire son autorisation. Les deux tâches sont supprimées après retrait de la
dernière planification active. Un échec de nettoyage est signalé ; une tâche
résiduelle ne trouve plus l'autorisation supprimée et ne synchronise rien.
Les anciennes tâches `SyncEngine-clock` / `SyncEngine-logon` sont retirées
lors de l'enregistrement si leur action correspond exactement à l'agent et
au dossier d'état utilisés. Les tâches étrangères ne sont pas supprimées.

Exemple : clic droit sur un profil → **Planifier…** → **Ouverture de session**
→ autoriser les écritures → enregistrer (accepter l'UAC si demandée).
À la prochaine ouverture de session du compte concerné, Windows appelle
l'agent, même si la fenêtre est fermée. Pour un délai, choisir le déclencheur
différé ; l'événement est persisté puis repris par les passages de l'horloge.
En cas d'échec, le dialogue affiche le diagnostic d'enregistrement Windows ;
les profils déjà enregistrés dans un groupe restent explicitement indiqués.

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

**Windows** (alternative CLI ; l'interface réalise l'installation automatiquement) :

```powershell
syncengine-agent export $etat C:\SyncEngine\taches windows
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
foreach ($event in 'clock', 'logon') {
    $xml = [IO.File]::ReadAllText("C:\SyncEngine\taches\syncengine-$event.xml")
    $xml = $xml.Replace('<LogonType>', "<UserId>$sid</UserId><LogonType>")
    $xml = $xml.Replace('</LogonTrigger>', "<UserId>$sid</UserId></LogonTrigger>")
    Register-ScheduledTask -TaskName "SyncEngine-$event" -Xml $xml -Force
}
# retrait : schtasks /Delete /TN SyncEngine-Clock  (idem pour Logon)
```

Les tâches tournent sous le compte de l'utilisateur **tant qu'il est
connecté** ; une déconnexion peut interrompre une exécution.
Ne pas combiner cette installation manuelle avec les tâches automatiques de
l'interface. Pour une utilisation normale, enregistrer depuis l'interface.
Cette variante manuelle peut nécessiter un terminal élevé ; elle n'intègre
pas la demande UAC automatique de l'interface.

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

## Validation des corrections Windows

- Étant donné un miroir déjà autorisé, ouvrir son horaire sans changer le
  profil conserve les autorisations ; changer la destination ou les filtres
  dans l'assistant exige de les recocher.
- Étant donné un groupe contenant un profil non autorisé ou modifié, ouvrir
  l'horaire du groupe ne reprend pas les consentements d'un autre membre.
- Étant donné un refus d'accès à l'enregistrement, seule l'installation
  demande l'UAC ; l'identité et les droits d'exécution restent ceux du compte
  qui a ouvert l'application. Sans accord UAC, aucun nouvel horaire n'est
  autorisé par ce parcours.
- Les tests COM emploient `TASK_VALIDATE_ONLY` : aucune tâche persistante ni
  synchronisation réelle n'est créée par les tests automatiques ordinaires.
  L'acceptation/refus interactif de l'UAC et une réelle fermeture/réouverture
  de session nécessitent une validation manuelle dans un compte de test.

Référence consultée : les sources locales FreeFileSync
`Source/application.cpp` chargent les configurations `.ffs_batch` et les
exécutent sans l'interface normale. SyncEngine garde son agent autonome et
ses autorisations persistées, avec installation intégrée au parcours graphique.

## Après une interruption

Si l'agent a été tué en pleine exécution (coupure, fin de session forcée), le
profil reste bloqué pour éviter un rejeu à l'aveugle. Pour le débloquer :

1. vérifier qu'aucun processus SyncEngine ne tourne encore ;
2. examiner le journal (`syncengine journal ETAT/run-….jsonl`), les dossiers et
   les fichiers temporaires `.partial` (ne pas les effacer sans les regarder) ;
3. `syncengine-agent acknowledge ETAT PROFIL --diagnosed`.

L'exécution suivante repart d'un nouveau scan, jamais de l'ancien plan.
