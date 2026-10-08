//! Essaim's local server: agent terminals, the Studio bridge, the MCP
//! endpoint and the web UI, all on one loopback port.

mod agents;
mod api;
mod assets;
mod git;
mod guard;
mod import;
mod lint;
pub mod log;
mod input;
mod isolation;
mod jobs;
mod mcp;
mod projects;
mod pty;
mod screenshot;
mod settings;
mod state;
mod studio;
mod sync;

use std::{path::PathBuf, sync::Arc};

use axum::{
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE, HOST},
        StatusCode, Uri,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use rust_embed::Embed;

use state::{AppState, Shared};

pub const DEFAULT_PORT: u16 = 34880;

#[derive(Embed)]
#[folder = "../../ui/dist"]
struct Ui;

pub struct Handle {
    state: Shared,
    pub port: u16,
    /// Address of the UI, to be opened in a window or a browser.
    pub url: String,
}

impl Handle {
    /// Receives the name of each agent that stops to wait for the user, so
    /// the host window can ask for attention.
    pub fn attention(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.state.attention.subscribe()
    }

    /// Tells the UI that the host found a newer version.
    pub fn announce_update(&self, version: &str) {
        *self.state.update.lock().unwrap() = Some(version.to_owned());
        self.state.notify();
    }

    /// Fires when the user asks for the announced update to be installed.
    pub fn install_requests(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.state.install_update.subscribe()
    }
}

/// Settings live under Roaming, apart from the program: the installer puts
/// the app in `%LOCALAPPDATA%\Essaim`, where earlier versions kept them.
fn data_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory"))?
        .join("Essaim");
    std::fs::create_dir_all(&dir)?;

    let previous = dirs::data_local_dir().map(|local| local.join("Essaim"));
    if let Some(previous) = previous.filter(|_| !dir.join("projects.json").exists()) {
        for file in ["token", "projects.json", "sessions.json"] {
            if previous.join(file).exists() {
                let _ = std::fs::copy(previous.join(file), dir.join(file));
            }
        }
    }
    Ok(dir)
}

fn load_token(data_dir: &std::path::Path) -> std::io::Result<String> {
    let path = data_dir.join("token");
    if let Ok(token) = std::fs::read_to_string(&path) {
        if token.len() >= 32 {
            return Ok(token);
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(path, &token)?;
    Ok(token)
}

/// Rejects requests addressed to any name other than loopback, which is what
/// a DNS-rebinding page would send.
async fn local_only(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    let expected = [
        format!("127.0.0.1:{}", state.port),
        format!("localhost:{}", state.port),
    ];
    if !expected.iter().any(|allowed| allowed == host) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(request).await
}

/// The API can start processes, so every call must prove it comes from the
/// UI this server handed its token to.
async fn authenticated(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    // WebSockets can't set headers from a browser, hence the query fallback.
    let query = request.uri().query().and_then(|query| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("token="))
    });

    if bearer.or(query) != Some(state.token.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

async fn ui(State(state): State<Shared>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    let Some(file) = Ui::get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if path == "index.html" {
        // Only a same-origin page can read this response, so embedding the
        // token here doesn't expose it to other sites.
        let html = String::from_utf8_lossy(&file.data).replace("__ESSAIM_TOKEN__", &state.token);
        return ([(CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    }

    let mime = mime_guess::from_path(path).first_or_octet_stream();
    ([(CONTENT_TYPE, mime.as_ref())], file.data.into_owned()).into_response()
}

fn router(state: Shared) -> Router {
    let private = Router::new()
        .route("/api/state", get(api::get_state))
        .route("/api/projects", post(api::create_project))
        .route("/api/projects/{id}", delete(api::remove_project))
        .route("/api/projects/{id}/sync", post(api::project_sync))
        .route("/api/projects/{id}/bind", post(api::bind_project))
        .route(
            "/api/projects/{id}/prompts",
            get(api::get_prompts).put(api::put_prompts),
        )
        .route(
            "/api/projects/{id}/git",
            get(api::project_history).post(api::project_git),
        )
        .route("/api/sessions", post(api::create_session))
        .route("/api/sessions/{id}", delete(api::remove_session))
        .route("/api/plugin/install", post(api::install_plugin))
        .route("/api/assets", get(api::list_assets))
        .route("/api/assets/{id}", delete(api::remove_asset))
        .route("/api/assets/{id}/thumb", get(api::asset_thumb))
        .route("/api/approvals/{id}", post(api::answer_approval))
        .route("/api/update", post(api::install_update))
        .route("/api/log", get(api::read_log))
        .route("/api/settings", get(api::get_settings).put(api::put_settings))
        .route(
            "/api/projects/{id}/changes",
            get(api::project_changes).post(api::review_changes),
        )
        .route("/api/projects/{id}/changes/diff", get(api::change_diff))
        .route(
            "/api/dormant/{agent}",
            post(api::resume_session).delete(api::forget_session),
        )
        .route("/api/projects/{id}/publish", post(api::publish_project))
        .route("/ws/events", get(api::events_ws))
        .route("/ws/pty/{id}", get(api::pty_ws))
        .layer(middleware::from_fn_with_state(state.clone(), authenticated));

    Router::new()
        .merge(private)
        .route("/mcp/{token}/{project}", post(mcp::handler))
        .route("/mcp/{token}/{project}/{session}", post(mcp::session_handler))
        .route("/hook/{token}/{session}", post(agents::hook))
        .route("/studio", get(studio::ws_handler))
        .fallback(ui)
        .layer(middleware::from_fn_with_state(state.clone(), local_only))
        .with_state(state)
}

/// Binds the server and runs it in the background. Fails if the port is taken,
/// which usually means another Essaim is already running.
pub async fn start(port: u16, version: &str) -> anyhow::Result<Handle> {
    let data_dir = data_dir()?;
    log::init(&data_dir);
    log::info(format!("Essaim {version} démarre, port {port}, réglages dans {}", data_dir.display()));

    let token = load_token(&data_dir)?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .inspect_err(|error| log::warn(format!("Port {port} indisponible : {error}")))?;
    let state: Shared = Arc::new(AppState::new(port, token, data_dir, version.to_owned()));

    let projects = state.projects.lock().unwrap().clone();
    for project in &projects {
        let _ = projects::write_mcp_config(&state, &project.id);
        let _ = sync::start(&state, project);
    }

    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    agents::load_dormant(&state);

    Ok(Handle {
        state,
        port,
        url: format!("http://127.0.0.1:{port}/"),
    })
}
