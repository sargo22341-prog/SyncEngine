# Notes de la prochaine version

Écrire les changements de la prochaine version, en Markdown (une ligne `- …`
par changement), **sous la ligne de marqueur seule ci-dessous** (jamais
dans cet en-tête). À chaque push sur `main`, la CI publie ce texte comme
description de la GitHub Release, puis vide la liste dans le commit
`Version X.Y.Z`. Sans notes, GitHub génère la liste des commits.
Détails : [docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

<!-- notes -->
- Mini-suivi plus large (bouton « Agrandir » plus à l'aise) et sans textes explicatifs superflus.
- Accueil : changer de filtre (Tous · En cours · À vérifier) ne fait plus disparaître les filtres et les groupes quand un filtre est vide ; seuls les profils sont masqués.
- Accueil › « À traiter » : « Examiner » ouvre une fenêtre qui liste les problèmes de la dernière exécution (exclure un fichier, relancer une simulation, ouvrir l'historique) ; « Renouveler » renouvelle une ou plusieurs autorisations de planification (liste à cocher).
- Page d'un profil : un clic sur une des dernières exécutions ouvre précisément celle-ci dans l'historique.
- Interface allégée : suppression de textes d'information redondants (simulation Miroir, assistant, planification de la semaine).
