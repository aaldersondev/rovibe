use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use bytes::Bytes;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::broadcast;

use crate::{
    agents, isolation,
    state::{Project, Shared},
};

/// Replayed to a UI that attaches after the fact, e.g. after a page reload.
const SCROLLBACK_BYTES: usize = 512 * 1024;

/// Events Claude Code reports back to the app. PreToolUse is limited to the
/// tools that can write files, edits and shell commands alike: it is the one
/// hook that can refuse an action.
const HOOK_EVENTS: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("Edit|Write|MultiEdit|NotebookEdit|Bash|PowerShell")),
    ("PostToolUse", Some("*")),
    ("PermissionRequest", None),
    ("Notification", None),
    ("Stop", None),
];

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Claude,
    Codex,
    Shell,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Claude => "Claude Code",
            Kind::Codex => "Codex",
            Kind::Shell => "Terminal",
        }
    }
}

#[derive(Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub project_id: String,
    pub kind: Kind,
    pub title: String,
    /// The branch the agent works on, when it has a folder of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// A copy of the project for one agent: its own folder on its own git
/// branch. Several agents can then rewrite the same file, and their work
/// meets when the user merges it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
pub struct Worktree {
    pub dir: PathBuf,
    pub branch: String,
}

pub struct Status {
    /// `starting`, `working`, `waiting` (needs the user) or `idle` (turn over).
    pub state: &'static str,
    pub detail: String,
    pub since: Instant,
}

/// What to start. `resume` carries the agent's own conversation id when a
/// session from a previous run of the app is being brought back.
pub struct Launch {
    pub kind: Kind,
    pub skip_permissions: bool,
    pub title: Option<String>,
    pub resume: Option<String>,
    /// Model for this session; `None` falls back to the user's setting.
    pub model: Option<String>,
    /// Confine the agent to its project.
    pub isolated: bool,
    /// The agent runs inside the isolation distribution, which
    /// `isolation::prepare` has made ready for this project.
    pub in_wsl: bool,
    /// The folder the agent works in instead of the project's own.
    pub worktree: Option<Worktree>,
}

impl Launch {
    /// Where the agent starts: its own folder if it has one.
    pub fn folder<'a>(&'a self, project: &'a Project) -> &'a Path {
        self.worktree.as_ref().map_or(&project.path, |worktree| &worktree.dir)
    }
}

pub struct Session {
    pub info: SessionInfo,
    pub exited: AtomicBool,
    pub skip_permissions: bool,
    pub isolated: bool,
    pub worktree: Option<Worktree>,
    /// The agent's own conversation id, needed to resume it later.
    pub agent_id: Mutex<Option<String>>,
    /// Whether the agent ever received a prompt: one that didn't has no
    /// conversation on disk and can't be resumed.
    pub used: AtomicBool,
    pub status: Mutex<Status>,
    /// Every project file this agent changed, for the review of changes.
    pub touched: Mutex<std::collections::BTreeSet<String>>,
    /// Files that already had changes when the agent's current shell command
    /// started: what is dirty afterwards and wasn't before is its doing.
    pub shell_before: Mutex<Option<std::collections::HashSet<String>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// Everything the agent started, so that closing the session leaves
    /// nothing running behind it.
    family: crate::jobs::Family,
    scrollback: Mutex<Vec<u8>>,
    /// Columns and rows the process currently draws for.
    size: Mutex<(u16, u16)>,
    output: broadcast::Sender<Bytes>,
}

/// What a terminal that joins a session needs to show it as it is.
pub struct Attached {
    /// The size everything in `replay` was drawn for. Replayed at any other
    /// width, an agent's interface comes out with its lines over one another.
    pub size: (u16, u16),
    pub replay: Bytes,
    pub output: broadcast::Receiver<Bytes>,
}

impl Session {
    /// Returns everything printed so far together with a receiver for what
    /// comes next. Both are taken under the scrollback lock, which the reader
    /// thread also holds while publishing, so no byte is lost or duplicated.
    pub fn attach(&self) -> Attached {
        let scrollback = self.scrollback.lock().unwrap();
        let size = *self.size.lock().unwrap();
        Attached {
            size,
            replay: Bytes::copy_from_slice(&scrollback),
            output: self.output.subscribe(),
        }
    }

