use std::sync::atomic::Ordering;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;

use std::time::Duration;

use crate::{agents, assets, git, isolation, mcp, projects, pty, state::Shared, sync};

const PLUGIN_FILE: &str = "RoVibeStudio.rbxm";

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn fail(message: impl Into<String>) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": message.into() })),
    )
}

fn plugins_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("Roblox/Plugins"))
}

fn plugin_installed() -> bool {
    plugins_dir().is_some_and(|dir| dir.join(PLUGIN_FILE).exists())
}

async fn write_plugin() -> Result<(), String> {
    let binary = sync::binary().ok_or("Serveur de synchro introuvable")?;
    let output = tokio::process::Command::new(binary)
        .args(["plugin", "install"])
        .output()
        .await
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    if let Some(dir) = plugins_dir() {
        crate::legacy::remove_old_plugin(&dir);
    }
    Ok(())
}

/// A plugin installed under the app's former name is swapped for the current
/// one. Studio keeps the one it has loaded until it restarts.
pub fn replace_old_plugin(state: &Shared) {
    if !plugins_dir().is_some_and(|dir| crate::legacy::old_plugin_installed(&dir)) {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        match write_plugin().await {
            Ok(()) => crate::log::info("Plugin Studio renommé en RoVibe Studio : redémarre Studio pour le charger"),
            Err(error) => crate::log::warn(format!("Plugin Studio : {error}")),
        }
        state.notify();
    });
}

pub async fn get_state(State(state): State<Shared>) -> Json<Value> {
    let syncs = state.syncs.lock().unwrap();
    let projects: Vec<Value> = state
        .projects
        .lock()
        .unwrap()
        .iter()
        .map(|project| {
            json!({
                "id": project.id,
                "name": project.name,
                "path": project.path,
                "sync_port": project.sync_port,
                "place_id": project.place_id,
                "place_name": project.place_name,
                "sync_running": syncs.contains_key(&project.id),
                "sync_log": syncs.get(&project.id).map(|running| {
                    let log = running.log.lock().unwrap();
                    log.iter().rev().take(8).rev().cloned().collect::<Vec<_>>()
                }),
            })
        })
        .collect();

    let mut sessions: Vec<Value> = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .map(|session| {
            let status = session.status.lock().unwrap();
            // An agent that hasn't reported in after a few seconds is not
            // loading: it is asking something before it starts, typically
            // whether the folder can be trusted, and only the terminal shows it.
            let stalled = status.state == "starting" && status.since.elapsed().as_secs() >= 8;
            let (shown, detail) = if stalled {
                ("waiting", "une question t'attend dans son terminal")
            } else {
                (status.state, status.detail.as_str())
            };
            json!({
                "id": session.info.id,
                "project_id": session.info.project_id,
                "kind": session.info.kind,
                "title": session.info.title,
                "exited": session.exited.load(Ordering::Relaxed),
                "status": shown,
                "detail": detail,
                "since": status.since.elapsed().as_secs(),
                "files": agents::files_of(&state, &session.info.id),
                "isolated": session.isolated,
            })
        })
        .collect();
    // Session ids are "s<n>" in creation order; the map has no order of its own.
    sessions.sort_by_key(|session| {
        session["id"].as_str().unwrap_or("s0")[1..]
            .parse::<u64>()
            .unwrap_or(0)
    });

    let studios: Vec<Value> = state
        .studios
        .lock()
        .unwrap()
        .values()
        .map(|studio| {
            let mut info = json!(studio.info());
            // Only the editor's window can be told apart by its title.
            if studio.context == "edit" {
                info["blocked"] = json!(crate::input::blocking_dialog(&studio.name)
                    .map(|title| crate::input::describe_dialog(&title)));
            }
            info
        })
        .collect();
    let dormant = state.dormant.lock().unwrap().clone();

    let approvals: Vec<Value> = state
        .approvals
        .lock()
        .unwrap()
        .iter()
        .map(|(id, approval)| {
            json!({
                "id": id,
                "project_id": approval.project_id,
                "requester": approval.requester,
                "request": approval.request,
            })
        })
        .collect();
    let (selene, luau_lsp) = crate::lint::available();

    Json(json!({
        "version": state.version,
        "approvals": approvals,
        "dormant": dormant,
        "update": *state.update.lock().unwrap(),
        "isolation": isolation::available(),
        "checkers": { "selene": selene, "luau_lsp": luau_lsp },
        "projects": projects,
        "sessions": sessions,
        "studios": studios,
        "tools": {
            "claude": which::which("claude").is_ok(),
            "codex": which::which("codex").is_ok(),
            "sync": sync::binary().is_some(),
        },
        "plugin_installed": plugin_installed(),
    }))
}

