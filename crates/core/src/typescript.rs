//! Projects written in TypeScript, compiled to Luau by roblox-ts.
//!
//! Agents write `.ts` in `src/`; the compiler, kept running in watch mode
//! next to the project's sync server, writes the Luau into `out/`, and the
//! sync carries `out/` to Studio. A save reaches Studio in about half a
//! second. The compiler's findings are kept from one cycle to the next, so
//! that `check_code` and the check after each edit read them instead of
//! compiling again.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
    sync::Notify,
};

use crate::lint::Diagnostic;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Where roblox-ts keeps its command line, inside a project's dependencies.
const COMPILER: &str = "node_modules/roblox-ts/out/CLI/cli.js";
/// The first install downloads the compiler and Roblox's types (about 60 MB).
const INSTALL_LIMIT: Duration = Duration::from_secs(300);
const BUILD_LIMIT: Duration = Duration::from_secs(120);

/// A folder is a roblox-ts project when it has a `tsconfig.json` and lists
/// the compiler among its packages; a folder made by roblox-ts' own
/// `create-roblox-ts` is one too.
pub fn is_project(dir: &Path) -> bool {
    dir.join("tsconfig.json").exists()
        && std::fs::read_to_string(dir.join("package.json")).is_ok_and(|package| package.contains("\"roblox-ts\""))
}

pub fn has_compiler(dir: &Path) -> bool {
    dir.join(COMPILER).exists()
}

fn node() -> Option<PathBuf> {
    which::which("node").ok()
}

fn npm() -> Option<PathBuf> {
    which::which("npm.cmd").or_else(|_| which::which("npm")).ok()
}

/// Node.js and npm, which a TypeScript project needs to install and run its
/// compiler.
pub fn node_available() -> bool {
    node().is_some() && npm().is_some()
}

fn hidden(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("FORCE_COLOR", "0")
        .env("NO_COLOR", "1")
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
}

/// Installs the project's packages (`npm install`), compiler included.
pub async fn install(dir: &Path) -> Result<(), String> {
    let npm = npm().ok_or("npm est introuvable : installe Node.js (nodejs.org) puis relance RoVibe")?;
    let mut command = Command::new(npm);
    command
        .current_dir(dir)
        .args(["install", "--no-audit", "--no-fund", "--loglevel=error"]);
    hidden(&mut command);
    let output = tokio::time::timeout(INSTALL_LIMIT, command.output())
        .await
        .map_err(|_| "npm install n'a pas fini en 5 minutes".to_owned())?
        .map_err(|error| error.to_string())?;
    if output.status.success() && has_compiler(dir) {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    let last: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).rev().take(6).collect();
    Err(format!(
        "npm install a échoué : {}",
        last.into_iter().rev().collect::<Vec<_>>().join(" / ")
    ))
}

/// An agent's own branch is a second folder without the project's packages
/// (they aren't versioned). Rather than a second install, it borrows the
/// project's through a junction, which needs no administrator rights.
pub fn share_modules(project: &Path, worktree: &Path) -> Result<(), String> {
    let source = project.join("node_modules");
    let link = worktree.join("node_modules");
    if !source.exists() || link.exists() {
        return Ok(());
    }
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&source)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("lien vers node_modules impossible".into())
    }
}

/// Removes the junction made by `share_modules`, and only the junction:
/// deleting the agent's folder must never reach into the project's packages.
pub fn unshare_modules(worktree: &Path) {
    let link = worktree.join("node_modules");
    if std::fs::symlink_metadata(&link).is_ok_and(|meta| meta.file_type().is_symlink() || is_junction(&link)) {
        // On a junction, removing the directory entry removes the link alone.
        let _ = std::fs::remove_dir(&link);
    }
}

