//! Running an agent where a mistake can't reach the rest of the PC.
//!
//! Isolated Claude Code sessions run in a dedicated WSL distribution (created
//! by `scripts/setup-isolation.ps1`) that mounts no Windows drive and can't
//! start Windows programs. Before a session starts, the app mounts the one
//! folder it may touch, its project, from the outside as root, and closes the
//! distribution's network to the agent's user except for the model's API.

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

const DISTRO: &str = "rovibe";
pub const CLAUDE: &str = "/home/agent/.local/bin/claude";
pub const CODEX: &str = "/usr/local/bin/codex";
/// What Codex's hooks run, under the same name as on Windows: the project's
/// hooks file is one for both worlds.
const HOOK_PATH: &str = "/usr/local/bin/rovibe-hook";
const HOOK: &str = "#!/bin/sh\nexec curl -s --max-time 3 -X POST -H 'Content-Type: application/json' --data-binary @- \"$ROVIBE_HOOK\"\n";
const RELAY: &str = include_str!("../plugin/relay.py");
const RELAY_PATH: &str = "/opt/rovibe/relay.py";
const HOSTS_PATH: &str = "/opt/rovibe/allowed-hosts";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Where a project appears inside the distribution.
pub fn mount_point(project: &Project) -> String {
    format!("/work/{}", project.id)
}

/// Folder of the distribution, seen from Windows, where per-session
/// configuration files are dropped for the agent to read.
fn config_dir() -> PathBuf {
    PathBuf::from(format!(r"\\wsl.localhost\{}\home\agent\.rovibe", distro()))
}

pub fn config_path(session_id: &str, what: &str) -> (PathBuf, String) {
    let name = format!("{session_id}-{what}.json");
    (config_dir().join(&name), format!("/home/agent/.rovibe/{name}"))
}

/// Port, on the distribution's own loopback, of the proxy that is an
/// isolated agent's only way out.
pub fn proxy_port(state: &Shared) -> u16 {
    state.port + 1
}

/// The script, run as root, that opens or closes the network for the agent's
/// user. Rules live in the running distribution only, hence on every start.
///
/// Closed, that user may talk to the distribution's own loopback, where the
/// relay and the proxy listen, and to nothing else: not even a DNS server,
/// which would be a way to carry data out one question at a time.
fn network_script(restricted: bool, hosts: &[String]) -> String {
    let mut script = format!(
        "mkdir -p /opt/rovibe
printf '%s\\n' {} > {HOSTS_PATH} && chmod 644 {HOSTS_PATH}\n",
        hosts.iter().map(|host| format!("'{host}'")).collect::<Vec<_>>().join(" ")
    );
    script.push_str("uid=$(id -u agent) || exit 1\n");
    script.push_str("for t in iptables ip6tables; do\n");
    if restricted {
        script.push_str("  command -v $t >/dev/null || { echo sans-pare-feu >&2; exit 3; }\n");
    } else {
        script.push_str("  command -v $t >/dev/null || continue\n");
    }
    script.push_str("  $t -N ROVIBE 2>/dev/null; $t -F ROVIBE || exit 1\n");
    if restricted {
        script.push_str(
            "  $t -A ROVIBE -p udp --dport 53 -j REJECT && $t -A ROVIBE -p tcp --dport 53 -j REJECT \\\n    && $t -A ROVIBE -o lo -j ACCEPT && $t -A ROVIBE -j REJECT || exit 1\n",
        );
    }
    script.push_str(
        "  $t -C OUTPUT -m owner --uid-owner $uid -j ROVIBE 2>/dev/null || $t -A OUTPUT -m owner --uid-owner $uid -j ROVIBE || exit 1\n",
    );
    script.push_str("done\n");
    script
}

/// Runs a script as root, fed through stdin: on a command line, wsl.exe
/// would let a first shell expand its variables before the script sees them.
async fn root_script(script: &str) -> Result<(), String> {
    let mut command = Command::new("wsl.exe");
    command
        .args(["-d", distro(), "-u", "root", "--", "sh", "-s"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut running = command.spawn().map_err(|error| format!("WSL est introuvable : {error}"))?;
    if let Some(mut stdin) = running.stdin.take() {
        stdin.write_all(script.as_bytes()).await.map_err(|error| error.to_string())?;
    }
    let output = tokio::time::timeout(Duration::from_secs(60), running.wait_with_output())
        .await
        .map_err(|_| "WSL n'a pas répondu".to_owned())?
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

async fn apply_network(state: &Shared) -> Result<(), String> {
    let (restricted, hosts) = {
        let settings = state.settings.lock().unwrap();
        (settings.isolation_restricted(), settings.isolation_allowed_hosts())
    };
    root_script(&network_script(restricted, &hosts))
        .await
        .map_err(|error| {
            if error.contains("sans-pare-feu") {
                "le pare-feu de l'environnement isolé n'est pas installé : relance scripts\\setup-isolation.ps1, ou passe le réseau des agents isolés sur « ouvert » dans les réglages".to_owned()
            } else {
                error
            }
        })
}

/// Applies a change of the network settings to agents that are already
/// running. One started while the network was open has no proxy to go
/// through, so closing it cuts that agent off: the safe side to err on.
pub fn refresh_network(state: &Shared) {
    if !*state.wsl_relay.lock().unwrap() {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = apply_network(&state).await {
            crate::log::warn(format!("Réseau de l'environnement isolé : {error}"));
        }
    });
}

/// Whether an isolated agent of this kind runs in the distribution. Claude
/// Code always does, and fails loudly if the distribution is missing; Codex
/// does when it is installed there, and keeps its own sandbox otherwise.
pub fn hosts(kind: crate::pty::Kind) -> bool {
    match kind {
        crate::pty::Kind::Claude => true,
        crate::pty::Kind::Codex => has_codex(),
        crate::pty::Kind::Shell => false,
    }
}

/// Whether Codex is installed in the distribution. Without it, an isolated
/// Codex session falls back on Codex's own sandbox, on Windows.
pub fn has_codex() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, known)) = *cache {
        if at.elapsed() < Duration::from_secs(60) {
            return known;
        }
    }
    let found = installed().is_some() && {
        let mut command = std::process::Command::new("wsl.exe");
        command.args(["-d", distro(), "-u", "root", "--", "test", "-x", CODEX]);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
        command.status().is_ok_and(|status| status.success())
    };
    *cache = Some((Instant::now(), found));
    found
}

