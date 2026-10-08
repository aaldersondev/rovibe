//! MCP server over Streamable HTTP. Every tool call is a single POST answered
//! with plain JSON; Studio is reached through the already-open WebSocket, so
//! a call costs one local round trip and no polling.

use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use base64::Engine;

use crate::{
    agents, assets, input, lint, screenshot,
    state::{Approval, Project, Shared},
    studio, sync,
};

const DEFAULT_PROTOCOL: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Pilote Roblox Studio pour le projet courant. Le code du jeu se modifie dans les fichiers du projet (synchronisés vers Studio), pas via ces outils : ils servent à inspecter la place, exécuter du Luau, lire la console et lancer des playtests.";

fn tools() -> Value {
    let context = json!({
        "type": "string",
        "enum": ["edit", "server", "client"],
        "description": "DataModel visé. `server` et `client` n'existent que pendant un playtest. Défaut : edit."
    });

    json!([
        {
            "name": "studio_status",
            "description": "Liste les Roblox Studio connectés (place, contexte) et l'état de la synchro du projet.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "run_luau",
            "description": "Exécute du Luau dans Studio. Renvoie les print/warn du code et ses valeurs de retour. En contexte edit, la modification est annulable avec Ctrl+Z.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "context": context,
                    "timeout": { "type": "number", "description": "Secondes, défaut 30." }
                },
                "required": ["code"]
            }
        },
        {
            "name": "get_tree",
            "description": "Arborescence de la place en texte indenté : `Nom [Classe]`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Ex. `game.Workspace.Map`. Défaut : les services principaux." },
                    "depth": { "type": "integer", "description": "Défaut 2." },
                    "max": { "type": "integer", "description": "Nombre maximal d'instances, défaut 300." },
                    "context": context
                }
            }
        },
        {
            "name": "search",
            "description": "Cherche des instances par nom (sous-chaîne, insensible à la casse) et/ou par classe (IsA).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "class": { "type": "string" },
                    "root": { "type": "string" },
                    "limit": { "type": "integer", "description": "Défaut 50." },
                    "context": context
                }
            }
        },
        {
            "name": "get_instance",
            "description": "Détail d'une instance : classe, propriétés courantes, attributs, tags, enfants. `source: true` ajoute le code d'un script.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "source": { "type": "boolean" },
                    "context": context
                },
                "required": ["path"]
            }
        },
        {
            "name": "get_console",
            "description": "Sortie de la console Studio, déjà reçue par l'app (réponse immédiate). Passe le curseur renvoyé dans `since` pour ne lire que les nouvelles lignes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "since": { "type": "integer" },
                    "limit": { "type": "integer", "description": "Défaut 100, les plus récentes." },
                    "level": { "type": "string", "enum": ["output", "info", "warn", "error"] },
                    "contains": { "type": "string" }
                }
            }
        },
        {
            "name": "playtest",
            "description": "Démarre ou arrête un test dans Studio. Au démarrage, attend que le serveur (et le client en mode play) soient prêts avant de répondre.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "stop"] },
                    "mode": { "type": "string", "enum": ["play", "run"], "description": "play = avec un joueur (défaut), run = serveur seul." }
                },
                "required": ["action"]
            }
        },
        {
            "name": "check_code",
            "description": "Vérifie le code du projet sans lancer le jeu : selene (noms indéfinis, erreurs de syntaxe, variables inutilisées) et luau-lsp (erreurs de type, avec l'API Roblox et l'arborescence du projet). À lancer après une série de modifications, avant un playtest.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Fichiers ou dossiers relatifs au projet. Défaut : `src`." },
                    "warnings": { "type": "boolean", "description": "Lister aussi les avertissements. Par défaut seules les erreurs sont détaillées, les avertissements sont comptés par type." }
                }
            }
        },
        {
            "name": "publish",
            "description": "Publie la place ouverte dans Studio sur Roblox, c'est-à-dire la met en ligne pour les joueurs. L'utilisateur doit d'abord accepter la demande dans l'app : n'appelle cet outil que s'il te l'a demandé.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "agents_status",
            "description": "Montre les autres agents qui travaillent sur ce projet : ce que chacun fait, les fichiers qu'il a réservés ou modifiés récemment. À consulter avant de commencer une tâche et quand une modification est refusée.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "claim_files",
            "description": "Réserve des fichiers ou des dossiers du projet avant d'y travailler, pour que les autres agents n'y touchent pas. Tout ou rien : si une partie est déjà prise, rien n'est réservé. Les fichiers que tu modifies sont de toute façon réservés pour toi pendant 10 minutes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Chemins relatifs au projet, ex. `src/server/Shop` ou `src/shared/Config.luau`." },
                    "note": { "type": "string", "description": "Ce que tu y fais, visible par les autres agents." }
                },
                "required": ["paths"]
            }
        },
        {
            "name": "release_files",
            "description": "Libère tes réservations quand ta tâche est finie. Sans `paths`, libère tout.",
            "inputSchema": {
                "type": "object",
                "properties": { "paths": { "type": "array", "items": { "type": "string" } } }
            }
        },
        {
            "name": "play_move",
            "description": "Pendant un test Play, fait marcher le personnage du joueur jusqu'à une position ou une instance, avec pathfinding et sauts. Ne prend pas le focus de la fenêtre.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "description": "`[x, y, z]` ou le chemin d'une instance, ex. `game.Workspace.Checkpoint1`." },
                    "timeout": { "type": "number", "description": "Secondes, défaut 20." }
                },
                "required": ["to"]
            }
        },
        {
            "name": "play_input",
            "description": "Pendant un test Play, envoie de vraies entrées clavier et souris au jeu : touches, clics dans le viewport, clics sur un élément d'interface. Met Roblox Studio au premier plan le temps de la séquence puis rend la main à la fenêtre précédente : l'utilisateur ne doit pas taper au clavier pendant ce temps.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "description": "Étapes jouées dans l'ordre (30 au plus). Chaque étape a un seul de ces champs : `key`, `gui`, `click` ou `wait`.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": { "description": "Une touche ou une liste de touches tenues ensemble : lettres, chiffres, Space, Enter, Tab, Escape, Shift, Ctrl, Left/Right/Up/Down, F1 à F12." },
                                "hold": { "type": "integer", "description": "Durée d'appui en ms pour `key`, défaut 80, maximum 5000." },
                                "gui": { "type": "string", "description": "Chemin d'un GuiObject à cliquer en son centre, ex. `game.Players.LocalPlayer.PlayerGui.Menu.Jouer`." },
                                "click": { "type": "array", "items": { "type": "integer" }, "description": "[x, y] en pixels du viewport." },
                                "wait": { "type": "integer", "description": "Pause en ms, maximum 5000." }
                            }
                        }
                    }
                },
                "required": ["steps"]
            }
        },
        {
            "name": "screenshot",
            "description": "Capture la vue 3D de Roblox Studio (le jeu pendant un test) et renvoie l'image. Fonctionne même si Studio est derrière d'autres fenêtres ; s'il est réduit, il est restauré en arrière-plan.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "focus": { "type": "string", "description": "Chemin d'un Model ou d'une Part à cadrer dans le viewport avant la capture, ex. `game.Workspace.Map`." },
                    "area": { "type": "string", "enum": ["viewport", "window"], "description": "viewport = la vue 3D seule (défaut) ; window = toute la fenêtre Studio, avec l'explorateur et les panneaux." },
                    "max_width": { "type": "integer", "description": "Largeur maximale en pixels, défaut 1280." }
                }
            }
        },
        {
            "name": "asset_search",
            "description": "Cherche des assets dans la banque locale (modèles enregistrés depuis Studio) et dans le Creator Store Roblox (gratuits uniquement). Renvoie des identifiants à passer à asset_insert : `bank:<id>` ou un numéro du Store.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Mots-clés. Vide = toute la banque locale." },
                    "collection": { "type": "string", "description": "Limite la banque locale à une collection." },
                    "source": { "type": "string", "enum": ["all", "bank", "store"], "description": "Défaut all." },
                    "type": { "type": "string", "enum": ["model", "audio", "decal", "mesh"], "description": "Type cherché dans le Store, défaut model." },
                    "limit": { "type": "integer", "description": "Défaut 10, maximum 30." }
                }
            }
        },
        {
            "name": "asset_insert",
            "description": "Insère un asset dans la place. Les scripts d'un asset du Store sont désactivés à l'insertion, sauf `keep_scripts: true` : un modèle gratuit peut contenir une porte dérobée, lis ses scripts avant de les activer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "asset": { "type": "string", "description": "`bank:<id>` ou l'identifiant numérique d'un asset du Store." },
                    "type": { "type": "string", "enum": ["model", "audio", "decal", "mesh"], "description": "Type de l'asset du Store, défaut model." },
                    "parent": { "type": "string", "description": "Défaut game.Workspace." },
                    "name": { "type": "string" },
                    "position": { "type": "array", "items": { "type": "number" }, "description": "[x, y, z] du pivot." },
                    "keep_scripts": { "type": "boolean" }
                },
                "required": ["asset"]
            }
        },
        {
            "name": "asset_save",
            "description": "Enregistre une instance de la place (avec ses descendants) dans la banque locale, pour la réutiliser dans d'autres projets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Ex. `game.Workspace.Arbre`." },
                    "name": { "type": "string", "description": "Défaut : le nom de l'instance." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "collection": { "type": "string", "description": "Collection où ranger l'asset, ex. `Nature`. Créée si elle n'existe pas." }
                },
                "required": ["path"]
            }
        },
        {
            "name": "asset_preview",
            "description": "Renvoie l'image d'aperçu d'un asset, pour juger de son apparence avant de l'insérer. Pour un asset de la banque sans aperçu, l'image est prise dans Studio.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "asset": { "type": "string", "description": "`bank:<id>` ou l'identifiant numérique d'un asset du Store." }
                },
                "required": ["asset"]
            }
        },
        {
            "name": "sync_connect",
            "description": "Démarre le serveur de synchro du projet si besoin et y connecte Studio.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn as_text(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => serde_json::to_string_pretty(&other).unwrap_or_default(),
    }
}

