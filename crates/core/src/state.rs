use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{agents::Lock, pty::Session, studio::Studio, sync::SyncProc};

const MAX_LOG_ENTRIES: usize = 5000;

#[derive(Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub sync_port: u16,
    /// Set once a Studio place is bound to the project, so that tool calls
    /// keep reaching the right place when several Studios are open.
    #[serde(default)]
    pub place_id: Option<u64>,
    /// Stands in for the id of a place that was never published: those all
    /// report 0, and only the file name tells them apart.
    #[serde(default)]
    pub place_name: Option<String>,
}

impl Project {
    pub fn is_bound(&self) -> bool {
        self.place_id.is_some() || self.place_name.is_some()
    }

    /// What the project is tied to, in words for an error message.
    pub fn binding_label(&self) -> String {
        match (&self.place_name, self.place_id) {
            (Some(name), _) => name.clone(),
            (None, Some(id)) => format!("n° {id}"),
            (None, None) => String::new(),
        }
    }
}

#[derive(Clone, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    pub level: String,
    pub context: String,
    pub place_id: u64,
    pub message: String,
}

/// Something an agent may only do once the user has said yes in the app.
pub struct Approval {
    pub project_id: String,
    pub requester: String,
    pub request: String,
    pub answer: tokio::sync::oneshot::Sender<bool>,
}

#[derive(Default)]
pub struct LogBuffer {
    pub entries: VecDeque<LogEntry>,
    pub next_seq: u64,
}

impl LogBuffer {
    pub fn push(&mut self, level: String, context: String, place_id: u64, message: String) {
        self.next_seq += 1;
        if self.entries.len() >= MAX_LOG_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(LogEntry {
            seq: self.next_seq,
            level,
            context,
            place_id,
            message,
        });
    }
}

pub struct AppState {
    /// Version of the program hosting this server, shown in the UI.
    pub version: String,
    pub port: u16,
    pub token: String,
    pub data_dir: PathBuf,
    pub projects: Mutex<Vec<Project>>,
    pub sessions: Mutex<HashMap<String, Arc<Session>>>,
    pub studios: Mutex<HashMap<u64, Arc<Studio>>>,
    pub syncs: Mutex<HashMap<String, SyncProc>>,
    pub logs: Mutex<LogBuffer>,
    pub locks: Mutex<Vec<Lock>>,
    /// Agent sessions that were open when the app last closed, waiting for
    /// the user to resume or dismiss them.
    pub dormant: Mutex<Vec<crate::agents::Saved>>,
    pub approvals: Mutex<HashMap<u64, Approval>>,
    /// Whether the relay serving agents inside WSL is running.
    pub wsl_relay: Mutex<bool>,
    /// Version of an update the host found, if any.
    pub update: Mutex<Option<String>>,
    /// The user asking for that update to be installed.
    pub install_update: broadcast::Sender<()>,
    pub events: broadcast::Sender<()>,
    /// Names an agent that just stopped to wait for the user.
    pub attention: broadcast::Sender<String>,
    next_id: AtomicU64,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(port: u16, token: String, data_dir: PathBuf, version: String) -> Self {
        let projects = std::fs::read(data_dir.join("projects.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();

        Self {
            version,
            port,
            token,
            data_dir,
            projects: Mutex::new(projects),
            sessions: Mutex::default(),
            studios: Mutex::default(),
            syncs: Mutex::default(),
            logs: Mutex::default(),
            locks: Mutex::default(),
            dormant: Mutex::default(),
            approvals: Mutex::default(),
            wsl_relay: Mutex::default(),
            update: Mutex::default(),
            install_update: broadcast::channel(4).0,
            events: broadcast::channel(16).0,
            attention: broadcast::channel(16).0,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn skip_ids_to(&self, next: u64) {
        self.next_id.fetch_max(next, Ordering::Relaxed);
    }

    /// Tells every open UI that the state snapshot is stale.
    pub fn notify(&self) {
        let _ = self.events.send(());
    }

    pub fn project(&self, id: &str) -> Option<Project> {
        self.projects
            .lock()
            .unwrap()
            .iter()
            .find(|project| project.id == id)
            .cloned()
    }

    pub fn save_projects(&self) -> std::io::Result<()> {
        let projects = self.projects.lock().unwrap();
        let json = serde_json::to_vec_pretty(&*projects)?;
        std::fs::write(self.data_dir.join("projects.json"), json)
    }

    pub fn session_mcp_url(&self, project_id: &str, session_id: &str) -> String {
        format!("{}/{}", self.mcp_url(project_id), session_id)
    }

    pub fn mcp_url(&self, project_id: &str) -> String {
        // The token and project travel in the path rather than the query
        // string: `&` would need escaping when the URL goes through cmd.exe.
        format!(
            "http://127.0.0.1:{}/mcp/{}/{}",
            self.port, self.token, project_id
        )
    }
}
