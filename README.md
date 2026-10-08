# RoVibe

**Vibe Code Together in Roblox Studio.**

`Multi-Agent` · `MCP` · `Studio Sync`

**RoVibe — Multi-Agent AI Coding for Roblox Studio**

> Connect Claude Code, Codex, and other AI coding agents to a shared Roblox Studio project. Real-time file synchronization, MCP integration, and multi-agent coordination.

Application desktop pour développer des jeux Roblox avec plusieurs agents de code en parallèle (Claude Code, Codex) sur un même projet.

| Composant | Rôle | Dans ce dépôt |
|---|---|---|
| `rovibe` | L'application desktop | `app/` (fenêtre) et `ui/` (interface), au-dessus de `crates/core` |
| `rovibe-cli` | Le serveur sans fenêtre, et `status` pour interroger un RoVibe lancé | `crates/core/src/main.rs` |
| `rovibe-studio` | Le plugin Roblox Studio (`RoVibeStudio.rbxm`) | `vendor/sync/plugin`, outils dans `crates/core/plugin` |
| `rovibe-mcp` | Le serveur MCP donné à chaque agent | `crates/core/src/mcp.rs` |
| `rovibe-sync` | Le moteur de synchronisation, basé sur Rojo | `vendor/sync` |

- **Sessions multiples** : chaque agent tourne dans son propre terminal, côte à côte, dans le dossier du projet.
- **RoVibe Sync** : fork de [Rojo](https://github.com/rojo-rbx/rojo) (`vendor/sync`). Le code vit dans des fichiers, Studio le reflète en direct.
- **Pont Studio** : le plugin garde un WebSocket ouvert vers l'app. Un appel d'outil fait un seul aller-retour local, sans polling.
- **MCP `rovibe`** (serveur `rovibe-mcp`) : `run_luau` (edit, serveur et client), `get_tree`, `search`, `get_instance`, `get_console`, `check_code`, `publish`, `playtest`, `play_move`, `play_input`, `screenshot`, `asset_search`, `asset_preview`, `asset_insert`, `asset_save`, `sync_connect`, `studio_status`. Chaque session d'agent le reçoit automatiquement.
- **Banque d'assets** : modèles enregistrés depuis Studio, chacun avec un aperçu photographié à l'enregistrement, dans `Documents\RoVibe\Banque`, réutilisables d'un projet à l'autre et rangés en collections. Un pack (un dossier de `.rbxm` ou `.rbxmx`) s'importe d'un coup, ses sous-dossiers devenant des collections ; les aperçus manquants se créent ensuite dans Studio. L'onglet Creator Store cherche les assets gratuits de Roblox avec leurs images et les insère dans Studio ; un agent voit ces mêmes images avec `asset_preview`. Les scripts d'un asset du Store sont désactivés à l'insertion.
- **Coordination entre agents** : un fichier modifié par un agent lui est réservé 10 minutes ; un autre agent, Claude Code ou Codex, qui tente de l'éditer est refusé avec le nom de celui qui le tient. Le refus vaut aussi pour une commande shell qui écrirait dans ce fichier, et pour celles qui réécrivent tout le dossier (`git reset --hard`, `git stash`, `git checkout .`) tant qu'un autre agent tient quelque chose. Les agents peuvent aussi réserver à l'avance (`claim_files`) et voir qui fait quoi (`agents_status`).
- **État des agents** : chaque panneau indique si l'agent travaille, attend une réponse ou a fini ; la barre des tâches clignote quand l'un d'eux attend.
- **Sessions persistantes** : les sessions Claude Code ouvertes à la fermeture de l'app sont proposées au lancement suivant ; « Reprendre » relance l'agent sur sa conversation.
- **Changements** : la liste de ce que les agents ont modifié depuis ta dernière relecture, commité ou non, avec l'agent en cause et le diff ; chaque fichier s'annule séparément, « Tout accepter » repart de l'état courant.
- **Réglages** : modèle par défaut de Claude Code et de Codex, dossier des nouveaux projets, raccourci de publication de Studio. Le modèle se choisit aussi session par session.
- **Journal** : `%APPDATA%\RoVibe\rovibe.log`, consultable depuis l'app, trace les sessions, la synchro, les connexions de Studio, les outils en erreur et les mises à jour.
- **Studio bloqué** : une boîte de dialogue ouverte dans Studio (par exemple « Auto Recovery » après une fermeture brutale) est signalée dans l'app et par `studio_status`, et les tests ne sont pas lancés tant qu'elle est là.
- **Consignes** : une consigne s'envoie à plusieurs agents à la fois ; les consignes fréquentes s'enregistrent dans le projet (`.rovibe/consignes.json`).
- **Import d'un jeu existant** : « Nouveau projet » peut partir des scripts de la place ouverte dans Studio. Seuls les scripts deviennent des fichiers ; la map et les interfaces restent dans la place et la synchro n'y touche pas.
- **Historique** : chaque projet est un dépôt git. Un point de sauvegarde est créé avant toute session « Sans confirmations », et « Historique » permet de revenir à un état antérieur sans rien perdre.
- **Outils mis à jour sans redémarrer Studio** : le plugin ne contient que le transport ; l'app lui envoie le code des outils (`crates/core/plugin/methods.luau`) à chaque connexion.
- **Jouer le test** : pendant un test Play, l'agent déplace le personnage (pathfinding) et envoie de vraies touches et de vrais clics, y compris sur un élément d'interface désigné par son chemin.
- **Capture d'écran** : l'app photographie la vue 3D de Studio, même en arrière-plan, et peut d'abord cadrer une instance.
- **Vérification du code** : `check_code` lance selene et luau-lsp (`scripts\get-tools.ps1` les installe dans `dist\RoVibe\tools`). Après chaque fichier écrit par un agent Claude Code, les erreurs selene lui sont renvoyées aussitôt.
- **Publication** : l'outil `publish` et le bouton « Publier » envoient à Studio son raccourci de publication (Alt+P). Un agent ne peut publier qu'après accord dans l'app. La publication est ensuite confirmée en relisant la date de dernière version de la place chez Roblox.

## Construire

Prérequis : Rust, Node 22, les Build Tools Visual Studio.

```powershell
.\scripts\build.ps1            # app portable dans dist\RoVibe
.\scripts\release.ps1 0.3.0    # installeur signé + latest.json dans dist\release\0.3.0
```

L'installeur (NSIS, par utilisateur, sans droits administrateur) place l'app dans `%LOCALAPPDATA%\RoVibe` ; les réglages sont dans `%APPDATA%\RoVibe`.

### Mises à jour

Au démarrage, l'app lit `latest.json` sur la dernière release GitHub du dépôt indiqué dans `app/tauri.conf.json`. Si une version plus récente existe, un bandeau propose de l'installer ; l'app se relance et les sessions Claude Code reprennent. Chaque installeur est signé avec la clé `%USERPROFILE%\.tauri\rovibe.key` : sans elle, aucune mise à jour n'est acceptée par les copies installées. Ne la mets pas dans le dépôt, et sauvegarde-la.

### Signature Windows (Authenticode)

La clé ci-dessus ne dit rien à Windows : sans certificat de signature de code, l'installeur s'ouvre sur « Éditeur inconnu » et SmartScreen demande une confirmation. Avec un certificat, `release.ps1` signe l'app, le serveur de synchro et l'installeur (`scripts\sign.ps1`, qui appelle `signtool` du SDK Windows) dès qu'une de ces variables est définie :

```powershell
$env:ROVIBE_SIGN_THUMBPRINT = "<empreinte SHA-1>"   # certificat du magasin Windows : jeton USB ou HSM cloud
# ou, pour un certificat encore en fichier :
$env:ROVIBE_SIGN_PFX = "C:\chemin\certificat.pfx"; $env:ROVIBE_SIGN_PFX_PASSWORD = "..."
.\scripts\release.ps1 0.3.0
```

Un certificat s'achète auprès d'une autorité (Certum, Sectigo, DigiCert…) ou se loue via Azure Trusted Signing ; un certificat OV ne fait disparaître l'avertissement SmartScreen qu'une fois la réputation acquise, un EV tout de suite. Sans variable, la release est construite non signée, comme aujourd'hui.

### Isolation

`.\scripts\setup-isolation.ps1` crée une fois la distribution WSL « rovibe ». Une session Claude Code lancée avec « Isolé » y tourne : seul le dossier du projet y est monté, aucun disque Windows n'est visible, les programmes Windows ne peuvent pas être lancés et l'utilisateur de l'agent ne peut pas devenir root. Son réseau est fermé : l'utilisateur de l'agent ne joint que la boucle locale de la distribution, où l'attendent le relais vers l'app et un proxy qui n'ouvre de tunnels HTTPS que vers l'API du modèle (`anthropic.com`, `claude.ai`, `claude.com`) et les hôtes ajoutés dans les réglages (par exemple `github.com`). Ni accès direct, ni DNS ; chaque hôte refusé est noté une fois dans le journal. Le réglage « Ouvert » rend tout internet. Si la distribution a été créée avant cette version, relance le script : il y installe `iptables`. Codex, lui, utilise son propre bac à sable (`--sandbox workspace-write`).

## Utiliser

1. Lance `RoVibe.exe`, puis « Installer le plugin Studio » et redémarre Roblox Studio.
2. « Nouveau projet » : crée un dossier modèle, ou reprend un dossier qui contient déjà un `default.project.json`.
3. Ouvre la place dans Studio, puis « Connecter Studio ». La synchro s'applique sans confirmation ; pour en redemander une, change « Confirmation Behavior » dans les réglages du plugin.
4. Lance des agents avec « + Claude Code » ou « + Codex ».

`claude` et `codex` doivent être dans le PATH. Au premier lancement de Codex dans un projet, Codex demande d'approuver les hooks qu'RoVibe y a déposés (`.codex/hooks.json`) : c'est par eux qu'il transmet son état et respecte les verrous. L'app n'inclut aucun modèle : chaque agent utilise ton propre abonnement.

## Tests

```powershell
cd ui; npm run build; cd ..     # le serveur embarque l'interface
cargo test -p rovibe-core
```

Les tests couvrent ce qui peut faire perdre du travail : l'import d'une place, les verrous de fichiers entre agents, le retour arrière git, le choix du Studio visé, la lecture des vérificateurs de code et les codes de touches. Ils tournent aussi sur chaque push (`.github/workflows/ci.yml`).

## Architecture

| Dossier | Rôle |
|---|---|
| `crates/core` | Serveur local (Rust, axum) : terminaux ConPTY, pont Studio, MCP HTTP, API et UI embarquée |
| `app` | Coque Tauri : une fenêtre native sur le serveur local |
| `ui` | Interface (TypeScript, xterm.js) |
| `vendor/sync` | Fork de Rojo ; le transport du pont est dans `plugin/src/Bridge.lua` |
| `crates/core/plugin` | Côté Studio de chaque outil, envoyé au plugin par l'app |

Tout écoute sur `127.0.0.1` uniquement. Port 34880 pour l'app, 34873 et suivants pour la synchro de chaque projet. L'API et le MCP exigent un jeton ; le point d'entrée du plugin refuse les navigateurs.

Développement sans fenêtre native : `cargo run -p rovibe-core`, puis ouvre `http://127.0.0.1:34880`.

## Licences

Le code d'RoVibe est sous licence MIT. `vendor/sync` reste sous MPL-2.0 : ses fichiers modifiés doivent être redistribués avec leurs sources.