async fn forward(
    state: &Shared,
    project: Option<&Project>,
    method: &str,
    args: &Value,
) -> Result<String, String> {
    let context = args["context"].as_str().unwrap_or("edit");
    let studio = studio::pick(state, project, context)?;
    studio
        .call(method, args.clone(), Duration::from_secs(30))
        .await
        .map(as_text)
}

async fn run_luau(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let context = args["context"].as_str().unwrap_or("edit");
    let timeout = args["timeout"].as_f64().unwrap_or(30.0).clamp(1.0, 600.0);
    let studio = studio::pick(state, project, context)?;

    // The plugin enforces the timeout itself; the margin only covers a Studio
    // that stops answering altogether.
    let result = studio
        .call(
            "run_luau",
            json!({ "code": args["code"], "timeout": timeout }),
            Duration::from_secs_f64(timeout + 5.0),
        )
        .await?;

    let mut text = result["output"].as_str().unwrap_or_default().to_owned();
    if result["ok"] == true {
        if let Some(returns) = result["returns"].as_array().filter(|list| !list.is_empty()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("=> ");
            text.push_str(&serde_json::to_string(returns).unwrap_or_default());
        }
        if text.is_empty() {
            text.push_str("(aucune sortie)");
        }
        Ok(text)
    } else {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(result["error"].as_str().unwrap_or("erreur inconnue"));
        Err(text)
    }
}

