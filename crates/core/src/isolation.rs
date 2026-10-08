//! Running an agent where a mistake can't reach the rest of the PC.
//!
//! Isolated Claude Code sessions run in a dedicated WSL distribution (created
//! by `scripts/setup-isolation.ps1`) that mounts no Windows drive and can't
//! start Windows programs. Before a session starts, the app mounts the one
//! folder it may touch, its project, from the outside as root.

use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

use crate::state::{Project, Shared};

pub const DISTRO: &str = "essaim";
pub const CLAUDE: &str = "/home/agent/.local/bin/claude";
const RELAY: &str = include_str!("../plugin/relay.py");
const RELAY_PATH: &str = "/opt/essaim/relay.py";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where a project appears inside the distribution.
pub fn mount_point(project: &Project) -> String {
    format!("/work/{}", project.id)
}

/// Folder of the distribution, seen from Windows, where per-session
/// configuration files are dropped for the agent to read.
fn config_dir() -> PathBuf {
    PathBuf::from(format!(r"\\wsl.localhost\{DISTRO}\home\agent\.essaim"))
}

pub fn config_path(session_id: &str, what: &str) -> (PathBuf, String) {
    let name = format!("{session_id}-{what}.json");
    (config_dir().join(&name), format!("/home/agent/.essaim/{name}"))
}

async fn wsl(user: Option<&str>, script: &str) -> Result<String, String> {
    let mut command = Command::new("wsl.exe");
    command.args(["-d", DISTRO]);
    if let Some(user) = user {
        command.args(["-u", user]);
    }
    // Everything after `--` is handed to a shell as one line, so the script
    // travels as a single quoted argument of `sh -c`.
    command
        .args(["--", "sh", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let output = tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .map_err(|_| "WSL n'a pas répondu".to_owned())?
        .map_err(|error| format!("WSL est introuvable : {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Whether the distribution exists. Asked on every refresh of the UI, so the
/// answer is kept for a while: listing distributions takes a noticeable time.
pub fn available() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, known)) = *cache {
        if at.elapsed() < Duration::from_secs(60) {
            return known;
        }
    }

    let mut command = std::process::Command::new("wsl.exe");
    command.args(["--list", "--quiet"]);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    // wsl.exe prints its own listings in UTF-16.
    let found = command.output().is_ok_and(|output| {
        let units: Vec<u16> = output
            .stdout
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units).lines().any(|line| line.trim() == DISTRO)
    });
    *cache = Some((Instant::now(), found));
    found
}

/// Starts the relay through which agents in the distribution reach the app:
/// a process there that listens on its own 127.0.0.1 and passes requests to
/// this one over its stdin and stdout. No port is opened on any network
/// interface, so the Windows firewall has nothing to allow or block.
async fn start_relay(state: &Shared) -> Result<(), String> {
    // Installed as root on every start: the agent can't alter it, and it
    // always matches this version of the app.
    let mut install = Command::new("wsl.exe");
    install
        .args(["-d", DISTRO, "-u", "root", "--", "sh", "-c"])
        .arg(format!("mkdir -p /opt/essaim && cat > {RELAY_PATH}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    install.creation_flags(CREATE_NO_WINDOW);
    let mut installing = install.spawn().map_err(|error| error.to_string())?;
    if let Some(mut stdin) = installing.stdin.take() {
        stdin.write_all(RELAY.as_bytes()).await.map_err(|error| error.to_string())?;
    }
    installing.wait().await.map_err(|error| error.to_string())?;

    let mut command = Command::new("wsl.exe");
    command
        .args(["-d", DISTRO, "-u", "root", "--", "python3", "-u", RELAY_PATH])
        .arg(state.port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let mut relay = command.spawn().map_err(|error| error.to_string())?;
    if let Some(process_id) = relay.id() {
        crate::jobs::adopt(process_id);
    }
    let replies = Arc::new(tokio::sync::Mutex::new(relay.stdin.take().ok_or("relais sans entrée")?));
    let mut requests = BufReader::new(relay.stdout.take().ok_or("relais sans sortie")?).lines();

    let port = state.port;
    let running = state.clone();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        while let Ok(Some(line)) = requests.next_line().await {
            let Ok(request) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let client = client.clone();
            let replies = replies.clone();
            tokio::spawn(async move {
                let path = request["path"].as_str().unwrap_or_default();
                // An isolated agent gets its tools and its hooks, both of
                // which demand the token in their path. Never the API.
                let allowed = path.starts_with("/mcp/") || path.starts_with("/hook/");
                let (status, body) = if allowed {
                    let sent = client
                        .post(format!("http://127.0.0.1:{port}{path}"))
                        .header("Content-Type", "application/json")
                        .body(request["body"].as_str().unwrap_or_default().to_owned())
                        .send()
                        .await;
                    match sent {
                        Ok(response) => (response.status().as_u16(), response.text().await.unwrap_or_default()),
                        Err(_) => (502, String::new()),
                    }
                } else {
                    (404, String::new())
                };

                let mut line = json!({ "id": request["id"], "status": status, "body": body }).to_string();
                line.push('\n');
                let _ = replies.lock().await.write_all(line.as_bytes()).await;
            });
        }
        // Keeps the process handle alive for as long as the relay answers.
        drop(relay);
        *running.wsl_relay.lock().unwrap() = false;
    });

    // The first agent must find the relay listening.
    tokio::time::sleep(Duration::from_millis(900)).await;
    Ok(())
}

/// Makes the project visible in the distribution and the app reachable from
/// it, at the same 127.0.0.1 address as on Windows.
pub async fn prepare(state: &Shared, project: &Project) -> Result<(), String> {
    if !available() {
        return Err("L'isolation n'est pas installée : lance scripts\\setup-isolation.ps1".into());
    }

    let target = mount_point(project);
    // Single quotes keep the backslashes of the Windows path literal.
    let source = project.path.to_string_lossy().replace('\'', r"'\''");
    wsl(
        Some("root"),
        &format!(
            "mkdir -p {target} && (mountpoint -q {target} || mount -t drvfs '{source}' {target} -o uid=$(id -u agent),gid=$(id -g agent))"
        ),
    )
    .await
    .map_err(|error| format!("Le projet n'a pas pu être monté dans WSL : {error}"))?;

    let needed = !std::mem::replace(&mut *state.wsl_relay.lock().unwrap(), true);
    if needed {
        if let Err(error) = start_relay(state).await {
            *state.wsl_relay.lock().unwrap() = false;
            return Err(format!("Le relais vers l'app n'a pas démarré : {error}"));
        }
    }
    Ok(())
}