#[cfg(windows)]
fn is_junction(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

#[cfg(not(windows))]
fn is_junction(_path: &Path) -> bool {
    false
}

/// The sync refuses to start on a `$path` that doesn't exist, and `out/` only
/// exists once the compiler has run. The folders it names are made empty
/// beforehand; the compiler fills them.
pub fn prepare_out(dir: &Path) {
    let Ok(text) = std::fs::read_to_string(dir.join("default.project.json")) else {
        return;
    };
    let Ok(project) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let mut paths = Vec::new();
    collect_paths(&project["tree"], &mut paths);
    for path in paths {
        let target = dir.join(&path);
        // A file it names is the user's; only folders are the compiler's to fill.
        if !target.exists() && Path::new(&path).extension().is_none() {
            let _ = std::fs::create_dir_all(target);
        }
    }
}

fn collect_paths(node: &Value, paths: &mut Vec<String>) {
    if let Some(object) = node.as_object() {
        for (key, value) in object {
            if key == "$path" {
                if let Some(path) = value.as_str() {
                    paths.push(path.to_owned());
                }
            } else if !key.starts_with('$') {
                collect_paths(value, paths);
            }
        }
    }
}

/// One finding of the compiler, as it prints them:
/// `src/a.ts:3:7 - error TS2322: message` or `… - error TS roblox-ts: message`.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// Relative to the project, with forward slashes; empty for a finding
    /// about the whole project (a bad `tsconfig.json`).
    pub path: String,
    pub line: u32,
    pub error: bool,
    pub code: String,
    pub message: String,
}

impl Finding {
    pub fn diagnostic(&self) -> Diagnostic {
        let place = if self.path.is_empty() { "projet".to_owned() } else { format!("{}:{}", self.path, self.line) };
        Diagnostic {
            error: self.error,
            rule: Some(self.code.clone()),
            text: format!("{place}: {} [{}]", self.message, self.code),
        }
    }
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ … letter
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_finding(line: &str) -> Option<Finding> {
    let (location, rest, error) = if let Some((location, rest)) = line.split_once(" - error ") {
        (location, rest, true)
    } else if let Some((location, rest)) = line.split_once(" - warning ") {
        (location, rest, false)
    } else if let Some(rest) = line.strip_prefix("error ") {
        ("", rest, true)
    } else {
        return None;
    };
    let (code, message) = rest.split_once(": ")?;
    if !code.starts_with("TS") {
        return None;
    }
    let (path, line_number) = if location.is_empty() {
        (String::new(), 0)
    } else {
        let mut parts = location.rsplitn(3, ':');
        let (_column, line_number, path) = (parts.next()?, parts.next()?, parts.next()?);
        (path.replace('\\', "/"), line_number.parse().ok()?)
    };
    Some(Finding {
        path,
        line: line_number,
        error,
        code: code.to_owned(),
        message: message.trim().to_owned(),
    })
}

/// Reads the compiler's output a line at a time. Each finding is followed
/// by the source line it points at, which is dropped, except roblox-ts'
/// "Suggestion:", worth keeping with the message.
#[derive(Default)]
struct Reader {
    findings: Vec<Finding>,
}

enum Event {
    Started,
    Finished,
}

impl Reader {
    fn line(&mut self, raw: &str) -> Option<Event> {
        let line = strip_ansi(raw);
        let line = line.trim_end();
        if line.contains("Starting compilation") || line.contains("Starting incremental compilation") {
            self.findings.clear();
            return Some(Event::Started);
        }
        if line.contains("Watching for file changes") {
            return Some(Event::Finished);
        }
        if let Some(finding) = parse_finding(line) {
            self.findings.push(finding);
        } else if let Some(suggestion) = line.strip_prefix("Suggestion: ") {
            if let Some(last) = self.findings.last_mut() {
                last.message.push_str(&format!(" ({suggestion})"));
            }
        }
        None
    }
}

/// Compiles once, for a check when no compiler is watching the folder. One
/// at a time: two compilers writing the same `out/` would race.
pub async fn build_once(dir: &Path) -> Result<Vec<Finding>, String> {
    static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _turn = ONE_AT_A_TIME.lock().await;

    let node = node().ok_or("node est introuvable : installe Node.js (nodejs.org)")?;
    if !has_compiler(dir) {
        return Err("Le compilateur roblox-ts n'est pas installé dans ce projet : lance « npm install » dans son dossier".into());
    }
    let mut command = Command::new(node);
    command.current_dir(dir).arg(dir.join(COMPILER));
    hidden(&mut command);
    let output = tokio::time::timeout(BUILD_LIMIT, command.output())
        .await
        .map_err(|_| "roblox-ts n'a pas fini en 2 minutes".to_owned())?
        .map_err(|error| error.to_string())?;

    let mut reader = Reader::default();
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    for line in text.lines() {
        reader.line(line);
    }
    if !output.status.success() && reader.findings.is_empty() {
        let last: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).take(6).collect();
        return Err(format!("roblox-ts a échoué : {}", last.join(" / ")));
    }
    Ok(reader.findings)
}