fn get_console(state: &Shared, project: Option<&Project>, args: &Value) -> String {
    let since = args["since"].as_u64().unwrap_or(0);
    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    let level = args["level"].as_str();
    let contains = args["contains"].as_str().map(str::to_lowercase);
    let place = project.and_then(|project| project.place_id);

    let logs = state.logs.lock().unwrap();
    let matching: Vec<_> = logs
        .entries
        .iter()
        .filter(|entry| entry.seq > since)
        .filter(|entry| place.is_none_or(|place| entry.place_id == place))
        .filter(|entry| level.is_none_or(|level| entry.level == level))
        .filter(|entry| {
            contains
                .as_ref()
                .is_none_or(|needle| entry.message.to_lowercase().contains(needle))
        })
        .collect();

    let skipped = matching.len().saturating_sub(limit);
    let mut text = String::new();
    if skipped > 0 {
        text.push_str(&format!("({skipped} lignes plus anciennes omises)\n"));
    }
    for entry in &matching[skipped..] {
        text.push_str(&format!(
            "[{}] {}: {}\n",
            entry.context, entry.level, entry.message
        ));
    }
    text.push_str(&format!("curseur: {}", logs.next_seq));
    text
}

fn studio_status(state: &Shared, project: Option<&Project>) -> String {
    let mut text = String::new();
    let studios = state.studios.lock().unwrap();
    if studios.is_empty() {
        text.push_str("Aucun Studio connecté.\n");
    }
    for studio in studios.values() {
        text.push_str(&format!(
            "Studio « {} » placeId={} contexte={}\n",
            studio.name, studio.place_id, studio.context
        ));
        if studio.context == "edit" {
            if let Some(title) = input::blocking_dialog(&studio.name) {
                text.push_str(&format!(
                    "  bloqué par {} : l'utilisateur doit y répondre dans Studio\n",
                    input::describe_dialog(&title)
                ));
            }
        }
    }

    if let Some(project) = project {
        let running = state.syncs.lock().unwrap().contains_key(&project.id);
        text.push_str(&format!(
            "Projet « {} » : synchro {} sur le port {}",
            project.name,
            if running { "active" } else { "arrêtée" },
            project.sync_port
        ));
    }
    text
}

