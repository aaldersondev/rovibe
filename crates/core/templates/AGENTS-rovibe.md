## Outils MCP `rovibe`

- `agents_status`, `claim_files`, `release_files` : coordination avec les autres agents du projet.
- `studio_status` : Studio connecté, contexte (edit/server/client), état de la synchro.
- `run_luau` : exécute du Luau dans Studio et renvoie ses `print` et ses valeurs de retour. `context: "server"` ou `"client"` pendant un playtest.
- `get_tree`, `search`, `get_instance` : lecture de l'arborescence de la place. Quand plusieurs enfants portent le même nom, un chemin sans précision est refusé : `search` donne `Nom[n]` pour désigner le bon.
- `selection` : ce que l'utilisateur a sélectionné dans Studio. Quand il dit « ça » ou « cette pièce » sans chemin, c'est là.
- `get_console` : sortie de la console Studio ; ne te renvoie que les lignes que tu n'as pas encore lues (`recent` pour relire).
- `playtest_check` : vérifie une fonctionnalité dans un test neuf, en un appel, avec un verdict PASS / FAIL / INCONCLUSIVE. À préférer aux étapes séparées.
- `playtest` : démarre ou arrête un test à la main, et attend qu'il soit prêt.
- `play_move` : fait marcher le personnage jusqu'à une position ou une instance pendant un test Play.
- `play_input` : vraies touches, vrais clics et texte dans le jeu (interfaces, outils, ProximityPrompt), par les entrées du moteur Roblox. Aussi : clic droit, molette, glisser, et `look` pour tourner la caméra. Un clic `gui` sur un bouton échoue s'il ne l'a pas activé. Se joue en arrière-plan, sans prendre le clavier ni la souris de l'utilisateur.
- `check_code` : vérifie le code sans lancer le jeu (compilation roblox-ts pour un projet TypeScript, selene et types Luau sinon). Les erreurs d'un fichier que tu viens d'écrire te sont aussi renvoyées automatiquement.
- `screenshot` : capture la vue 3D de Studio pour voir le rendu réel ; `frame` vise une cible sous un angle donné, puis remet la caméra en place.
- `capture_sequence` : rafale de captures en planche-contact horodatée, pour juger une animation, un effet ou une transition.
- `rig_check`, `asset_check`, `ui_check` : un modèle tient-il ensemble, est-il bien posé, l'interface déborde-t-elle de l'écran.
- `performance_check` : mesure le jeu pendant quelques secondes (images par seconde, temps d'image moyen et p95, mémoire, nombre d'instances), dans Studio ou pendant un test.
- `studio_wait`, `studio_release` : un seul agent à la fois pilote Studio (tests, captures, entrées). Si on te dit qu'un autre pilote, attends ton tour avec `studio_wait` ; rends la main avec `studio_release` quand tu as fini.
- `publish` : met la place en ligne, après accord de l'utilisateur dans l'app. Uniquement s'il te l'a demandé.
- `asset_search`, `asset_insert` : cherche et insère des assets de la banque locale (`bank:<id>`) ou du Creator Store (gratuits). Les scripts d'un asset du Store arrivent désactivés.
- `asset_save` : enregistre une instance de la place dans la banque locale.
- `sync_connect` : connecte Studio au serveur de synchro du projet. Sur un projet protégé, l'utilisateur doit d'abord l'accepter dans l'app, puis valider les changements dans Studio.

## Vérifier un changement

1. Modifie les fichiers, puis `check_code`.
2. `playtest_check` : `setup_luau` prépare la scène, `walk_to` déplace le personnage, `assert_luau` affiche `FAIL` (ou renvoie false) si la fonctionnalité ne marche pas. Pour un parcours en plusieurs temps, donne plutôt `steps`, joués dans l'ordre (marcher, appuyer, attendre, vérifier, capturer, mesurer). `players` lance plusieurs joueurs. Lis le verdict et les consoles du rapport. `save_as` garde le scénario pour le rejouer après chaque changement.
3. Pour explorer à la main : `playtest` avec `action: "start"`, puis `play_move`, `play_input`, `screenshot`, `get_console`, `run_luau` en contexte `server` ou `client`, et `playtest` avec `action: "stop"`.

## Fichiers de l'utilisateur

Les images et fichiers que l'utilisateur colle ou dépose dans ton terminal sont enregistrés dans `.rovibe/attachments/` (hors de git) ; leur chemin arrive dans son message. Ouvre-les comme n'importe quel fichier : une capture d'écran montre souvent ce qu'il veut dire mieux que ses mots.