    pub fn write(&self, data: &[u8]) {
        let mut writer = self.writer.lock().unwrap();
        let _ = writer.write_all(data);
        let _ = writer.flush();
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        let mut size = self.size.lock().unwrap();
        if *size == (cols, rows) {
            return;
        }
        let resized = self.master.lock().unwrap().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if resized.is_ok() {
            *size = (cols, rows);
            *LAST_SIZE.lock().unwrap() = (cols, rows);
        }
    }

    pub fn kill(&self) {
        // An agent is rarely one process: Codex runs under a launcher, and
        // either agent may have commands of its own still going.
        self.family.terminate();
        let _ = self.killer.lock().unwrap().kill();
    }

    pub fn set_status(&self, state: &'static str, detail: impl Into<String>) {
        *self.status.lock().unwrap() = Status {
            state,
            detail: detail.into(),
            since: Instant::now(),
        };
    }
}

/// Variables an agent host sets for the session it runs. When RoVibe itself
/// is started from such a session, passing them on would make every agent
/// behave as that session's child (no transcript, borrowed messaging token).
const HOST_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_PID",
    "CLAUDE_AGENT_SDK_MCP_NO_PREFIX",
    "CLAUDE_AGENT_SDK_VERSION",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_DESKTOP_APP_VERSION",
    "CLAUDE_CODE_DISABLE_TERMINAL_TITLE",
    "CLAUDE_CODE_EAGER_FLUSH",
    "CLAUDE_CODE_EMIT_TOOL_USE_SUMMARIES",
    "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_REPORT_FINDINGS",
    "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_TERMINAL_MCP_TOOLS",
];

/// Variables a user sets on purpose, for every Claude Code they run.
const KEPT_PREFIXES: &[&str] = &["CLAUDE_CONFIG_DIR", "CLAUDE_CODE_USE_", "CLAUDE_CODE_GIT_BASH_PATH"];

/// Whether a variable inherited by the app would make an agent behave as the
/// child of another agent's session. When the app was itself started from
/// such a session, that is nearly every `CLAUDE*` variable: one of them
/// switches the interface to a plain, colorless mode.
fn from_host_session(name: &str, started_by_agent: bool) -> bool {
    HOST_SESSION_VARS.contains(&name)
        || (started_by_agent
            && name.starts_with("CLAUDE")
            && !KEPT_PREFIXES.iter().any(|kept| name.starts_with(kept)))
}

/// The size the last terminal to speak had: the best guess for the next
/// session's, until its own terminal says. An agent resuming a conversation
/// prints all of it at once, at whatever size it starts with.
static LAST_SIZE: Mutex<(u16, u16)> = Mutex::new((120, 30));

/// What tells a program it may use every color the terminal shows.
const COLOR_ENV: [(&str, &str); 2] = [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")];

fn command_for(program: &Path) -> CommandBuilder {
    let is_script = program
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "cmd" | "bat"));

    // npm installs CLIs as .cmd shims, which only cmd.exe can launch.
    if is_script {
        let mut command = CommandBuilder::new("cmd.exe");
        command.args(["/d", "/c"]);
        command.arg(program);
        command
    } else {
        CommandBuilder::new(program)
    }
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
    let text = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(path, text).map_err(|error| error.to_string())
}

fn model_for(state: &Shared, launch: &Launch) -> Option<String> {
    let settings = state.settings.lock().unwrap();
    let default = match launch.kind {
        Kind::Claude => &settings.claude_model,
        Kind::Codex => &settings.codex_model,
        Kind::Shell => return None,
    };
    launch
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .or_else(|| (!default.trim().is_empty()).then(|| default.trim().to_owned()))
}

/// Where the agent finds the app. The same address everywhere: inside WSL it
/// is the relay's, which forwards to the app.
fn app_address(state: &Shared) -> String {
    format!("http://127.0.0.1:{}", state.port)
}