pub async fn connect_sync(state: &Shared, project: &Project) -> Result<String, String> {
    let port = sync::start(state, project)?;
    state.notify();
    let studio = studio::pick(state, Some(project), "edit")?;

    // Syncing writes into the place: from here on the project belongs to
    // this one, and a second Studio opened later can't be mistaken for it.
    if !project.is_bound() {
        let mut projects = state.projects.lock().unwrap();
        if let Some(stored) = projects.iter_mut().find(|stored| stored.id == project.id) {
            if studio.place_id != 0 {
                stored.place_id = Some(studio.place_id);
            } else {
                stored.place_name = Some(studio.name.clone());
            }
        }
        drop(projects);
        let _ = state.save_projects();
    }
    studio
        .call(
            "sync_connect",
            json!({ "host": "localhost", "port": port }),
            Duration::from_secs(10),
        )
        .await?;
    Ok(format!(
        "Studio se connecte à la synchro sur le port {port}. La première synchro demande une confirmation dans Studio."
    ))
}

async fn asset_search(args: &Value) -> Result<String, String> {
    let query = args["query"].as_str().unwrap_or_default().trim();
    let source = args["source"].as_str().unwrap_or("all");
    let kind = args["type"].as_str().unwrap_or("model");
    let limit = (args["limit"].as_u64().unwrap_or(10) as usize).clamp(1, 30);

    let mut text = String::new();
    if source != "store" {
        text.push_str("Banque locale :\n");
        let found = assets::search_bank(query, args["collection"].as_str(), limit);
        if found.is_empty() {
            text.push_str("(aucun résultat)\n");
        }
        for asset in found {
            text.push_str(&format!(
                "bank:{} | {} | {} | {} instances | {} Ko{}{}\n",
                asset.id,
                asset.name,
                if asset.class.is_empty() { "?" } else { &asset.class },
                asset.instances,
                asset.bytes / 1024,
                if asset.collection.is_empty() {
                    String::new()
                } else {
                    format!(" | collection : {}", asset.collection)
                },
                if asset.tags.is_empty() {
                    String::new()
                } else {
                    format!(" | tags : {}", asset.tags.join(", "))
                },
            ));
        }
    }

    if source != "bank" && !query.is_empty() {
        text.push_str(&format!("\nCreator Store, {kind}, gratuits :\n"));
        let found = assets::search_store(query, kind, limit).await?;
        if found.is_empty() {
            text.push_str("(aucun résultat)\n");
        }
        for item in &found {
            text.push_str(&store_line(item, kind));
        }
        text.push_str("\nasset_preview montre l'image d'un résultat avant de l'insérer.\n");
    }
    Ok(text)
}

fn store_line(item: &assets::StoreItem, kind: &str) -> String {
    let mut line = format!(
        "{} | {} | par {}{}",
        item.id,
        item.name,
        item.creator,
        if item.verified { " (vérifié)" } else { "" },
    );
    if let Some((percent, count)) = item.votes {
        line.push_str(&format!(" | {percent} % positifs sur {count} votes"));
    }
    if kind == "model" {
        if let Some(triangles) = item.triangles {
            line.push_str(&format!(" | {triangles} triangles"));
        }
        line.push_str(if item.has_scripts { " | CONTIENT DES SCRIPTS" } else { " | sans script" });
    }
    line.push('\n');
    line
}

fn asset_ref(args: &Value) -> Result<String, String> {
    // Accept a bare number as well: models tend to pass store ids unquoted.
    match &args["asset"] {
        Value::String(text) => Ok(text.trim().to_owned()),
        Value::Number(number) => Ok(number.to_string()),
        _ => Err("`asset` est requis".into()),
    }
}

async fn asset_preview(state: &Shared, project: Option<&Project>, args: &Value) -> Result<Vec<Value>, String> {
    let asset = asset_ref(args)?;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);

    if let Some(id) = asset.strip_prefix("bank:") {
        let path = assets::thumb_path(id).map_err(|error| error.to_string())?;
        if !path.exists() {
            let studio = studio::pick(state, project, "edit")?;
            make_preview(&studio, id).await?;
            state.notify();
        }
        let image = std::fs::read(path).map_err(|error| error.to_string())?;
        return Ok(vec![
            json!({ "type": "image", "data": encode(&image), "mimeType": "image/jpeg" }),
            json!({ "type": "text", "text": format!("Aperçu de bank:{id}") }),
        ]);
    }

    let id: u64 = asset
        .parse()
        .map_err(|_| "`asset` doit valoir `bank:<id>` ou un identifiant numérique du Store")?;
    let image = assets::store_preview(id).await?;
    Ok(vec![
        json!({ "type": "image", "data": encode(&image), "mimeType": "image/png" }),
        json!({ "type": "text", "text": format!("Aperçu de l'asset {id} du Creator Store") }),
    ])
}