#[derive(Deserialize)]
pub struct NewProject {
    name: String,
    path: Option<String>,
    /// Build the project from the scripts of a place open in Studio: the id
    /// of that Studio connection, as listed in the state.
    import_studio: Option<u64>,
}

pub async fn create_project(State(state): State<Shared>, Json(body): Json<NewProject>) -> ApiResult {
    let export = if let Some(id) = body.import_studio {
        let studio = state
            .studios
            .lock()
            .unwrap()
            .get(&id)
            .filter(|studio| studio.context == "edit")
            .cloned()
            .ok_or_else(|| fail("Cette place n'est plus ouverte dans Studio"))?;
        let export = studio
            .call("export_scripts", json!({}), Duration::from_secs(120))
            .await
            .map_err(fail)?;
        Some(export)
    } else {
        None
    };

    let (project, report) =
        projects::create(&state, &body.name, body.path, export.as_ref()).map_err(fail)?;

    crate::log::info(format!("Projet « {} » créé dans {}", project.name, project.path.display()));
    let mut notes = Vec::new();
    if let Some(report) = report {
        notes.push(format!("{} scripts importés depuis Studio.", report.scripts));
        if !report.skipped.is_empty() {
            notes.push(format!(
                "{} scripts sont restés dans Studio : la liste est dans IMPORT.md.",
                report.skipped.len()
            ));
        }
    }
    if let Err(error) = git::ensure_repo(&project.path).await {
        crate::log::warn(format!("Historique git non créé pour « {} » : {error}", project.name));
        notes.push(format!("Historique git non créé : {error}"));
    }

    // A project nobody is syncing is useless, so the server starts with it.
    let _ = sync::start(&state, &project);
    state.notify();

    let mut reply = json!(project);
    if !notes.is_empty() {
        reply["message"] = json!(notes.join(" "));
    }
    Ok(Json(reply))
}

pub async fn project_history(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    Ok(Json(json!(git::history(&project.path).await)))
}

#[derive(Deserialize)]
pub struct GitAction {
    action: String,
    message: Option<String>,
    commit: Option<String>,
}

pub async fn project_git(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<GitAction>,
) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let message = match body.action.as_str() {
        "snapshot" => {
            let label = body
                .message
                .filter(|message| !message.trim().is_empty())
                .unwrap_or_else(|| "Point de sauvegarde".to_owned());
            if git::snapshot(&project.path, label.trim()).await.map_err(fail)? {
                "Point de sauvegarde créé.".to_owned()
            } else {
                "Rien n'a changé depuis le dernier point de sauvegarde.".to_owned()
            }
        }
        "restore" => {
            let commit = body.commit.ok_or_else(|| fail("`commit` est requis"))?;
            git::restore(&project.path, &commit).await.map_err(fail)?
        }
        _ => return Err(fail("Action inconnue")),
    };
    Ok(Json(json!({ "message": message })))
}

/// Forgets the project. The files on disk are left untouched.
pub async fn remove_project(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    if let Some(running) = state.syncs.lock().unwrap().get(&id) {
        running.stop();
    }
    for session in state.sessions.lock().unwrap().values() {
        if session.info.project_id == id {
            session.kill();
        }
    }
    state
        .sessions
        .lock()
        .unwrap()
        .retain(|_, session| session.info.project_id != id);
    state
        .projects
        .lock()
        .unwrap()
        .retain(|project| project.id != id);
    state.dormant.lock().unwrap().retain(|session| session.project_id != id);
    agents::save_sessions(&state);
    state.save_projects().map_err(|error| fail(error.to_string()))?;
    state.notify();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct SyncAction {
    action: String,
}

pub async fn project_sync(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<SyncAction>,
) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let message = match body.action.as_str() {
        "start" => format!("Synchro démarrée sur le port {}", sync::start(&state, &project).map_err(fail)?),
        "stop" => {
            if let Some(running) = state.syncs.lock().unwrap().get(&id) {
                running.stop();
            }
            "Synchro arrêtée".to_owned()
        }
        "connect" => mcp::connect_sync(&state, &project).await.map_err(fail)?,
        _ => return Err(fail("Action inconnue")),
    };
    state.notify();
    Ok(Json(json!({ "message": message })))
}