/// Variables that send an isolated agent's HTTPS through the proxy. Without
/// them it reaches nothing at all, which is the safe way to fail.
pub fn proxy_env(state: &Shared) -> Vec<String> {
    if !state.settings.lock().unwrap().isolation_restricted() {
        return Vec::new();
    }
    let proxy = format!("http://127.0.0.1:{}", proxy_port(state));
    vec![
        format!("HTTPS_PROXY={proxy}"),
        format!("https_proxy={proxy}"),
        format!("HTTP_PROXY={proxy}"),
        format!("http_proxy={proxy}"),
        "NO_PROXY=127.0.0.1,localhost".to_owned(),
        "no_proxy=127.0.0.1,localhost".to_owned(),
    ]
}

async fn wsl(user: Option<&str>, script: &str) -> Result<String, String> {
    let mut command = Command::new("wsl.exe");
    command.args(["-d", distro()]);
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

pub fn available() -> bool {
    installed().is_some()
}

/// The distribution to run agents in: ours, or the one an earlier version of
/// the app created under its former name.
pub fn distro() -> &'static str {
    installed().unwrap_or(DISTRO)
}

/// Which distribution exists, if any. Asked on every refresh of the UI, so
/// the answer is kept for a while: listing distributions takes a noticeable
/// time.
fn installed() -> Option<&'static str> {
    static CACHE: Mutex<Option<(Instant, Option<&'static str>)>> = Mutex::new(None);
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
    let found = command.output().ok().and_then(|output| {
        let units: Vec<u16> = output
            .stdout
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let listing = String::from_utf16_lossy(&units);
        [DISTRO, crate::legacy::OLD_DISTRO]
            .into_iter()
            .find(|name| listing.lines().any(|line| line.trim() == *name))
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
        .args(["-d", distro(), "-u", "root", "--", "sh", "-c"])
        .arg(format!("mkdir -p /opt/rovibe && cat > {RELAY_PATH}"))
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
        .args(["-d", distro(), "-u", "root", "--", "python3", "-u", RELAY_PATH])
        .arg(state.port.to_string())
        .arg(proxy_port(state).to_string())
        .arg(HOSTS_PATH)
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
            if let Some(host) = request["refused"].as_str() {
                crate::log::warn(format!(
                    "Agent isolé : accès à {host} refusé, hors des hôtes autorisés (voir les réglages)"
                ));
                continue;
            }
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
            "mkdir -p /home/agent/.rovibe && chown agent:agent /home/agent/.rovibe && mkdir -p {target} && (mountpoint -q {target} || mount -t drvfs '{source}' {target} -o uid=$(id -u agent),gid=$(id -g agent))"
        ),
    )
    .await
    .map_err(|error| format!("Le projet n'a pas pu être monté dans WSL : {error}"))?;

    root_script(&format!("cat > {HOOK_PATH} <<'ROVIBE'\n{HOOK}ROVIBE\nchmod 755 {HOOK_PATH}\n"))
        .await
        .map_err(|error| format!("L'environnement isolé n'a pas pu être préparé : {error}"))?;

    // Before any agent starts, and again each time: the rules don't survive
    // a restart of the distribution, and the settings may have changed.
    apply_network(state)
        .await
        .map_err(|error| format!("Le réseau de l'environnement isolé n'a pas pu être réglé : {error}"))?;

    let needed = !std::mem::replace(&mut *state.wsl_relay.lock().unwrap(), true);
    if needed {
        if let Err(error) = start_relay(state).await {
            crate::log::warn(format!("Relais vers l'environnement isolé : {error}"));
            *state.wsl_relay.lock().unwrap() = false;
            return Err(format!("Le relais vers l'app n'a pas démarré : {error}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_network_leaves_the_agent_its_loopback_only() {
        let hosts = vec!["*.anthropic.com".to_owned(), "claude.ai".to_owned()];
        let closed = network_script(true, &hosts);
        assert!(closed.contains("printf '%s\\n' '*.anthropic.com' 'claude.ai' > /opt/rovibe/allowed-hosts"));
        // DNS is refused before loopback is allowed: WSL's resolver sits on it.
        let dns = closed.find("--dport 53 -j REJECT").unwrap();
        let loopback = closed.find("-o lo -j ACCEPT").unwrap();
        let rest = closed.find("-A ROVIBE -j REJECT").unwrap();
        assert!(dns < loopback && loopback < rest);
        // A distribution without a firewall must not start an agent.
        assert!(closed.contains("exit 3"));

        let open = network_script(false, &hosts);
        assert!(open.contains("-F ROVIBE"));
        assert!(!open.contains("REJECT") && !open.contains("exit 3"));
    }
}