/// A per-session configuration file: the path to write it at, and the path
/// the agent must be given, which differ when the agent runs inside WSL.
fn config_file(state: &Shared, launch: &Launch, session_id: &str, what: &str) -> (PathBuf, String) {
    if launch.in_wsl {
        isolation::config_path(session_id, what)
    } else {
        let path = state.data_dir.join(what).join(format!("{session_id}.json"));
        let shown = path.to_string_lossy().into_owned();
        (path, shown)
    }
}

/// Each session gets its own MCP address, which is how a tool call tells the
/// app which agent is asking.
fn write_mcp_config(state: &Shared, project: &Project, session_id: &str, launch: &Launch) -> Result<String, String> {
    let (path, shown) = config_file(state, launch, session_id, "mcp");
    let url = format!(
        "{}/mcp/{}/{}/{}",
        app_address(state),
        state.token,
        project.id,
        session_id
    );
    write_json(&path, &json!({ "mcpServers": { "rovibe": { "type": "http", "url": url } } }))?;
    Ok(shown)
}

/// Settings that make Claude Code post its lifecycle events to the app. The
/// event arrives on the hook's stdin and curl forwards it as is; whatever the
/// app answers is the hook's verdict.
fn write_hook_settings(state: &Shared, session_id: &str, launch: &Launch) -> Result<String, String> {
    let command = format!(
        "curl -s --max-time 3 -X POST -H \"Content-Type: application/json\" --data-binary @- {}/hook/{}/{}",
        app_address(state),
        state.token,
        session_id
    );

    let mut hooks = serde_json::Map::new();
    for (event, matcher) in HOOK_EVENTS {
        let mut entry = json!({ "hooks": [{ "type": "command", "command": command }] });
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        hooks.insert((*event).to_owned(), json!([entry]));
    }

    let (path, shown) = config_file(state, launch, session_id, "hooks");
    write_json(&path, &json!({ "hooks": hooks }))?;
    Ok(shown)
}

/// Marks a hooks file as ours, so that a project's own is never overwritten.
const CODEX_HOOK_TARGET: &str = "%ROVIBE_HOOK%";

/// Codex reads its hooks from the project, one file for every session. The
/// address each session reports to therefore comes from its environment,
/// which also keeps the app's token out of a file that may be committed.
fn write_codex_hooks(folder: &Path) {
    let path = folder.join(".codex").join("hooks.json");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if !existing.contains(CODEX_HOOK_TARGET) {
            crate::log::warn(format!(
                "{} existe déjà : Codex n'enverra ni état ni verrous pour ce projet",
                path.display()
            ));
            return;
        }
    }

    // Codex hands this line to the user's shell, PowerShell as a rule. It is
    // written without a single quote character: nested quotes reached curl
    // mangled and the events were silently never sent. cmd.exe is what
    // expands the variable, and `-T -` reads the event from standard input
    // without the `@` that PowerShell would take for its own syntax.
    let command = format!(
        "cmd.exe /d /c curl -s --max-time 3 -X POST -H Content-Type:application/json -T - {CODEX_HOOK_TARGET}"
    );
    let mut hooks = serde_json::Map::new();
    for (event, matcher) in [
        ("SessionStart", None),
        ("UserPromptSubmit", None),
        ("PreToolUse", Some("Bash|apply_patch")),
        ("PostToolUse", Some("*")),
        ("PermissionRequest", None),
        ("Stop", None),
    ] {
        let mut entry = json!({ "hooks": [{ "type": "command", "command": command }] });
        if let Some(matcher) = matcher {
            entry["matcher"] = json!(matcher);
        }
        hooks.insert(event.to_owned(), json!([entry]));
    }
    let _ = write_json(&path, &json!({ "hooks": hooks }));
}

