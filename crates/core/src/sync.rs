use std::{
    collections::VecDeque,
    net::TcpListener,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    sync::Notify,
};

use crate::state::{Project, Shared};

const FIRST_SYNC_PORT: u16 = 34873;
const MAX_LOG_LINES: usize = 200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct SyncProc {
    pub port: u16,
    /// The folder being served: the project's, or the one of an agent that
    /// asked to see its own branch in Studio.
    pub source: PathBuf,
    pub log: Arc<Mutex<VecDeque<String>>>,
    /// For a TypeScript project, the compiler that writes the `out/` this
    /// server carries to Studio.
    pub compiler: Option<Arc<crate::typescript::Watch>>,
    /// Required by the server on every request: only the Studio the app
    /// hands it to (with `sync_connect`) can read the project or write to it.
    pub token: String,
    stop: Arc<Notify>,
}

/// What the sync server says about itself (`/api/rovibe/status`).
pub struct ServerStatus {
    pub session_id: String,
    pub message_cursor: u64,
    /// Studios streaming its changes right now.
    pub socket_clients: u64,
}

/// Asks a project's running sync server where it stands.
pub async fn status(state: &Shared, project_id: &str) -> Result<ServerStatus, String> {
    let (port, token) = state
        .syncs
        .lock()
        .unwrap()
        .get(project_id)
        .map(|running| (running.port, running.token.clone()))
        .ok_or("la synchro ne tourne pas")?;
    let reply: serde_json::Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/rovibe/status"))
        .header("X-RoVibe-Token", token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| format!("serveur de synchro injoignable : {error}"))?
        .error_for_status()
        .map_err(|error| format!("serveur de synchro : {error}"))?
        .json()
        .await
        .map_err(|_| "ce serveur de synchro ne sait pas dire où il en est (rovibe-sync trop ancien)".to_owned())?;
    Ok(ServerStatus {
        session_id: reply["sessionId"].as_str().unwrap_or_default().to_owned(),
        message_cursor: reply["messageCursor"].as_u64().unwrap_or(0),
        socket_clients: reply["socketClients"].as_u64().unwrap_or(0),
    })
}

/// The address and token of a project's running sync server.
pub fn endpoint(state: &Shared, project_id: &str) -> Option<(u16, String)> {
    state
        .syncs
        .lock()
        .unwrap()
        .get(project_id)
        .map(|running| (running.port, running.token.clone()))
}

impl SyncProc {
    pub fn stop(&self) {
        self.stop.notify_one();
    }
}

/// Locates the sync server: next to the app once packaged, in the fork's
/// build output during development.
pub fn binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ROVIBE_SYNC_BIN") {
        return Some(PathBuf::from(path));
    }

    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        dir.join("rovibe-sync.exe"),
        dir.join("../../vendor/sync/target/release/rojo.exe"),
    ];
    candidates.into_iter().find(|path| path.exists())
}

fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn allocate_port(state: &Shared) -> u16 {
    let taken: Vec<u16> = state
        .projects
        .lock()
        .unwrap()
        .iter()
        .map(|project| project.sync_port)
        .collect();

    (FIRST_SYNC_PORT..FIRST_SYNC_PORT + 500)
        .find(|port| !taken.contains(port) && port_is_free(*port))
        .unwrap_or(FIRST_SYNC_PORT)
}

fn collect<R: AsyncRead + Unpin + Send + 'static>(stream: R, log: Arc<Mutex<VecDeque<String>>>) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut log = log.lock().unwrap();
            if log.len() >= MAX_LOG_LINES {
                log.pop_front();
            }
            log.push_back(line);
        }
    });
}

