# Notes de la prochaine version

Écrire sous la ligne `<!-- notes -->
- Miroir : une destination presque vide (ex. 1 fichier obsolète) n'est plus bloquée par le plafond de 50 %, appliqué désormais à partir de 10 suppressions.
- Simulation : la cause exacte d'un refus de suppressions est affichée, et un plafond dépassé peut être levé pour cette simulation uniquement.
- Débit d'exécution fiable : progression bloc par bloc, moyenne glissante sur 4 s, opérations/s, écriture vidée sur disque tous les 128 Mio.
- Release : seules les archives Windows et Linux sont publiées ; elles ne contiennent plus que les trois exécutables, la licence et les notices tierces.` les changements de la prochaine version, en Markdown (une
ligne `- …` par changement). À chaque push sur `main`, la CI publie ce texte comme description de
la GitHub Release, puis vide la liste dans le commit `Version X.Y.Z`. Sans notes, GitHub génère la
liste des commits. Détails : [docs/DEVELOPPEMENT.md](docs/DEVELOPPEMENT.md).

<!-- notes -->
- Fenêtre : barre de titre et bords intégrés aux couleurs de l'application (plus de cadre ni de bordure Windows) ; déplacer, agrandir (double-clic), réduire, fermer et redimensionner par les bords.
- Progression d'exécution : le pourcentage pondère volume et opérations comme FreeFileSync, affiche une décimale sous 10 %, est étiqueté « du volume » et le volume validé s'affiche en « X / Y ».
