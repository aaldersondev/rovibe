# Essaim

Application desktop pour développer des jeux Roblox avec plusieurs agents de code en parallèle (Claude Code, Codex) sur un même projet.

- **Sessions multiples** : chaque agent tourne dans son propre terminal, côte à côte, dans le dossier du projet.
- **Essaim Sync** : fork de [Rojo](https://github.com/rojo-rbx/rojo) (`vendor/sync`). Le code vit dans des fichiers, Studio le reflète en direct.
- **Pont Studio** : le plugin garde un WebSocket ouvert vers l'app. Un appel d'outil fait un seul aller-retour local, sans polling.
- **MCP `essaim`** : `run_luau` (edit, serveur et client), `get_tree`, `search`, `get_instance`, `get_console`, `check_code`, `publish`, `playtest`, `play_move`, `play_input`, `screenshot`, `asset_search`, `asset_insert`, `asset_save`, `sync_connect`, `studio_status`. Chaque session d'agent le reçoit automatiquement.
- **Banque d'assets** : modèles enregistrés depuis Studio en `.rbxm`, chacun avec un aperçu photographié à l'enregistrement, dans `Documents\Essaim\Banque`, réutilisables d'un projet à l'autre, plus la recherche dans le Creator Store Roblox (assets gratuits). Les scripts d'un asset du Store sont désactivés à l'insertion.
- **Coordination entre agents** : un fichier modifié par un agent lui est réservé 10 minutes ; un autre agent Claude Code qui tente de l'éditer est refusé avec le nom de celui qui le tient. Les agents peuvent aussi réserver à l'avance (`claim_files`) et voir qui fait quoi (`agents_status`).
- **État des agents** : chaque panneau indique si l'agent travaille, attend une réponse ou a fini ; la barre des tâches clignote quand l'un d'eux attend.
- **Sessions persistantes** : les sessions Claude Code ouvertes à la fermeture de l'app reprennent leur conversation au lancement suivant.
- **Consignes** : une consigne s'envoie à plusieurs agents à la fois ; les consignes fréquentes s'enregistrent dans le projet (`.essaim/consignes.json`).
- **Import d'un jeu existant** : « Nouveau projet » peut partir des scripts de la place ouverte dans Studio. Seuls les scripts deviennent des fichiers ; la map et les interfaces restent dans la place et la synchro n'y touche pas.
- **Historique** : chaque projet est un dépôt git. Un point de sauvegarde est créé avant toute session « Sans confirmations », et « Historique » permet de revenir à un état antérieur sans rien perdre.
- **Outils mis à jour sans redémarrer Studio** : le plugin ne contient que le transport ; l'app lui envoie le code des outils (`crates/core/plugin/methods.luau`) à chaque connexion.
- **Jouer le test** : pendant un test Play, l'agent déplace le personnage (pathfinding) et envoie de vraies touches et de vrais clics, y compris sur un élément d'interface désigné par son chemin.
- **Capture d'écran** : l'app photographie la vue 3D de Studio, même en arrière-plan, et peut d'abord cadrer une instance.
- **Vérification du code** : `check_code` lance selene et luau-lsp (`scripts\get-tools.ps1` les installe dans `dist\Essaim\tools`). Après chaque fichier écrit par un agent Claude Code, les erreurs selene lui sont renvoyées aussitôt.
- **Publication** : l'outil `publish` et le bouton « Publier » envoient à Studio son raccourci de publication (Alt+P). Un agent ne peut publier qu'après accord dans l'app.

## Construire

Prérequis : Rust, Node 22, les Build Tools Visual Studio.

```powershell
.\scripts\build.ps1            # app portable dans dist\Essaim
.\scripts\release.ps1 0.3.0    # installeur signé + latest.json dans dist\release\0.3.0
```

L'installeur (NSIS, par utilisateur, sans droits administrateur) place l'app dans `%LOCALAPPDATA%\Essaim` ; les réglages sont dans `%APPDATA%\Essaim`.

### Mises à jour

Au démarrage, l'app lit `latest.json` sur la dernière release GitHub du dépôt indiqué dans `app/tauri.conf.json`. Si une version plus récente existe, un bandeau propose de l'installer ; l'app se relance et les sessions Claude Code reprennent. Chaque installeur est signé avec la clé `%USERPROFILE%\.tauri\essaim.key` : sans elle, aucune mise à jour n'est acceptée par les copies installées. Ne la mets pas dans le dépôt, et sauvegarde-la.

### Isolation

`.\scripts\setup-isolation.ps1` crée une fois la distribution WSL « essaim ». Une session Claude Code lancée avec « Isolé » y tourne : seul le dossier du projet y est monté, aucun disque Windows n'est visible, les programmes Windows ne peuvent pas être lancés et l'utilisateur de l'agent ne peut pas devenir root. L'accès au réseau n'est pas restreint. Codex, lui, utilise son propre bac à sable (`--sandbox workspace-write`).

## Utiliser

1. Lance `Essaim.exe`, puis « Installer le plugin Studio » et redémarre Roblox Studio.
2. « Nouveau projet » : crée un dossier modèle, ou reprend un dossier qui contient déjà un `default.project.json`.
3. Ouvre la place dans Studio, puis « Connecter Studio ». La première synchro demande une confirmation dans Studio.
4. Lance des agents avec « + Claude Code » ou « + Codex ».

`claude` et `codex` doivent être dans le PATH. L'app n'inclut aucun modèle : chaque agent utilise ton propre abonnement.

## Architecture

| Dossier | Rôle |
|---|---|
| `crates/core` | Serveur local (Rust, axum) : terminaux ConPTY, pont Studio, MCP HTTP, API et UI embarquée |
| `app` | Coque Tauri : une fenêtre native sur le serveur local |
| `ui` | Interface (TypeScript, xterm.js) |
| `vendor/sync` | Fork de Rojo ; le transport du pont est dans `plugin/src/Bridge.lua` |
| `crates/core/plugin` | Côté Studio de chaque outil, envoyé au plugin par l'app |

Tout écoute sur `127.0.0.1` uniquement. Port 34880 pour l'app, 34873 et suivants pour la synchro de chaque projet. L'API et le MCP exigent un jeton ; le point d'entrée du plugin refuse les navigateurs.

Développement sans fenêtre native : `cargo run -p essaim-core`, puis ouvre `http://127.0.0.1:34880`.

## Licences

Le code d'Essaim est sous licence MIT. `vendor/sync` reste sous MPL-2.0 : ses fichiers modifiés doivent être redistribués avec leurs sources.