#[derive(Deserialize)]
pub struct Binding {
    place_id: Option<u64>,
    place_name: Option<String>,
}

pub async fn bind_project(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<Binding>,
) -> ApiResult {
    {
        let mut projects = state.projects.lock().unwrap();
        let project = projects
            .iter_mut()
            .find(|project| project.id == id)
            .ok_or_else(|| fail("Projet inconnu"))?;
        project.place_id = body.place_id.filter(|id| *id != 0);
        project.place_name = body.place_name.filter(|name| !name.is_empty());
    }
    state.save_projects().map_err(|error| fail(error.to_string()))?;
    state.notify();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct NewSession {
    project_id: String,
    kind: pty::Kind,
    #[serde(default)]
    skip_permissions: bool,
    #[serde(default)]
    isolated: bool,
    model: Option<String>,
}

pub async fn create_session(State(state): State<Shared>, Json(body): Json<NewSession>) -> ApiResult {
    let project = state
        .project(&body.project_id)
        .ok_or_else(|| fail("Projet inconnu"))?;
    projects::write_mcp_config(&state, &project.id).map_err(|error| fail(error.to_string()))?;

    // An agent that never asks before acting gets a commit to come back to.
    // A project without its own repository simply has no such net.
    if body.skip_permissions {
        let _ = git::snapshot(
            &project.path,
            "Point de sauvegarde avant une session sans confirmations",
        )
        .await;
    }

    let in_wsl = body.isolated && body.kind == pty::Kind::Claude;
    if in_wsl {
        isolation::prepare(&state, &project).await.map_err(fail)?;
    }
    let launch = pty::Launch {
        kind: body.kind,
        skip_permissions: body.skip_permissions,
        isolated: body.isolated && body.kind != pty::Kind::Shell,
        in_wsl,
        title: None,
        resume: None,
        model: body.model,
    };
    let info = pty::spawn(&state, &project, launch).map_err(fail)?;
    Ok(Json(json!(info)))
}

pub async fn remove_session(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    if let Some(session) = state.sessions.lock().unwrap().remove(&id) {
        session.kill();
    }
    // Closing a session is the user saying it shouldn't come back.
    agents::release_all(&state, &id);
    agents::save_sessions(&state);
    state.notify();
    Ok(Json(json!({})))
}

pub async fn install_plugin(State(state): State<Shared>) -> ApiResult {
    write_plugin().await.map_err(fail)?;
    state.notify();
    Ok(Json(json!({ "message": "Plugin installé. Redémarre Roblox Studio pour le charger." })))
}

fn prompts_path(project: &crate::state::Project) -> std::path::PathBuf {
    project.path.join(".rovibe").join("consignes.json")
}

/// Saved prompts live in the project, so they are versioned with it.
pub async fn get_prompts(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let prompts: Value = std::fs::read(prompts_path(&project))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!([]));
    Ok(Json(prompts))
}

#[derive(Deserialize, serde::Serialize)]
pub struct Prompt {
    name: String,
    text: String,
}

pub async fn put_prompts(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(prompts): Json<Vec<Prompt>>,
) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let path = prompts_path(&project);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| fail(error.to_string()))?;
    let json = serde_json::to_vec_pretty(&prompts).map_err(|error| fail(error.to_string()))?;
    std::fs::write(path, json).map_err(|error| fail(error.to_string()))?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct Decision {
    allow: bool,
}

/// The user's answer to something an agent asked permission for.
pub async fn answer_approval(
    State(state): State<Shared>,
    Path(id): Path<u64>,
    Json(body): Json<Decision>,
) -> ApiResult {
    let approval = state
        .approvals
        .lock()
        .unwrap()
        .remove(&id)
        .ok_or_else(|| fail("Cette demande n'est plus en attente"))?;
    let _ = approval.answer.send(body.allow);
    state.notify();
    Ok(Json(json!({})))
}

pub async fn get_settings(State(state): State<Shared>) -> Json<Value> {
    Json(json!(*state.settings.lock().unwrap()))
}