/// Photographs a bank asset that has no preview: it is laid out in the place
/// away from everything, pictured, then taken out again. What Studio says
/// about it (class, size) fills the gaps of a file imported by hand.
pub async fn make_preview(studio: &studio::Studio, id: &str) -> Result<(), String> {
    let data = assets::read(id).map_err(|error| format!("Asset de banque illisible : {error}"))?;
    let staged = studio
        .call(
            "asset_stage",
            json!({ "data": base64::engine::general_purpose::STANDARD.encode(data) }),
            Duration::from_secs(60),
        )
        .await?;
    let _ = assets::edit(
        id,
        assets::Edit {
            class: staged["class"].as_str().map(str::to_owned),
            instances: staged["count"].as_u64(),
            ..assets::Edit::default()
        },
    );

    let pictured = if staged["visible"] == true {
        thumbnail(studio, staged["path"].as_str().unwrap_or_default(), id).await
    } else {
        Err("cet asset n'a rien à montrer dans la vue 3D".to_owned())
    };
    let _ = studio.call("asset_unstage", json!({}), Duration::from_secs(10)).await;
    pictured
}

pub async fn asset_insert(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let asset = asset_ref(args)?;

    let mut params = json!({
        "parent": args["parent"],
        "name": args["name"],
        "position": args["position"],
    });

    if let Some(id) = asset.strip_prefix("bank:") {
        let data = assets::read(id).map_err(|error| format!("Asset de banque illisible : {error}"))?;
        params["data"] = json!(base64::engine::general_purpose::STANDARD.encode(data));
    } else {
        let id: u64 = asset
            .parse()
            .map_err(|_| "`asset` doit valoir `bank:<id>` ou un identifiant numérique du Store")?;
        params["assetId"] = json!(id);
        params["kind"] = json!(args["type"].as_str().unwrap_or("model"));
        params["disableScripts"] = json!(args["keep_scripts"] != true);
    }

    let studio = studio::pick(state, project, "edit")?;
    studio
        .call("asset_insert", params, Duration::from_secs(90))
        .await
        .map(as_text)
}