/// What the watching compiler last said.
#[derive(Clone, Default)]
pub struct Status {
    pub compiling: bool,
    /// Compilations finished since it started.
    pub cycles: u64,
    pub findings: Vec<Finding>,
    pub finished: Option<Instant>,
}

/// The compiler in watch mode for one folder, stopped with the sync it
/// feeds.
pub struct Watch {
    pub dir: PathBuf,
    status: Mutex<Status>,
    stop: Notify,
}

impl Watch {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn stop(&self) {
        self.stop.notify_one();
    }

    /// Waits for the compiler to be done with whatever was saved just now:
    /// it notices a change after a short delay, then compiles. Returns
    /// what it found, or `None` if it was still compiling at the deadline.
    pub async fn settle(&self, limit: Duration) -> Option<Vec<Finding>> {
        let start = Instant::now();
        let cycles = self.status().cycles;
        // Long enough for the file watcher to notice a save.
        let notice = Duration::from_millis(700);
        loop {
            let status = self.status();
            let moved_on = status.cycles > cycles;
            if !status.compiling && (moved_on || start.elapsed() >= notice) {
                return Some(status.findings);
            }
            if start.elapsed() >= limit {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Starts `rbxtsc --watch` in `dir`. The first compilation starts at once
/// and covers the whole project. `changed` is called each time one starts
/// or ends, for the interface to show it.
pub fn watch(dir: &Path, name: &str, changed: impl Fn() + Send + Sync + 'static) -> Result<Arc<Watch>, String> {
    let node = node().ok_or("node est introuvable : installe Node.js (nodejs.org)")?;
    if !has_compiler(dir) {
        return Err("compilateur roblox-ts absent (npm install)".into());
    }
    let mut command = Command::new(node);
    command.current_dir(dir).arg(dir.join(COMPILER)).arg("--watch");
    hidden(&mut command);
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let Some(process_id) = child.id() {
        crate::jobs::adopt(process_id);
    }

    let watch = Arc::new(Watch {
        dir: dir.to_path_buf(),
        status: Mutex::new(Status { compiling: true, ..Status::default() }),
        stop: Notify::new(),
    });

    if let Some(stdout) = child.stdout.take() {
        let watch = watch.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut reader = Reader::default();
            while let Ok(Some(line)) = lines.next_line().await {
                match reader.line(&line) {
                    Some(Event::Started) => {
                        watch.status.lock().unwrap().compiling = true;
                        changed();
                    }
                    Some(Event::Finished) => {
                        {
                            let mut status = watch.status.lock().unwrap();
                            status.compiling = false;
                            status.cycles += 1;
                            status.finished = Some(Instant::now());
                            status.findings = std::mem::take(&mut reader.findings);
                        }
                        changed();
                    }
                    None => {}
                }
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let name = name.to_owned();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    crate::log::warn(format!("roblox-ts (« {name} ») : {}", strip_ansi(&line)));
                }
            }
        });
    }

    crate::log::info(format!("Compilateur TypeScript de « {name} » démarré"));
    let watching = watch.clone();
    let name = name.to_owned();
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                crate::log::warn(format!(
                    "Compilateur TypeScript de « {name} » arrêté tout seul ({})",
                    status.map(|status| status.to_string()).unwrap_or_default()
                ));
                watching.status.lock().unwrap().compiling = false;
            }
            _ = watching.stop.notified() => {
                let _ = child.kill().await;
            }
        }
    });
    Ok(watch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_errors_and_roblox_ts_errors_are_both_read() {
        let found = parse_finding("src/server/bad.server.ts:2:7 - error TS2322: Type 'string' is not assignable to type 'number'.").unwrap();
        assert_eq!(found.path, "src/server/bad.server.ts");
        assert_eq!(found.line, 2);
        assert_eq!(found.code, "TS2322");
        assert!(found.error);

        let found = parse_finding("src/a.ts:1:11 - error TS roblox-ts: `null` is not supported!").unwrap();
        assert_eq!(found.code, "TS roblox-ts");
        assert_eq!(found.message, "`null` is not supported!");

        let global = parse_finding("error TS5023: Unknown compiler option 'x'.").unwrap();
        assert_eq!(global.path, "");
        assert_eq!(global.diagnostic().text, "projet: Unknown compiler option 'x'. [TS5023]");
    }

    #[test]
    fn source_lines_and_progress_are_not_findings() {
        for line in [
            "2 const n: number = 'texte';",
            "        ~~~~~",
            "[17:41:02] Found 0 errors. Watching for file changes.",
            "  src/server/bad.server.ts:3:15",
            "",
        ] {
            assert!(parse_finding(line).is_none(), "{line}");
        }
    }

    #[test]
    fn a_watch_cycle_ends_with_its_own_findings_and_suggestions() {
        let mut reader = Reader::default();
        let output = [
            "[\u{1b}[90m17:41:05\u{1b}[0m] File change detected. Starting incremental compilation...",
            "\u{1b}[96msrc/a.ts\u{1b}[0m:\u{1b}[93m1\u{1b}[0m:\u{1b}[93m11\u{1b}[0m - \u{1b}[91merror\u{1b}[0m\u{1b}[90m TS roblox-ts: \u{1b}[0m`null` is not supported!",
            "Suggestion: Use `undefined` instead.",
            "1 const a = null;",
        ];
        assert!(matches!(reader.line(output[0]), Some(Event::Started)));
        for line in &output[1..] {
            assert!(reader.line(line).is_none());
        }
        assert!(matches!(reader.line("[17:41:06] Found 1 error. Watching for file changes."), Some(Event::Finished)));
        assert_eq!(reader.findings.len(), 1);
        assert_eq!(reader.findings[0].message, "`null` is not supported! (Use `undefined` instead.)");

        // The next cycle starts from nothing.
        reader.line("[17:41:07] File change detected. Starting incremental compilation...");
        assert!(reader.findings.is_empty());
    }

    #[test]
    fn the_folders_the_sync_expects_are_made_before_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("default.project.json"),
            r#"{"name":"x","tree":{"$className":"DataModel","ServerScriptService":{"TS":{"$path":"out/server"}},
                "ReplicatedStorage":{"rbxts_include":{"$path":"include","node_modules":{"$className":"Folder","@rbxts":{"$path":"node_modules/@rbxts"}}},
                "Config":{"$path":"config.json"}}}}"#,
        )
        .unwrap();
        prepare_out(dir.path());
        assert!(dir.path().join("out/server").is_dir());
        assert!(dir.path().join("include").is_dir());
        assert!(dir.path().join("node_modules/@rbxts").is_dir());
        // A file is never invented.
        assert!(!dir.path().join("config.json").exists());
    }

    #[test]
    fn a_roblox_ts_folder_is_recognised_by_its_compiler() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_project(dir.path()));
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"devDependencies":{"typescript":"5"}}"#).unwrap();
        assert!(!is_project(dir.path()));
        std::fs::write(dir.path().join("package.json"), r#"{"devDependencies":{"roblox-ts":"3.0.0"}}"#).unwrap();
        assert!(is_project(dir.path()));
    }

    #[cfg(windows)]
    #[test]
    fn removing_a_branch_folder_leaves_the_projects_packages_alone() {
        let root = tempfile::tempdir().unwrap();
        let (project, worktree) = (root.path().join("projet"), root.path().join("branche"));
        std::fs::create_dir_all(project.join("node_modules/roblox-ts")).unwrap();
        std::fs::write(project.join("node_modules/roblox-ts/package.json"), "{}").unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        share_modules(&project, &worktree).unwrap();
        assert!(worktree.join("node_modules/roblox-ts/package.json").exists());
        unshare_modules(&worktree);
        assert!(!worktree.join("node_modules").exists());
        assert!(project.join("node_modules/roblox-ts/package.json").exists());
    }
}
