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
    pub log: Arc<Mutex<VecDeque<String>>>,
    stop: Arc<Notify>,
}

impl SyncProc {
    pub fn stop(&self) {
        self.stop.notify_one();
    }
}

/// Locates the sync server: next to the app once packaged, in the fork's
/// build output during development.
pub fn binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ESSAIM_SYNC_BIN") {
        return Some(PathBuf::from(path));
    }

    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidates = [
        dir.join("essaim-sync.exe"),
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

pub fn start(state: &Shared, project: &Project) -> Result<u16, String> {
    if let Some(running) = state.syncs.lock().unwrap().get(&project.id) {
        return Ok(running.port);
    }

    let binary = binary().ok_or("Serveur de synchro introuvable (essaim-sync.exe)")?;
    if !port_is_free(project.sync_port) {
        return Err(format!(
            "Le port {} est déjà utilisé par un autre programme",
            project.sync_port
        ));
    }

    let mut command = Command::new(binary);
    command
        .arg("serve")
        .arg(project.path.join("default.project.json"))
        .args(["--address", "127.0.0.1", "--port"])
        .arg(project.sync_port.to_string())
        .current_dir(&project.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let Some(process_id) = child.id() {
        crate::jobs::adopt(process_id);
    }
    let log = Arc::new(Mutex::new(VecDeque::new()));
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
            log,
            stop: stop.clone(),
        },
    );

    let state = state.clone();
    let project_id = project.id.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = child.wait() => {}
            _ = stop.notified() => {
                let _ = child.kill().await;
            }
        }
        state.syncs.lock().unwrap().remove(&project_id);
        state.notify();
    });

    Ok(project.sync_port)
}