async fn asset_save(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let studio = studio::pick(state, project, "edit")?;
    let exported = studio
        .call("asset_export", json!({ "path": args["path"] }), Duration::from_secs(60))
        .await?;

    let data = base64::engine::general_purpose::STANDARD
        .decode(exported["data"].as_str().unwrap_or_default())
        .map_err(|error| format!("Export illisible : {error}"))?;
    let name = args["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .or(exported["name"].as_str())
        .unwrap_or("asset");
    let tags = args["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tag| tag.as_str().map(str::to_owned))
        .collect();
    let instances = exported["count"].as_u64().unwrap_or(0);

    let new = assets::NewAsset {
        name,
        tags,
        class: exported["class"].as_str().unwrap_or_default(),
        instances,
        collection: args["collection"].as_str().unwrap_or_default(),
    };
    let id = assets::save(new, &data).map_err(|error| error.to_string())?;

    let preview = match thumbnail(&studio, args["path"].as_str().unwrap_or_default(), &id).await {
        Ok(()) => ", avec aperçu",
        // The asset is saved either way; a preview is a convenience.
        Err(_) => ", sans aperçu",
    };
    state.notify();
    Ok(format!(
        "Enregistré dans la banque : bank:{id} ({instances} instances, {} Ko{preview})",
        data.len() / 1024
    ))
}

/// Photographs an instance for the bank, then puts the user's camera back
/// where it was.
async fn thumbnail(studio: &studio::Studio, path: &str, id: &str) -> Result<(), String> {
    let short = Duration::from_secs(10);
    let camera = studio.call("camera_get", json!({}), short).await?;
    let framed = studio.call("focus_camera", json!({ "path": path }), short).await;

    let shot = match framed {
        Ok(_) => {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let size = (
                camera["width"].as_i64().unwrap_or(0) as i32,
                camera["height"].as_i64().unwrap_or(0) as i32,
            );
            let place = studio.name.clone();
            tokio::task::spawn_blocking(move || screenshot::capture(&place, Some(size), 360))
                .await
                .map_err(|error| error.to_string())?
        }
        Err(error) => Err(error),
    };
    let _ = studio.call("camera_set", camera, short).await;

    let shot = shot?;
    // A picture of the whole window would show panels, not the asset.
    if !shot.cropped {
        return Err("vue 3D introuvable".into());
    }
    let target = assets::thumb_path(id).map_err(|error| error.to_string())?;
    std::fs::write(target, shot.jpeg).map_err(|error| error.to_string())
}

/// Sends Studio its own "Publish to Roblox" shortcut. A plugin has no way to
/// publish, so this goes through the same door as the user.
pub async fn publish(state: &Shared, project: &Project) -> Result<String, String> {
    if studio::pick(state, Some(project), "server").is_ok() {
        return Err("Un test est en cours : arrête-le avant de publier".into());
    }
    let studio = studio::pick(state, Some(project), "edit")?;
    let info = studio.call("place_info", json!({}), Duration::from_secs(10)).await?;
    let size = (
        info["width"].as_i64().unwrap_or(0) as i32,
        info["height"].as_i64().unwrap_or(0) as i32,
    );
    let place = studio.name.clone();
    let place_id = info["placeId"].as_u64().unwrap_or(0);
    // Roblox's own record of the place, read before and after: the only
    // proof of a publication that doesn't depend on reading Studio's screen.
    let before = place_updated(place_id).await;

    let keys = state.settings.lock().unwrap().publish_keys();
    let dialog = tokio::task::spawn_blocking(move || {
        let shortcut = [input::Step::Keys { keys, hold_ms: 80 }];
        input::run(&place, size, &shortcut)?;
        std::thread::sleep(Duration::from_millis(1200));
        input::dialog_open(&place)
    })
    .await
    .map_err(|error| error.to_string())??;

    let confirmed = if place_id != 0 && !dialog && before.is_some() {
        let mut seen = None;
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let now = place_updated(place_id).await;
            if now.is_some() && now != before {
                seen = now;
                break;
            }
        }
        Some(seen)
    } else {
        None
    };
    crate::log::info(format!(
        "Publication de « {} » (place {place_id}) demandée : dialogue={dialog}, confirmation={confirmed:?}",
        project.name
    ));

    Ok(match (place_id, dialog) {
        (id, false) if matches!(confirmed, Some(Some(_))) => format!(
            "Place {id} publiée : Roblox a enregistré la nouvelle version à {} (UTC).",
            confirmed.flatten().unwrap_or_default()
        ),
        (id, false) if matches!(confirmed, Some(None)) => format!(
            "Raccourci de publication envoyé pour la place {id}, mais Roblox n'a enregistré aucune nouvelle version en 45 s. Vérifie dans Studio : la publication a pu échouer ou attendre une réponse."
        ),
        (0, true) => "Cette place n'a jamais été publiée : Studio a ouvert sa fenêtre de publication, à terminer à la main (nom du jeu, créateur).".to_owned(),
        (0, false) => "Raccourci de publication envoyé, mais Studio n'a pas ouvert sa fenêtre de publication : publie cette place une première fois par Fichier > Publier sur Roblox.".to_owned(),
        (_, true) => "Raccourci de publication envoyé : Studio a ouvert une fenêtre qui attend une réponse, regarde-la.".to_owned(),
        (id, false) => format!("Publication de la place {id} lancée par le raccourci de Studio. Studio indique le résultat dans ses notifications ; l'app ne peut pas le lire."),
    })
}

/// When Roblox last recorded a new version of a place. A public figure, so
/// no key or login is involved.
async fn place_updated(place_id: u64) -> Option<String> {
    if place_id == 0 {
        return None;
    }
    let details: Value = reqwest::Client::new()
        .get(format!("https://economy.roblox.com/v2/assets/{place_id}/details"))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    details["Updated"].as_str().map(str::to_owned)
}

/// The `publish` tool: nothing happens until the user accepts in the app.
async fn request_publish(state: &Shared, project: &Project, session: Option<&str>) -> Result<String, String> {
    let requester = session
        .and_then(|id| state.sessions.lock().unwrap().get(id).map(|session| session.info.title.clone()))
        .unwrap_or_else(|| "Un agent".to_owned());

    let (answer, decision) = tokio::sync::oneshot::channel();
    let id = state.next_id();
    state.approvals.lock().unwrap().insert(
        id,
        Approval {
            project_id: project.id.clone(),
            requester: requester.clone(),
            request: "publier la place sur Roblox".to_owned(),
            answer,
        },
    );
    let _ = state.attention.send(requester);
    state.notify();

    let allowed = tokio::time::timeout(Duration::from_secs(180), decision).await;
    state.approvals.lock().unwrap().remove(&id);
    state.notify();

    match allowed {
        Ok(Ok(true)) => publish(state, project).await,
        Ok(_) => Err("L'utilisateur a refusé la publication.".into()),
        Err(_) => Err("L'utilisateur n'a pas répondu à la demande de publication en 3 minutes : rien n'a été publié.".into()),
    }
}

async fn take_screenshot(state: &Shared, project: Option<&Project>, args: &Value) -> Result<Vec<Value>, String> {
    // Without a connected Studio there is no place name to match, and the
    // capture falls back to the only Studio window if there is just one.
    let studio = studio::pick(state, project, "edit");
    // During a test the 3D view belongs to the client, and its layout (and
    // so its size) differs from the editor's.
    let viewer = studio::pick(state, project, "client").or_else(|_| studio.clone());

    if let Some(path) = args["focus"].as_str() {
        studio
            .clone()?
            .call("focus_camera", json!({ "path": path }), Duration::from_secs(10))
            .await?;
        // The camera move only shows once Studio has drawn another frame.
        tokio::time::sleep(Duration::from_millis(350)).await;
    }

    let mut viewport = None;
    if args["area"] != "window" {
        if let Ok(viewer) = &viewer {
            if let Ok(size) = viewer.call("viewport_info", json!({}), Duration::from_secs(10)).await {
                viewport = Some((
                    size["width"].as_i64().unwrap_or(0) as i32,
                    size["height"].as_i64().unwrap_or(0) as i32,
                ));
            }
        }
    }

    let place = studio.map(|studio| studio.name.clone()).unwrap_or_default();
    let max_width = (args["max_width"].as_u64().unwrap_or(1280) as u32).clamp(320, 3840);

    let capture = tokio::task::spawn_blocking(move || screenshot::capture(&place, viewport, max_width))
        .await
        .map_err(|error| error.to_string())??;

    let what = if capture.cropped { "Vue 3D de Studio" } else { "Fenêtre Studio entière" };
    Ok(vec![
        json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&capture.jpeg), "mimeType": "image/jpeg" }),
        json!({ "type": "text", "text": format!("{what}, {}x{} px", capture.width, capture.height) }),
    ])
}

