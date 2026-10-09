# RoVibe

English · [Français](README.fr.md)

**Vibe Code Together in Roblox Studio.**

`Multi-Agent` · `MCP` · `Studio Sync`

**RoVibe — Multi-Agent AI Coding for Roblox Studio**

> Connect Claude Code, Codex, and other AI coding agents to a shared Roblox Studio project. Real-time file synchronization, MCP integration, and multi-agent coordination.

A desktop app for building Roblox games with several coding agents working side by side (Claude Code, Codex) on one project.

| Component | Role | In this repository |
|---|---|---|
| `rovibe` | The desktop app | `app/` (window) and `ui/` (interface), on top of `crates/core` |
| `rovibe-cli` | The server without a window, and `status` to query a running RoVibe | `crates/core/src/main.rs` |
| `rovibe-studio` | The Roblox Studio plugin (`RoVibeStudio.rbxm`) | `vendor/sync/plugin`, tools in `crates/core/plugin` |
| `rovibe-mcp` | The MCP server handed to every agent | `crates/core/src/mcp.rs` |
| `rovibe-sync` | The sync engine, based on Rojo | `vendor/sync` |

- **Several sessions**: each agent runs in its own terminal, side by side, in the project folder.
- **RoVibe Sync**: a fork of [Rojo](https://github.com/rojo-rbx/rojo) (`vendor/sync`). Code lives in files, Studio mirrors it live.
- **Studio bridge**: the plugin keeps a WebSocket open to the app. A tool call is one local round trip, with no polling.
- **MCP `rovibe`** (server `rovibe-mcp`): `run_luau` (edit, server and client), `get_tree`, `search`, `get_instance`, `get_console`, `check_code`, `publish`, `playtest`, `play_move`, `play_input`, `screenshot`, `asset_search`, `asset_preview`, `asset_insert`, `asset_save`, `sync_connect`, `studio_status`. Every agent session gets it automatically.
- **Asset bank**: models saved from Studio, each with a preview pictured when it is saved, in `Documents\RoVibe\Banque`, reusable from one project to the next and sorted in collections. A pack (a folder of `.rbxm` or `.rbxmx` files) is imported in one go, its sub-folders becoming collections; missing previews are then made in Studio. The Creator Store tab searches Roblox's free assets with their pictures and inserts them in Studio; an agent sees the same pictures with `asset_preview`. Scripts of a Store asset are disabled on insertion.
- **Coordination between agents**: a file an agent changed is reserved for it for 10 minutes; another agent, Claude Code or Codex, that tries to edit it is refused and told who holds it. The same goes for a shell command that would write to that file, and for those that rewrite the whole folder (`git reset --hard`, `git stash`, `git checkout .`) while another agent holds anything. Agents can also reserve ahead (`claim_files`) and see who is doing what (`agents_status`).
- **Own branch**: with this option, each new agent works in a copy of the project, on its own git branch. Several agents can then rewrite the same files; you read each one's work from its pane (files and diff), then merge it, and a merge that conflicts is undone, naming the files involved. Studio shows the project; an agent brings its branch into Studio with `sync_connect`.
- **Notifications**: Windows tells you when an agent needs you or is done, if the window isn't in front (configurable).
- **Layout**: panes are reordered by dragging their header, and shown all side by side or one at a time with tabs. Interrupted sessions wait in a strip of their own.
- **Protected project**: for a live game. Studio shows the changes before each sync instead of applying them, and an agent needs your approval in the app before connecting the sync, as it does to publish.
- **Background**: the agents run in a server apart from the window. Closing the window while sessions run offers to let them carry on; a window that crashes or is updated finds them again. The app stays by the clock; its icon, or starting it again, brings the window back. With no session running, closing quits.
- **Getting started**: a checklist of what RoVibe needs on the PC (Claude Code or Codex, git, the Studio plugin…), what is in place and how to get the rest. It opens on first launch.
- **Language**: the interface comes in French and English; it follows the system's language, or the one chosen in the settings. Server messages and tool descriptions remain in French.
- **Agent status**: each pane says whether the agent is working, waiting for an answer or done; the taskbar button flashes when one is waiting.
- **Persistent sessions**: Claude Code sessions open when the app closes are offered again at the next launch; “Resume” restarts the agent on its conversation.
- **Changes**: the list of what agents changed since your last review, committed or not, with the agent responsible and the diff; each file can be undone on its own, “Accept all” starts again from the current state.
- **Settings**: default model for Claude Code and Codex, folder for new projects, Studio's publish shortcut. The model can also be chosen per session.
- **Journal**: `%USERPROFILE%\.rovibe\rovibe.log`, readable from the app, records sessions, sync, Studio connections, failed tools and updates.
- **Blocked Studio**: a dialog open in Studio (for instance “Auto Recovery” after a crash) is reported in the app and by `studio_status`, and tests are not started while it is there.
- **Prompts**: one prompt goes to several agents at once; frequent prompts are saved in the project (`.rovibe/consignes.json`).
- **Importing an existing game**: “New project” can start from the scripts of the place open in Studio. Only scripts become files; the map and the interfaces stay in the place and the sync leaves them alone.
- **History**: each project is a git repository. A checkpoint is created before any “No confirmations” session, and “History” goes back to an earlier state without losing anything.
- **Tools updated without restarting Studio**: the plugin only holds the transport; the app sends it the tools' code (`crates/core/plugin/methods.luau`) on every connection.
- **Playing the test**: during a Play test, the agent moves the character (pathfinding) and sends real keys and clicks, including on an interface element named by its path. Right click, wheel, dragging and turning the camera too. All of it happens in the background: Studio doesn't come forward, your keyboard and mouse stay yours.
- **Screenshot**: the app pictures Studio's 3D view, even in the background, and can frame an instance first.
- **Code checking**: `check_code` runs selene and luau-lsp (`scripts\get-tools.ps1` installs them in `dist\RoVibe\tools`). After each file a Claude Code agent writes, selene's errors are sent straight back to it.
- **Publishing**: the `publish` tool and the “Publish” button send Studio its publish shortcut (Alt+P), without bringing it forward. An agent can only publish after approval in the app. The publication is then confirmed by reading the place's last-updated date back from Roblox.

## Install

Download the installer from the [latest release](https://github.com/aaldersondev/rovibe/releases/latest) (Windows 10/11, 64-bit). It is not signed: Windows shows “Unknown publisher”. The app then updates itself.

## Build

Requirements: Rust, Node 22, the Visual Studio Build Tools.

```powershell
.\scripts\build.ps1            # portable app in dist\RoVibe
.\scripts\release.ps1 0.7.0    # signed installer + latest.json in dist\release\0.7.0
```

The installer (NSIS, per user, no administrator rights) puts the app in `%LOCALAPPDATA%\RoVibe`; settings are in `%USERPROFILE%\.rovibe`.

### Updates

On start, the app reads `latest.json` from the latest GitHub release of the repository named in `app/tauri.conf.json`. If a newer version exists, a banner offers to install it. Only the window restarts: the server that runs the agents is a process of its own, started from a copy kept with the settings, so sessions carry on through the update. The new version takes over once they are closed, or at once on request. Each installer is signed with the key `%USERPROFILE%\.tauri\rovibe.key`: without it, installed copies accept no update. Keep it out of the repository, and back it up.

### Releases built by GitHub

Pushing a tag `v1.2.3` runs `.github/workflows/release.yml`: it builds the installer of that version and publishes it with `latest.json`. It needs the update key as the repository secret `TAURI_SIGNING_PRIVATE_KEY`; without it, the tag builds nothing. Run by hand from the Actions tab, the workflow only tries the build and keeps the installer as an artifact.

```powershell
Get-Content $env:USERPROFILE\.tauri\rovibe.key -Raw | gh secret set TAURI_SIGNING_PRIVATE_KEY --repo aaldersondev/rovibe
git tag v0.9.0; git push origin v0.9.0
```

### Windows signing (Authenticode)

The key above says nothing to Windows: without a code-signing certificate, the installer opens on “Unknown publisher” and SmartScreen asks for confirmation. With a certificate, `release.ps1` signs the app, the sync server and the installer (`scripts\sign.ps1`, which calls `signtool` from the Windows SDK) as soon as one of these variables is set:

```powershell
$env:ROVIBE_SIGN_THUMBPRINT = "<SHA-1 thumbprint>"   # certificate in the Windows store: USB token or cloud HSM
# or, for a certificate that is still a file:
$env:ROVIBE_SIGN_PFX = "C:\path\certificate.pfx"; $env:ROVIBE_SIGN_PFX_PASSWORD = "..."
.\scripts\release.ps1 0.7.0
```

A certificate is bought from an authority (Certum, Sectigo, DigiCert…) or rented through Azure Trusted Signing; an OV certificate only makes the SmartScreen warning go away once reputation is earned, an EV one at once. With no variable set, the release is built unsigned.

### Isolation

`.\scripts\setup-isolation.ps1` creates the WSL distribution “rovibe” once. A session started with “Isolated” runs there: only the project folder is mounted, no Windows drive is visible, Windows programs can't be started and the agent's user can't become root. Its network is closed: the agent's user only reaches the distribution's own loopback, where the relay to the app and a proxy are waiting; the proxy only opens HTTPS tunnels to the model's API (`anthropic.com`, `claude.ai`, `claude.com`, `openai.com`, `chatgpt.com`) and to the hosts added in the settings (for instance `github.com`). No direct access, no DNS; each refused host is noted once in the journal. The “Open” setting gives the whole internet back. Codex runs there too, through the same door; until it is installed in the distribution, an “Isolated” Codex session keeps its own sandbox (`--sandbox workspace-write`). Each agent signs in to its account once, in its terminal. If the distribution was created by an earlier version, run the script again.

## Use

1. Start `RoVibe.exe`; “Getting started” lists what is missing. Install the Studio plugin from there and restart Roblox Studio.
2. “New project”: creates a template folder, or takes over a folder that already holds a `default.project.json`.
3. Open the place in Studio, then “Connect”. The sync applies without confirmation unless the project is protected.
4. Start agents with “Claude Code” or “Codex”.

`claude` and `codex` must be in the PATH. The first time Codex runs in a project, it asks you to approve the hooks RoVibe put there (`.codex/hooks.json`): that is how it reports its status and honours the locks. On Windows as in the isolated distribution, those hooks call `rovibe-hook`, a small program the app puts on the PATH of its sessions. The app includes no model: each agent uses your own subscription.

## Tests

```powershell
cd ui; npm run build; cd ..     # the server embeds the interface
cargo test -p rovibe-core
cd ui; npx playwright install chromium; npm test   # the interface, in a real browser
```

Tests cover what can lose work: importing a place, file locks between agents, going back in git, merging an agent's branch, choosing the Studio to act on, reading the code checkers, and key codes. The interface tests start a server of their own, on another port and with an empty settings folder, and replay what once broke: terminals overflowing their pane, panes resizing each other, a refused setting blocking the next ones, French left in the English interface. Everything runs on every push (`.github/workflows/ci.yml`).

## Architecture

| Folder | Role |
|---|---|
| `crates/core` | Local server (Rust, axum): ConPTY terminals, Studio bridge, MCP over HTTP, API and embedded UI |
| `app` | Tauri shell: a native window on the local server |
| `ui` | Interface (TypeScript, xterm.js) |
| `vendor/sync` | Fork of Rojo; the bridge's transport is in `plugin/src/Bridge.lua` |
| `crates/core/plugin` | The Studio side of each tool, sent to the plugin by the app |

Everything listens on `127.0.0.1` only. Port 34880 for the app, 34873 and up for each project's sync. The API and the MCP require a token; the plugin's entry point refuses browsers.

Development without a native window: `cargo run -p rovibe-core`, then open `http://127.0.0.1:34880`.

## Licences

RoVibe's code is under the MIT licence. `vendor/sync` remains under MPL-2.0: its modified files must be redistributed with their sources.