pub async fn put_settings(
    State(state): State<Shared>,
    Json(settings): Json<crate::settings::Settings>,
) -> ApiResult {
    crate::input::check_keys(&settings.publish_keys())
        .map_err(|error| fail(format!("Raccourci de publication : {error}")))?;
    let folder = settings.projects_dir.trim();
    if !folder.is_empty() && !std::path::Path::new(folder).is_absolute() {
        return Err(fail("Le dossier des projets doit être un chemin complet"));
    }
    settings.save(&state.data_dir).map_err(|error| fail(error.to_string()))?;
    *state.settings.lock().unwrap() = settings;
    isolation::refresh_network(&state);
    Ok(Json(json!({ "message": "Réglages enregistrés." })))
}

/// Who, among the agents of this run, changed a file.
fn authors(state: &Shared, project_id: &str, path: &str) -> Vec<String> {
    let mut titles: Vec<String> = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .filter(|session| session.info.project_id == project_id)
        .filter(|session| {
            session.touched.lock().unwrap().iter().any(|touched| touched.eq_ignore_ascii_case(path))
        })
        .map(|session| session.info.title.clone())
        .collect();
    titles.sort();
    titles
}

pub async fn project_changes(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let changes = git::changes(&project.path).await.map_err(fail)?;
    let listed: Vec<Value> = changes
        .iter()
        .map(|change| {
            let mut entry = json!(change);
            entry["agents"] = json!(authors(&state, &id, &change.path));
            entry
        })
        .collect();
    Ok(Json(json!({ "changes": listed })))
}

#[derive(Deserialize)]
pub struct DiffQuery {
    path: String,
}

pub async fn change_diff(
    State(state): State<Shared>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<DiffQuery>,
) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let diff = git::diff(&project.path, &query.path).await.map_err(fail)?;
    Ok(Json(json!({ "diff": diff })))
}

#[derive(Deserialize)]
pub struct Review {
    action: String,
    path: Option<String>,
}

pub async fn review_changes(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<Review>,
) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let message = match body.action.as_str() {
        "accept" => git::accept(&project.path).await.map_err(fail)?,
        "revert" => {
            let path = body.path.ok_or_else(|| fail("`path` est requis"))?;
            crate::log::info(format!("« {} » : {path} remis à son état relu", project.name));
            git::revert(&project.path, &path).await.map_err(fail)?
        }
        _ => return Err(fail("Action inconnue")),
    };
    Ok(Json(json!({ "message": message })))
}

pub async fn read_log() -> Json<Value> {
    Json(json!({ "path": crate::log::path(), "lines": crate::log::tail(300) }))
}

pub async fn resume_session(State(state): State<Shared>, Path(agent): Path<String>) -> ApiResult {
    let info = agents::resume(&state, &agent).await.map_err(fail)?;
    Ok(Json(json!(info)))
}

pub async fn forget_session(State(state): State<Shared>, Path(agent): Path<String>) -> ApiResult {
    agents::forget(&state, &agent);
    Ok(Json(json!({})))
}

pub async fn install_update(State(state): State<Shared>) -> ApiResult {
    if state.update.lock().unwrap().is_none() {
        return Err(fail("Aucune mise à jour n'est disponible"));
    }
    let _ = state.install_update.send(());
    Ok(Json(json!({ "message": "Téléchargement de la mise à jour : l'app va redémarrer." })))
}

/// Publishing asked for by the user from the app itself.
pub async fn publish_project(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    let project = state.project(&id).ok_or_else(|| fail("Projet inconnu"))?;
    let message = mcp::publish(&state, &project).await.map_err(fail)?;
    Ok(Json(json!({ "message": message })))
}