fn build_command(
    state: &Shared,
    project: &Project,
    session_id: &str,
    launch: &Launch,
    agent_id: Option<&str>,
) -> Result<CommandBuilder, String> {
    let locate = |name: &str| {
        which::which(name).map_err(|_| format!("`{name}` est introuvable dans le PATH"))
    };

    let mut command = match launch.kind {
        Kind::Claude => {
            let mut command = if launch.in_wsl {
                // The distribution's default user is the unprivileged agent,
                // and the project is the only Windows folder mounted there.
                let mut command = CommandBuilder::new("wsl.exe");
                command.args(["-d", isolation::distro(), "--cd"]);
                command.arg(isolation::mount_point(project));
                command.args(["--", "env"]);
                command.args(COLOR_ENV.map(|(name, value)| format!("{name}={value}")));
                command.args(isolation::proxy_env(state));
                command.arg(isolation::CLAUDE);
                command
            } else {
                command_for(&locate("claude")?)
            };
            command.arg("--mcp-config");
            command.arg(write_mcp_config(state, project, session_id, launch)?);
            command.arg("--settings");
            command.arg(write_hook_settings(state, session_id, launch)?);
            if let Some(id) = agent_id {
                command.arg(if launch.resume.is_some() { "--resume" } else { "--session-id" });
                command.arg(id);
            }
            if let Some(model) = model_for(state, launch) {
                command.args(["--model", &model]);
            }
            if launch.skip_permissions {
                command.arg("--dangerously-skip-permissions");
            }
            command
        }
        Kind::Codex => {
            // Through npm's own launcher: started directly, the executable
            // inside the package draws an empty screen. None of the
            // arguments below needs quoting, which is all cmd.exe gets wrong.
            let mut command = command_for(&locate("codex")?);
            if let Some(id) = &launch.resume {
                command.args(["resume", id]);
            }
            write_codex_hooks(launch.folder(project));
            // Codex asks the user to review these hooks the first time it
            // meets them in a project. That question is left to them: the
            // flag that skips it also leaves the session's screen empty.
            command.env(
                "ROVIBE_HOOK",
                format!("{}/hook/{}/{}", app_address(state), state.token, session_id),
            );
            if let Some(model) = model_for(state, launch) {
                command.args(["--model", &model]);
            }
            // Unquoted on purpose: codex falls back to a plain string when
            // the value isn't valid TOML, and quotes don't survive cmd.exe.
            command.arg("-c");
            command.arg(format!(
                "mcp_servers.rovibe.url={}",
                state.session_mcp_url(&project.id, session_id)
            ));
            if launch.isolated {
                // Codex brings its own sandbox: commands run without asking,
                // but can only write inside the project.
                command.args(["--sandbox", "workspace-write"]);
                if launch.skip_permissions {
                    command.args(["--ask-for-approval", "never"]);
                }
            } else if launch.skip_permissions {
                command.arg("--dangerously-bypass-approvals-and-sandbox");
            }
            command
        }
        Kind::Shell => {
            let mut command = CommandBuilder::new("powershell.exe");
            command.arg("-NoLogo");
            command
        }
    };

    let started_by_agent = std::env::var_os("CLAUDECODE").is_some();
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(|name| from_host_session(name, started_by_agent)) {
            command.env_remove(name);
        }
    }
    // The panes are xterm.js terminals: 24-bit color, whatever the app was
    // started from.
    for (name, value) in COLOR_ENV {
        command.env(name, value);
    }
    // An agent host turns colors off for the programs it runs and reads;
    // a user who set this for themselves keeps it.
    if started_by_agent {
        command.env_remove("NO_COLOR");
    }
    // wsl.exe itself starts from a neutral folder: `--cd` is what places the
    // agent, and a Windows working directory would only leak a path.
    if !launch.in_wsl {
        command.cwd(launch.folder(project));
    }
    command.env("ROVIBE_PROJECT", &project.name);
    Ok(command)
}

