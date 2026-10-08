//! What agents working on the same project need from each other: who is
//! doing what, which files are taken, and a way to come back after the app
//! restarts.

use std::{
    path::Path as FsPath,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    isolation, lint,
    pty::{self, Kind, Launch},
    state::{Project, Shared},
};

/// How long an edit keeps a file reserved for the agent that made it. Long
/// enough to cover a train of edits, short enough that a finished agent
/// doesn't block the others for the rest of the session.
const AUTO_LOCK: Duration = Duration::from_secs(600);

pub struct Lock {
    pub project_id: String,
    pub session_id: String,
    /// Relative to the project, forward slashes, as the agent wrote it.
    /// Compared without regard to case, like the file system it describes.
    pub path: String,
    /// Asked for with `claim_files`, as opposed to left behind by an edit.
    pub explicit: bool,
    pub note: String,
    pub at: Instant,
}

/// Puts a path in the form locks are compared in, or `None` when it lies
/// outside the project: those files are nobody's business here.
fn relative(project: &Project, file: &str) -> Option<String> {
    // An isolated agent names files by where the project is mounted for it.
    if let Some(inside) = file.trim().strip_prefix(&format!("{}/", isolation::mount_point(project))) {
        return (!inside.is_empty()).then(|| inside.to_owned());
    }
    // Any other absolute path of that world, another project's mount
    // included, is outside this project.
    if file.trim().starts_with('/') {
        return None;
    }

    let normalized = file.trim().replace('\\', "/");
    let root = project.path.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');

    let inside = if FsPath::new(file.trim()).is_absolute() {
        if !normalized.to_lowercase().starts_with(&root.to_lowercase()) {
            return None;
        }
        // Counted in characters: lowercasing can change a string's byte length.
        let rest: String = normalized.chars().skip(root.chars().count()).collect();
        // `…/Demo2/x` starts with `…/Demo` without being inside it.
        if !rest.starts_with('/') {
            return None;
        }
        rest
    } else {
        normalized
    };
    let inside = inside.trim_matches('/').trim_start_matches("./");
    (!inside.is_empty()).then(|| inside.to_owned())
}

/// A lock on a folder covers everything below it, in both directions.
fn overlaps(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn prune(state: &Shared) {
    let alive: Vec<String> = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .filter(|session| !session.exited.load(Ordering::Relaxed))
        .map(|session| session.info.id.clone())
        .collect();

    state.locks.lock().unwrap().retain(|lock| {
        alive.contains(&lock.session_id) && (lock.explicit || lock.at.elapsed() < AUTO_LOCK)
    });
}

fn title_of(state: &Shared, session_id: &str) -> String {
    state
        .sessions
        .lock()
        .unwrap()
        .get(session_id)
        .map(|session| session.info.title.clone())
        .unwrap_or_else(|| "un autre agent".to_owned())
}

/// The other agent holding `path`, if any, with the note it left.
fn holder(state: &Shared, project_id: &str, session_id: &str, path: &str) -> Option<(String, String)> {
    prune(state);
    let locks = state.locks.lock().unwrap();
    let lock = holder_among(&locks, project_id, session_id, path)?;
    Some((lock.session_id.clone(), lock.note.clone()))
}

fn holder_among<'a>(locks: &'a [Lock], project_id: &str, session_id: &str, path: &str) -> Option<&'a Lock> {
    locks.iter().find(|lock| {
        lock.project_id == project_id && lock.session_id != session_id && overlaps(&lock.path, path)
    })
}

fn touch(state: &Shared, project_id: &str, session_id: &str, path: &str) {
    let mut locks = state.locks.lock().unwrap();
    if let Some(lock) = locks
        .iter_mut()
        .find(|lock| lock.session_id == session_id && same(&lock.path, path))
    {
        lock.at = Instant::now();
        return;
    }
    locks.push(Lock {
        project_id: project_id.to_owned(),
        session_id: session_id.to_owned(),
        path: path.to_owned(),
        explicit: false,
        note: String::new(),
        at: Instant::now(),
    });
}

pub fn release_all(state: &Shared, session_id: &str) {
    state
        .locks
        .lock()
        .unwrap()
        .retain(|lock| lock.session_id != session_id);
}

pub fn files_of(state: &Shared, session_id: &str) -> Vec<String> {
    state
        .locks
        .lock()
        .unwrap()
        .iter()
        .filter(|lock| lock.session_id == session_id)
        .map(|lock| lock.path.clone())
        .collect()
}

