# {{name}}

Jeu Roblox développé sur fichiers. Roblox Studio reflète ce dossier en direct via RoVibe Sync (fork de Rojo).

## Règles de travail

- Le code vit dans `src/`. Modifie les fichiers ici, jamais les scripts dans Studio : la synchro les écraserait.
{{layout}}
{{code}}
- Ce qui n'est pas du code (map, modèles, interfaces posées à la main) reste dans la place Studio. Pour le lire ou le modifier, passe par les outils MCP `rovibe`.
- Plusieurs agents travaillent en parallèle sur ce dossier. Avant de commencer, appelle `agents_status` pour voir qui fait quoi, puis `claim_files` sur les fichiers ou dossiers de ta tâche. Si une modification est refusée parce qu'un autre agent tient le fichier, n'insiste pas : avance sur autre chose. Termine par `release_files`.
- Le projet est un dépôt git. Fais un commit quand une tâche est finie et vérifiée ; ne réécris pas l'historique.

{{rovibe}}