pub fn spawn(state: &Shared, project: &Project, launch: Launch) -> Result<SessionInfo, String> {
    let number = state.next_id();
    let session_id = format!("s{number}");

    // Chosen here rather than by the agent so that the conversation can be
    // resumed even if the app dies before the agent reports anything.
    let agent_id = match launch.kind {
        Kind::Claude => Some(
            launch
                .resume
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        ),
        _ => None,
    };

    let command = build_command(state, project, &session_id, &launch, agent_id.as_deref())?;

    let start_size = *LAST_SIZE.lock().unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: start_size.1,
            cols: start_size.0,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;

    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| error.to_string())?;
    let family = crate::jobs::Family::new();
    if let Some(process_id) = child.process_id() {
        crate::jobs::adopt(process_id);
        family.adopt(process_id);
    }
    // The child holds its own handle; keeping ours open would stop the reader
    // from ever seeing the end of the stream.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| error.to_string())?;

    let (resumed, isolated, unconfirmed) = (launch.resume.is_some(), launch.isolated, launch.skip_permissions);
    let info = SessionInfo {
        id: session_id,
        project_id: project.id.clone(),
        kind: launch.kind,
        title: launch
            .title
            .unwrap_or_else(|| format!("{} #{number}", launch.kind.label())),
        branch: launch.worktree.as_ref().map(|worktree| worktree.branch.clone()),
    };

    let session = Arc::new(Session {
        info: info.clone(),
        exited: AtomicBool::new(false),
        skip_permissions: launch.skip_permissions,
        isolated: launch.isolated,
        worktree: launch.worktree.clone(),
        agent_id: Mutex::new(agent_id),
        used: AtomicBool::new(launch.resume.is_some()),
        touched: Mutex::default(),
        shell_before: Mutex::default(),
        status: Mutex::new(Status {
            state: if launch.kind == Kind::Shell { "" } else { "starting" },
            detail: String::new(),
            since: Instant::now(),
        }),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(child.clone_killer()),
        family,
        scrollback: Mutex::new(Vec::new()),
        size: Mutex::new(start_size),
        output: broadcast::channel(1024).0,
    });

    let reading = session.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut scrollback = reading.scrollback.lock().unwrap();
                    scrollback.extend_from_slice(&chunk[..read]);
                    if scrollback.len() > SCROLLBACK_BYTES {
                        let excess = scrollback.len() - SCROLLBACK_BYTES;
                        scrollback.drain(..excess);
                    }
                    let _ = reading.output.send(Bytes::copy_from_slice(&chunk[..read]));
                }
            }
        }
    });

    // ConPTY keeps the output pipe open after the process ends, so the exit
    // has to be observed on the child itself rather than on the reader.
    let waiting = session.clone();
    let notified = state.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        waiting.exited.store(true, Ordering::Relaxed);
        crate::log::info(format!("{} : session terminée", waiting.info.title));
        let _ = waiting
            .output
            .send(Bytes::from_static(b"\r\n\x1b[2m[session termin\xc3\xa9e]\x1b[0m\r\n"));
        // An agent that is gone must stop holding files back from the others.
        agents::release_all(&notified, &waiting.info.id);
        agents::save_sessions(&notified);
        notified.notify();
    });

    crate::log::info(format!(
        "{} : session lancée sur « {} »{}{}{}",
        info.title,
        project.name,
        if resumed { ", reprise" } else { "" },
        if isolated { ", isolée" } else { "" },
        if unconfirmed { ", sans confirmations" } else { "" },
    ));
    state
        .sessions
        .lock()
        .unwrap()
        .insert(info.id.clone(), session);
    agents::save_sessions(state);
    state.notify();
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_never_inherits_the_session_the_app_was_started_from() {
        // Always removed.
        assert!(from_host_session("CLAUDECODE", false));
        assert!(from_host_session("CLAUDE_CODE_SESSION_ID", false));
        // What a user sets for themselves stays, unless it came from a host.
        assert!(!from_host_session("CLAUDE_CODE_MAX_OUTPUT_TOKENS", false));
        assert!(from_host_session("CLAUDE_CODE_SIMPLE", true));
        assert!(from_host_session("CLAUDE_EFFORT", true));
        assert!(!from_host_session("CLAUDE_CONFIG_DIR", true));
        assert!(!from_host_session("CLAUDE_CODE_USE_BEDROCK", true));
        assert!(!from_host_session("ANTHROPIC_API_KEY", true));
        assert!(!from_host_session("PATH", true));
    }
}