/// `claim_files`: all or nothing, so an agent never ends up holding half of
/// what it planned to work on.
pub fn claim(state: &Shared, project: &Project, session_id: &str, args: &Value) -> Result<String, String> {
    let paths: Vec<String> = args["paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|path| path.as_str())
        .filter_map(|path| relative(project, path))
        .collect();
    if paths.is_empty() {
        return Err("`paths` doit lister au moins un fichier ou dossier du projet".into());
    }

    let mut refused = Vec::new();
    for path in &paths {
        if let Some((other, note)) = holder(state, &project.id, session_id, path) {
            let reason = if note.is_empty() { String::new() } else { format!(" ({note})") };
            refused.push(format!("{path} : pris par {}{reason}", title_of(state, &other)));
        }
    }
    if !refused.is_empty() {
        return Err(format!(
            "Rien n'a été réservé, car une partie est déjà prise :\n{}",
            refused.join("\n")
        ));
    }

    let note = args["note"].as_str().unwrap_or_default().trim().to_owned();
    let mut locks = state.locks.lock().unwrap();
    locks.retain(|lock| {
        !(lock.session_id == session_id && paths.iter().any(|path| same(path, &lock.path)))
    });
    for path in &paths {
        locks.push(Lock {
            project_id: project.id.clone(),
            session_id: session_id.to_owned(),
            path: path.clone(),
            explicit: true,
            note: note.clone(),
            at: Instant::now(),
        });
    }
    drop(locks);
    state.notify();
    Ok(format!(
        "Réservé pour toi jusqu'à release_files ou la fin de ta session :\n{}",
        paths.join("\n")
    ))
}

pub fn release(state: &Shared, project: &Project, session_id: &str, args: &Value) -> String {
    let only: Option<Vec<String>> = args["paths"].as_array().map(|paths| {
        paths
            .iter()
            .filter_map(|path| path.as_str())
            .filter_map(|path| relative(project, path))
            .collect()
    });

    let mut locks = state.locks.lock().unwrap();
    let before = locks.len();
    locks.retain(|lock| {
        lock.session_id != session_id
            || only
                .as_ref()
                .is_some_and(|only| !only.iter().any(|path| same(path, &lock.path)))
    });
    let released = before - locks.len();
    drop(locks);
    state.notify();
    format!("{released} réservation(s) libérée(s).")
}

/// `agents_status`: what every agent on the project is doing and holding.
pub fn describe(state: &Shared, project_id: &str, me: Option<&str>) -> String {
    prune(state);
    let sessions = state.sessions.lock().unwrap();
    let locks = state.locks.lock().unwrap();

    let mut agents: Vec<_> = sessions
        .values()
        .filter(|session| session.info.project_id == project_id && session.info.kind != Kind::Shell)
        .filter(|session| !session.exited.load(Ordering::Relaxed))
        .collect();
    agents.sort_by_key(|session| session.info.title.clone());

    let mut text = String::new();
    for session in agents {
        let status = session.status.lock().unwrap();
        let state_label = match status.state {
            "working" => "travaille",
            "waiting" => "attend l'utilisateur",
            "idle" => "a fini son tour",
            "starting" => "démarre",
            _ => "état inconnu",
        };
        text.push_str(&format!(
            "{}{} : {state_label}{}\n",
            session.info.title,
            if me == Some(session.info.id.as_str()) { " (toi)" } else { "" },
            if status.detail.is_empty() { String::new() } else { format!(", {}", status.detail) },
        ));
        for lock in locks.iter().filter(|lock| lock.session_id == session.info.id) {
            text.push_str(&format!(
                "  {} {}{}\n",
                if lock.explicit { "réservé :" } else { "modifié récemment :" },
                lock.path,
                if lock.note.is_empty() { String::new() } else { format!(" ({})", lock.note) },
            ));
        }
    }
    if text.is_empty() {
        text.push_str("Aucun agent actif sur ce projet.");
    }
    text
}

/// Receives every hook event of a Claude Code session. The answer is the
/// hook's verdict: empty to let things proceed, a denial to stop an edit.
pub async fn hook(
    State(state): State<Shared>,
    Path((token, session_id)): Path<(String, String)>,
    Json(event): Json<Value>,
) -> Json<Value> {
    let proceed = Json(json!({}));
    if token != state.token {
        return proceed;
    }
    let Some(session) = state.sessions.lock().unwrap().get(&session_id).cloned() else {
        return proceed;
    };

    // Resuming can hand the conversation a new id; the latest one is the one
    // that can be resumed next time.
    let mut changed = false;
    if let Some(id) = event["session_id"].as_str() {
        let mut agent_id = session.agent_id.lock().unwrap();
        if agent_id.as_deref() != Some(id) {
            *agent_id = Some(id.to_owned());
            changed = true;
        }
    }

    let before = session.status.lock().unwrap().state;
    let mut verdict = proceed;

    match event["hook_event_name"].as_str().unwrap_or_default() {
        "SessionStart" => session.set_status("idle", "prêt"),
        "UserPromptSubmit" => {
            changed |= !session.used.swap(true, Ordering::Relaxed);
            session.set_status("working", "");
        }
        "PreToolUse" => {
            let file = event["tool_input"]["file_path"]
                .as_str()
                .or(event["tool_input"]["notebook_path"].as_str());
            let project = state.project(&session.info.project_id);

            if let Some((project, path)) = project.zip(file).and_then(|(project, file)| {
                relative(&project, file).map(|path| (project, path))
            }) {
                if let Some((other, note)) = holder(&state, &project.id, &session_id, &path) {
                    let reason = if note.is_empty() { String::new() } else { format!(" ({note})") };
                    verdict = Json(json!({
                        "hookSpecificOutput": {
                            "hookEventName": "PreToolUse",
                            "permissionDecision": "deny",
                            "permissionDecisionReason": format!(
                                "{} travaille sur {path}{reason}. Ne modifie pas ce fichier maintenant : avance sur une autre partie de ta tâche, ou réessaie plus tard. L'outil agents_status du serveur essaim montre qui fait quoi.",
                                title_of(&state, &other)
                            ),
                        }
                    }));
                } else {
                    touch(&state, &project.id, &session_id, &path);
                    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                    session.set_status("working", format!("modifie {name}"));
                }
            }
        }
        "PostToolUse" => {
            let tool = event["tool_name"].as_str().unwrap_or_default();

            // A file that no longer parses, or names something that doesn't
            // exist, is cheaper to fix now than after a failed playtest.
            let written = matches!(tool, "Edit" | "Write" | "MultiEdit")
                .then(|| event["tool_input"]["file_path"].as_str())
                .flatten();
            if let Some((project, file)) = state.project(&session.info.project_id).zip(written) {
                // Checkers run on this side, on the file's Windows path.
                let on_host = match relative(&project, file) {
                    Some(inside) => project.path.join(inside),
                    None => FsPath::new(file).to_path_buf(),
                };
                if let Some(problems) = lint::after_edit(&state, &project, &on_host).await {
                    verdict = Json(json!({ "decision": "block", "reason": problems }));
                }
            }

            // File edits already carry a more useful detail than a tool name.
            if before != "working" || !session.status.lock().unwrap().detail.starts_with("modifie") {
                session.set_status("working", tool);
            }
        }
        "Notification" => {
            // The same event also fires when the agent has simply been idle
            // for a while; only a prompt in the middle of a turn is a call
            // for the user.
            if before == "working" {
                let message = event["message"].as_str().unwrap_or("attend ta réponse");
                session.set_status("waiting", message);
                let _ = state.attention.send(session.info.title.clone());
            }
        }
        "Stop" => {
            session.set_status("idle", "");
            let _ = state.attention.send(session.info.title.clone());
        }
        _ => {}
    }

    if changed {
        save_sessions(&state);
    }
    state.notify();
    verdict
}

#[derive(Serialize, Deserialize)]
struct Saved {
    project_id: String,
    title: String,
    agent_id: String,
    used: bool,
    skip_permissions: bool,
    #[serde(default)]
    isolated: bool,
}

/// Records the agents that are running, so that the next start of the app
/// can bring them back. A session the user closed is no longer in the map,
/// and one that ended on its own is skipped: neither comes back.
pub fn save_sessions(state: &Shared) {
    let saved: Vec<Saved> = state
        .sessions
        .lock()
        .unwrap()
        .values()
        .filter(|session| session.info.kind == Kind::Claude)
        .filter(|session| !session.exited.load(Ordering::Relaxed))
        .filter_map(|session| {
            Some(Saved {
                project_id: session.info.project_id.clone(),
                title: session.info.title.clone(),
                agent_id: session.agent_id.lock().unwrap().clone()?,
                used: session.used.load(Ordering::Relaxed),
                skip_permissions: session.skip_permissions,
                isolated: session.isolated,
            })
        })
        .collect();

    if let Ok(json) = serde_json::to_vec_pretty(&saved) {
        let _ = std::fs::write(state.data_dir.join("sessions.json"), json);
    }
}

pub async fn restore_sessions(state: &Shared) {
    let mut saved: Vec<Saved> = std::fs::read(state.data_dir.join("sessions.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    let number = |session: &Saved| {
        session
            .title
            .rsplit('#')
            .next()
            .and_then(|digits| digits.parse::<u64>().ok())
            .unwrap_or(0)
    };
    // Panes are laid out in creation order: restore in the order the user
    // opened the sessions, which the file doesn't keep.
    saved.sort_by_key(number);

    // New sessions must not reuse the number of a restored one.
    let highest = saved.iter().map(number).max().unwrap_or(0);
    state.skip_ids_to(highest + 1);

    for session in saved {
        let Some(project) = state.project(&session.project_id) else {
            continue;
        };
        // If the isolated environment can't be brought back, the session
        // stays closed: reopening it unconfined would betray its name.
        if session.isolated && isolation::prepare(state, &project).await.is_err() {
            continue;
        }
        let launch = Launch {
            kind: Kind::Claude,
            skip_permissions: session.skip_permissions,
            isolated: session.isolated,
            in_wsl: session.isolated,
            title: Some(session.title),
            // An agent that never got a prompt left no conversation to
            // resume; it simply starts again under the same name.
            resume: session.used.then_some(session.agent_id),
        };
        let _ = pty::spawn(state, &project, launch);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn project() -> Project {
        Project {
            id: "p1".into(),
            name: "Jeu".into(),
            path: PathBuf::from(r"C:\Jeux\Demo"),
            sync_port: 34873,
            place_id: None,
            place_name: None,
        }
    }

    fn lock(project_id: &str, session_id: &str, path: &str) -> Lock {
        Lock {
            project_id: project_id.into(),
            session_id: session_id.into(),
            path: path.into(),
            explicit: true,
            note: String::new(),
            at: Instant::now(),
        }
    }

    #[cfg(windows)]
    #[test]
    fn absolute_paths_become_relative_whatever_their_case() {
        let found = relative(&project(), r"c:\jeux\demo\Src\Shop.luau");
        assert_eq!(found.as_deref(), Some("Src/Shop.luau"));
    }

    #[cfg(windows)]
    #[test]
    fn a_sibling_folder_sharing_the_prefix_is_outside() {
        assert_eq!(relative(&project(), r"C:\Jeux\Demo2\src\a.luau"), None);
        assert_eq!(relative(&project(), r"C:\Windows\win.ini"), None);
    }

    #[test]
    fn relative_paths_are_taken_as_project_paths() {
        assert_eq!(relative(&project(), "./src/a.luau").as_deref(), Some("src/a.luau"));
        assert_eq!(relative(&project(), r"src\shared\b.luau").as_deref(), Some("src/shared/b.luau"));
        assert_eq!(relative(&project(), "  "), None);
    }

    #[test]
    fn an_isolated_agent_names_files_by_its_mount_point() {
        assert_eq!(relative(&project(), "/work/p1/src/a.luau").as_deref(), Some("src/a.luau"));
        // Another project's mount is not this project's file.
        assert_eq!(relative(&project(), "/work/p2/src/a.luau"), None);
        assert_eq!(relative(&project(), "/etc/passwd"), None);
    }

    #[test]
    fn a_folder_lock_covers_what_is_below_it_and_nothing_beside() {
        assert!(overlaps("src/shop", "src/shop/Cart.luau"));
        assert!(overlaps("src/shop/Cart.luau", "src/shop"));
        assert!(overlaps("SRC/Shop", "src/shop"));
        assert!(!overlaps("src/shop", "src/shopping/List.luau"));
        assert!(!overlaps("src/a.luau", "src/b.luau"));
    }

    #[test]
    fn only_another_agent_of_the_same_project_holds_a_file() {
        let locks = vec![
            lock("p1", "s1", "src/shop"),
            lock("p2", "s9", "src/shared/Config.luau"),
        ];

        let held = holder_among(&locks, "p1", "s2", "src/shop/Cart.luau");
        assert_eq!(held.map(|lock| lock.session_id.as_str()), Some("s1"));

        // Its own lock never stops an agent.
        assert!(holder_among(&locks, "p1", "s1", "src/shop/Cart.luau").is_none());
        // Same path, other project.
        assert!(holder_among(&locks, "p1", "s2", "src/shared/Config.luau").is_none());
    }
}
