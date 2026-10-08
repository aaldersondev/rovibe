# {{name}}

Jeu Roblox développé sur fichiers. Roblox Studio reflète ce dossier en direct via Essaim Sync (fork de Rojo).

## Règles de travail

- Le code vit dans `src/`. Modifie les fichiers ici, jamais les scripts dans Studio : la synchro les écraserait.
{{layout}}
- Suffixes : `.server.luau` (Script), `.client.luau` (LocalScript), `.luau` (ModuleScript). `init.*.luau` donne son type au dossier.
- Ce qui n'est pas du code (map, modèles, interfaces posées à la main) reste dans la place Studio. Pour le lire ou le modifier, passe par les outils MCP `essaim`.
- Plusieurs agents travaillent en parallèle sur ce dossier. Avant de commencer, appelle `agents_status` pour voir qui fait quoi, puis `claim_files` sur les fichiers ou dossiers de ta tâche. Si une modification est refusée parce qu'un autre agent tient le fichier, n'insiste pas : avance sur autre chose. Termine par `release_files`.
- Le projet est un dépôt git. Fais un commit quand une tâche est finie et vérifiée ; ne réécris pas l'historique.

## Outils MCP `essaim`

- `agents_status`, `claim_files`, `release_files` : coordination avec les autres agents du projet.
- `studio_status` : Studio connecté, contexte (edit/server/client), état de la synchro.
- `run_luau` : exécute du Luau dans Studio et renvoie ses `print` et ses valeurs de retour. `context: "server"` ou `"client"` pendant un playtest.
- `get_tree`, `search`, `get_instance` : lecture de l'arborescence de la place.
- `get_console` : sortie de la console Studio, avec un curseur pour ne lire que le nouveau.
- `playtest` : démarre ou arrête un test, et attend qu'il soit prêt.
- `play_move` : fait marcher le personnage jusqu'à une position ou une instance pendant un test Play.
- `play_input` : vraies touches et vrais clics dans le jeu (interfaces, outils, ProximityPrompt). Prend brièvement le premier plan.
- `check_code` : vérifie le code (selene + types Luau) sans lancer le jeu. Les erreurs d'un fichier que tu viens d'écrire te sont aussi renvoyées automatiquement.
- `screenshot` : capture la vue 3D de Studio pour voir le rendu réel.
- `publish` : met la place en ligne, après accord de l'utilisateur dans l'app. Uniquement s'il te l'a demandé.
- `asset_search`, `asset_insert` : cherche et insère des assets de la banque locale (`bank:<id>`) ou du Creator Store (gratuits). Les scripts d'un asset du Store arrivent désactivés.
- `asset_save` : enregistre une instance de la place dans la banque locale.
- `sync_connect` : connecte Studio au serveur de synchro du projet.

## Vérifier un changement

1. Modifie les fichiers, puis `check_code`.
2. `playtest` avec `action: "start"`.
3. Joue la fonctionnalité : `play_move` pour se déplacer, `play_input` pour cliquer et appuyer, `screenshot` pour voir.
4. `get_console` pour lire les erreurs, `run_luau` en contexte `server` ou `client` pour vérifier l'état.
5. `playtest` avec `action: "stop"`.