pub async fn asset_thumb(Path(id): Path<String>) -> Response {
    let image = assets::thumb_path(&id).ok().and_then(|path| std::fs::read(path).ok());
    match image {
        Some(bytes) => ([(axum::http::header::CONTENT_TYPE, "image/jpeg")], bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn list_assets() -> Json<Value> {
    Json(json!({ "dir": assets::bank_dir(), "assets": assets::list() }))
}

pub async fn edit_asset(
    State(state): State<Shared>,
    Path(id): Path<String>,
    Json(edit): Json<assets::Edit>,
) -> ApiResult {
    assets::edit(&id, edit).map_err(|error| fail(error.to_string()))?;
    state.notify();
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct ImportAssets {
    path: String,
    #[serde(default)]
    collection: String,
}

/// Copies a pack, a folder of model files, into the bank.
pub async fn import_assets(State(state): State<Shared>, Json(body): Json<ImportAssets>) -> ApiResult {
    let source = std::path::PathBuf::from(body.path.trim().trim_matches('"'));
    let count = tokio::task::spawn_blocking(move || assets::import_folder(&source, &body.collection))
        .await
        .map_err(|error| fail(error.to_string()))?
        .map_err(|error| fail(error.to_string()))?;
    if count == 0 {
        return Err(fail("Aucun fichier .rbxm ou .rbxmx dans ce dossier"));
    }
    state.notify();
    Ok(Json(json!({ "imported": count })))
}

/// Pictures, in whichever Studio is open, the bank assets that have no
/// preview yet. The place is left as it was.
pub async fn asset_previews(State(state): State<Shared>) -> ApiResult {
    let studio = crate::studio::pick(&state, None, "edit")
        .map_err(|_| fail("Ouvre une place dans Studio : les aperçus y sont pris"))?;
    let missing: Vec<String> = assets::list()
        .into_iter()
        .filter(|asset| !asset.thumb)
        .map(|asset| asset.id)
        .collect();

    let (mut made, mut failed) = (0, Vec::new());
    for id in &missing {
        match mcp::make_preview(&studio, id).await {
            Ok(()) => made += 1,
            Err(error) => failed.push(format!("{id} : {error}")),
        }
    }
    state.notify();
    Ok(Json(json!({ "made": made, "failed": failed })))
}

#[derive(Deserialize)]
pub struct StoreQuery {
    q: String,
    #[serde(default)]
    kind: String,
}

pub async fn search_store(axum::extract::Query(query): axum::extract::Query<StoreQuery>) -> ApiResult {
    let kind = if query.kind.is_empty() { "model" } else { &query.kind };
    let items = assets::search_store(query.q.trim(), kind, 24).await.map_err(fail)?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
pub struct StoreInsert {
    project_id: Option<String>,
    asset: u64,
    #[serde(default)]
    kind: String,
}

/// Inserts a store asset in Studio from the app, the way an agent would:
/// scripts disabled.
pub async fn insert_store_asset(State(state): State<Shared>, Json(body): Json<StoreInsert>) -> ApiResult {
    let project = body.project_id.as_deref().and_then(|id| state.project(id));
    let kind = if body.kind.is_empty() { "model" } else { &body.kind };
    let arguments = json!({ "asset": body.asset, "type": kind });
    let message = mcp::asset_insert(&state, project.as_ref(), &arguments).await.map_err(fail)?;
    Ok(Json(json!({ "message": message })))
}

pub async fn remove_asset(State(state): State<Shared>, Path(id): Path<String>) -> ApiResult {
    assets::remove(&id).map_err(|error| fail(error.to_string()))?;
    state.notify();
    Ok(Json(json!({})))
}

pub async fn events_ws(State(state): State<Shared>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |mut socket: WebSocket| async move {
        let mut events = state.events.subscribe();
        loop {
            tokio::select! {
                event = events.recv() => {
                    // Lagging only means several changes were coalesced.
                    if matches!(event, Err(RecvError::Closed)) {
                        break;
                    }
                    if socket.send(Message::Text("state".into())).await.is_err() {
                        break;
                    }
                }
                incoming = socket.recv() => {
                    if !matches!(incoming, Some(Ok(_))) {
                        break;
                    }
                }
            }
        }
    })
}

#[derive(Deserialize)]
#[serde(tag = "t")]
enum PtyInput {
    #[serde(rename = "i")]
    Input { d: String },
    #[serde(rename = "r")]
    Resize { cols: u16, rows: u16 },
}

pub async fn pty_ws(
    State(state): State<Shared>,
    Path(id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Some(session) = state.sessions.lock().unwrap().get(&id).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };

    upgrade.on_upgrade(move |socket: WebSocket| async move {
        let (mut sink, mut stream) = socket.split();
        let (scrollback, mut output) = session.attach();

        let forward = tokio::spawn(async move {
            if !scrollback.is_empty() && sink.send(Message::Binary(scrollback)).await.is_err() {
                return;
            }
            loop {
                match output.recv().await {
                    Ok(chunk) => {
                        if sink.send(Message::Binary(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });

        while let Some(Ok(message)) = stream.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            match serde_json::from_str(&text) {
                Ok(PtyInput::Input { d }) => session.write(d.as_bytes()),
                Ok(PtyInput::Resize { cols, rows }) => session.resize(cols, rows),
                Err(_) => {}
            }
        }
        forward.abort();
    })
}