/// Waits for the DataModels of a test that was just started, so the caller
/// can use them right away instead of polling studio_status.
async fn wait_for_test(state: &Shared, project: Option<&Project>, with_client: bool) -> String {
    let ready = |context: &str| studio::pick(state, project, context).is_ok();

    for _ in 0..180 {
        if ready("server") && (!with_client || ready("client")) {
            return if with_client {
                "Test démarré : serveur et client prêts.".to_owned()
            } else {
                "Test démarré : serveur prêt (mode run, sans joueur).".to_owned()
            };
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    "Test lancé, mais ses DataModels ne se sont pas connectés en 45 s : vérifie avec studio_status.".to_owned()
}

async fn play_input(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let client = studio::pick(state, project, "client")
        .map_err(|_| "play_input demande un test Play en cours (outil playtest, mode play)".to_owned())?;

    let listed = args["steps"].as_array().ok_or("`steps` est requis")?;
    if listed.is_empty() || listed.len() > 30 {
        return Err("`steps` doit contenir entre 1 et 30 étapes".into());
    }

    let mut steps = Vec::new();
    for step in listed {
        if !step["key"].is_null() {
            let keys: Vec<String> = match &step["key"] {
                Value::String(key) => vec![key.clone()],
                Value::Array(keys) => keys.iter().filter_map(|key| key.as_str().map(str::to_owned)).collect(),
                _ => return Err("`key` attend une touche ou une liste de touches".into()),
            };
            steps.push(input::Step::Keys {
                keys,
                hold_ms: step["hold"].as_u64().unwrap_or(80).min(5000),
            });
        } else if let Some(path) = step["gui"].as_str() {
            // Asked now rather than up front: an earlier step may be what
            // makes this element appear.
            let center = client
                .call("gui_center", json!({ "path": path }), Duration::from_secs(10))
                .await?;
            steps.push(input::Step::Click {
                x: center["x"].as_i64().unwrap_or(0) as i32,
                y: center["y"].as_i64().unwrap_or(0) as i32,
            });
        } else if let Some(point) = step["click"].as_array() {
            steps.push(input::Step::Click {
                x: point.first().and_then(Value::as_i64).unwrap_or(0) as i32,
                y: point.get(1).and_then(Value::as_i64).unwrap_or(0) as i32,
            });
        } else if let Some(ms) = step["wait"].as_u64() {
            steps.push(input::Step::Wait(ms.min(5000)));
        } else {
            return Err("Chaque étape doit avoir `key`, `gui`, `click` ou `wait`".into());
        }
    }

    let viewport = client
        .call("viewport_info", json!({}), Duration::from_secs(10))
        .await?;
    let size = (
        viewport["width"].as_i64().unwrap_or(0) as i32,
        viewport["height"].as_i64().unwrap_or(0) as i32,
    );
    // The window title carries the place name of the edit DataModel.
    let place = studio::pick(state, project, "edit")
        .map(|studio| studio.name.clone())
        .unwrap_or_default();
    let count = steps.len();

    tokio::task::spawn_blocking(move || input::run(&place, size, &steps))
        .await
        .map_err(|error| error.to_string())??;
    Ok(format!("{count} étape(s) jouée(s) dans le jeu."))
}

async fn call_tool(
    state: &Shared,
    project: Option<&Project>,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    match name {
        "studio_status" => Ok(studio_status(state, project)),
        "run_luau" => run_luau(state, project, args).await,
        "get_tree" | "search" | "get_instance" => forward(state, project, name, args).await,
        "get_console" => Ok(get_console(state, project, args)),
        "playtest" => {
            let stopping = args["action"] == "stop";
            // Only the server DataModel is allowed to end a running test.
            let context = if stopping { "server" } else { "edit" };
            let studio = studio::pick(state, project, context)?;
            if !stopping {
                // Studio queues the test behind an open dialog and the tool
                // would then wait for DataModels that never come.
                if let Some(title) = input::blocking_dialog(&studio.name) {
                    return Err(format!(
                        "Studio est bloqué par {} : l'utilisateur doit y répondre avant de lancer un test.",
                        input::describe_dialog(&title)
                    ));
                }
            }
            let reply = studio
                .call("playtest", args.clone(), Duration::from_secs(15))
                .await
                .map(as_text)?;
            if stopping {
                Ok(reply)
            } else {
                Ok(wait_for_test(state, project, args["mode"] != "run").await)
            }
        }
        "play_move" => {
            let client = studio::pick(state, project, "client")
                .map_err(|_| "play_move demande un test Play en cours (outil playtest, mode play)".to_owned())?;
            let timeout = args["timeout"].as_f64().unwrap_or(20.0).clamp(1.0, 120.0);
            client
                .call(
                    "play_move",
                    json!({ "to": args["to"], "timeout": timeout }),
                    Duration::from_secs_f64(timeout + 10.0),
                )
                .await
                .map(as_text)
        }
        "play_input" => play_input(state, project, args).await,
        "asset_search" => asset_search(args).await,
        "asset_insert" => asset_insert(state, project, args).await,
        "asset_save" => asset_save(state, project, args).await,
        "sync_connect" => {
            let project = project.ok_or("Cette session n'est rattachée à aucun projet")?;
            connect_sync(state, project).await
        }
        "check_code" => {
            let project = project.ok_or("Cette session n'est rattachée à aucun projet")?;
            let paths: Vec<String> = args["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect();
            lint::check(state, project, &paths, args["warnings"] == true).await
        }
        other => Err(format!("Outil inconnu : {other}")),
    }
}

fn coordinate(
    state: &Shared,
    project: Option<&Project>,
    session: Option<&str>,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let project = project.ok_or("Cette session n'est rattachée à aucun projet")?;
    if name == "agents_status" {
        return Ok(agents::describe(state, &project.id, session));
    }

    let session = session.ok_or("Les réservations demandent une session lancée depuis RoVibe")?;
    match name {
        "claim_files" => agents::claim(state, project, session, args),
        _ => Ok(agents::release(state, project, session, args)),
    }
}

pub async fn handler(
    State(state): State<Shared>,
    Path((token, project_id)): Path<(String, String)>,
    Json(request): Json<Value>,
) -> Response {
    serve(state, token, project_id, None, request).await
}

/// Same server, reached through the address of one agent session, which is
/// what lets coordination tools know who is calling.
pub async fn session_handler(
    State(state): State<Shared>,
    Path((token, project_id, session_id)): Path<(String, String, String)>,
    Json(request): Json<Value>,
) -> Response {
    serve(state, token, project_id, Some(session_id), request).await
}

async fn serve(
    state: Shared,
    token: String,
    project_id: String,
    session: Option<String>,
    request: Value,
) -> Response {
    if token != state.token {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let id = request["id"].clone();
    let method = request["method"].as_str().unwrap_or_default();

    // Notifications carry no id and expect no body.
    if id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }

    let result = match method {
        "initialize" => json!({
            "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "rovibe-mcp", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let project = state.project(&project_id);
            let params = &request["params"];
            let name = params["name"].as_str().unwrap_or_default();
            let arguments = &params["arguments"];
            let as_content = |text: String| vec![json!({ "type": "text", "text": text })];
            let outcome = match name {
                "screenshot" => take_screenshot(&state, project.as_ref(), arguments).await,
                "asset_preview" => asset_preview(&state, project.as_ref(), arguments).await,
                "publish" => match project.as_ref() {
                    Some(project) => request_publish(&state, project, session.as_deref())
                        .await
                        .map(as_content),
                    None => Err("Cette session n'est rattachée à aucun projet".to_owned()),
                },
                "agents_status" | "claim_files" | "release_files" => {
                    coordinate(&state, project.as_ref(), session.as_deref(), name, arguments)
                        .map(as_content)
                }
                _ => call_tool(&state, project.as_ref(), name, arguments)
                    .await
                    .map(as_content),
            };
            match outcome {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(text) => {
                    crate::log::warn(format!("Outil {name} : {text}"));
                    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
                }
            }
        }
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }))
            .into_response();
        }
    };

    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}