/// Serves the project from `project.path`, replacing a server that serves it
/// from another folder. That is how Studio moves between the project and an
/// agent's branch: one server per project, on the project's port.
pub async fn serve(state: &Shared, project: &Project) -> Result<u16, String> {
    let elsewhere = state
        .syncs
        .lock()
        .unwrap()
        .get(&project.id)
        .filter(|running| running.source != project.path)
        .map(|running| running.stop.clone());
    if let Some(stop) = elsewhere {
        stop.notify_one();
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if !state.syncs.lock().unwrap().contains_key(&project.id) && port_is_free(project.sync_port) {
                break;
            }
        }
    }
    // A folder never compiled (an agent's new branch) would reach Studio
    // empty until the watching compiler is done, and Studio would see its
    // scripts deleted then put back: it is compiled once beforehand.
    if crate::typescript::is_project(&project.path) && !project.path.join("out").exists() {
        let _ = crate::typescript::build_once(&project.path).await;
    }
    start(state, project)
}

pub fn start(state: &Shared, project: &Project) -> Result<u16, String> {
    if let Some(running) = state.syncs.lock().unwrap().get(&project.id) {
        return Ok(running.port);
    }

    let binary = binary().ok_or("Serveur de synchro introuvable (rovibe-sync.exe)")?;
    if !port_is_free(project.sync_port) {
        return Err(format!(
            "Le port {} est déjà utilisé par un autre programme",
            project.sync_port
        ));
    }

    // TypeScript: the server carries the compiler's output, kept up to date
    // by a compiler watching the same folder for as long as the server runs.
    let compiler = if crate::typescript::is_project(&project.path) {
        crate::typescript::prepare_out(&project.path);
        let notified = state.clone();
        match crate::typescript::watch(&project.path, &project.name, move || notified.notify()) {
            Ok(watch) => Some(watch),
            Err(error) => {
                crate::log::warn(format!("TypeScript de « {} » non compilé : {error}", project.name));
                None
            }
        }
    } else {
        None
    };

    let token = uuid::Uuid::new_v4().simple().to_string();
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .arg(project.path.join("default.project.json"))
        .args(["--address", "127.0.0.1", "--port"])
        .arg(project.sync_port.to_string())
        // Read by the fork: the token it requires, and the project a place
        // stamped for another one must refuse.
        .env("ROVIBE_SYNC_TOKEN", &token)
        .env("ROVIBE_PROJECT_ID", &project.id)
        .current_dir(&project.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Some(compiler) = &compiler {
                compiler.stop();
            }
            return Err(error.to_string());
        }
    };
    if let Some(process_id) = child.id() {
        crate::jobs::adopt(process_id);
    }
    let log = Arc::new(Mutex::new(VecDeque::new()));
    let output = log.clone();
    if let Some(stdout) = child.stdout.take() {
        collect(stdout, log.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        collect(stderr, log.clone());
    }

    let stop = Arc::new(Notify::new());
    state.syncs.lock().unwrap().insert(
        project.id.clone(),
        SyncProc {
            port: project.sync_port,
            source: project.path.clone(),
            log,
            compiler: compiler.clone(),
            token,
            stop: stop.clone(),
        },
    );

    crate::log::info(format!("Synchro de « {} » démarrée sur le port {}", project.name, project.sync_port));
    let state = state.clone();
    let project_id = project.id.clone();
    let name = project.name.clone();
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                // Nobody asked for this: its last words are the only clue.
                let last: Vec<String> = output.lock().unwrap().iter().rev().take(5).rev().cloned().collect();
                crate::log::warn(format!(
                    "Synchro de « {name} » arrêtée toute seule ({}) : {}",
                    status.map(|status| status.to_string()).unwrap_or_default(),
                    last.join(" / ")
                ));
            }
            _ = stop.notified() => {
                let _ = child.kill().await;
                crate::log::info(format!("Synchro de « {name} » arrêtée"));
            }
        }
        if let Some(compiler) = compiler {
            compiler.stop();
        }
        state.syncs.lock().unwrap().remove(&project_id);
        state.notify();
    });

    Ok(project.sync_port)
}

/// The compiler watching a project's folder, if it is a TypeScript project
/// whose sync runs.
pub fn compiler(state: &Shared, project_id: &str) -> Option<Arc<crate::typescript::Watch>> {
    state.syncs.lock().unwrap().get(project_id).and_then(|running| running.compiler.clone())
}
