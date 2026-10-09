//! What the app left on the PC when it was called Essaim.
//!
//! Each function here brings one of those things over to its new name, once.
//! Nothing else in the code knows the old name.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

const OLD: &str = "Essaim";
/// The WSL distribution of isolated agents can't be renamed from outside: an
/// existing one keeps being used under its old name.
pub const OLD_DISTRO: &str = "essaim";
const OLD_PLUGIN: &str = "EssaimSync.rbxm";
const SETTINGS: [&str; 4] = ["token", "projects.json", "sessions.json", "settings.json"];

fn copy_settings(from: &Path, to: &Path) -> bool {
    if !from.join("projects.json").exists() {
        return false;
    }
    for file in SETTINGS {
        if from.join(file).exists() {
            let _ = fs::copy(from.join(file), to.join(file));
        }
    }
    true
}

/// Fills a new, empty settings folder from the old one: under Roaming, or
/// under Local where the very first versions kept it.
pub fn adopt_settings(dir: &Path) {
    if dir.join("projects.json").exists() {
        return;
    }
    let candidates = [dirs::data_dir(), dirs::data_local_dir()];
    for previous in candidates.into_iter().flatten() {
        if copy_settings(&previous.join(OLD), dir) {
            return;
        }
    }
}

/// Left in a former settings folder once it has been taken over.
const MOVED: &str = "moved-to-home";

/// Takes over what the app kept under AppData, `former`, into `dir`. The
/// first folder found gives everything; a later one, seen when the app is
/// started from somewhere Windows shows another AppData, only adds its
/// projects: the token and the settings already chosen stay.
///
/// Adding projects rewrites the list a running server holds in memory and
/// would write back over: only a server that is starting may do it
/// (`merging`); any other program leaves that folder for later.
pub fn adopt_appdata(former: &Path, dir: &Path, merging: bool) {
    if !former.join("projects.json").exists() || former.join(MOVED).exists() {
        return;
    }
    if !dir.join("projects.json").exists() {
        copy_settings(former, dir);
    } else if merging {
        merge_projects(&former.join("projects.json"), &dir.join("projects.json"));
    } else {
        return;
    }
    let _ = fs::write(former.join(MOVED), "Les réglages de RoVibe sont désormais dans le dossier .rovibe de l'utilisateur.\n");
}

/// Adds to `into` the projects of `from` that it doesn't have, told apart by
/// their folder. One that would share a sync port gets the next free one.
fn merge_projects(from: &Path, into: &Path) {
    let read = |file: &Path| -> Vec<Value> {
        fs::read(file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    };
    let mut kept = read(into);
    let before = kept.len();
    for mut project in read(from) {
        let known = kept.iter().any(|other| other["path"] == project["path"] || other["id"] == project["id"]);
        if known || !project["path"].is_string() {
            continue;
        }
        let taken: Vec<u64> = kept.iter().filter_map(|other| other["sync_port"].as_u64()).collect();
        if project["sync_port"].as_u64().is_none_or(|port| taken.contains(&port)) {
            let free = (34873..35373).find(|port| !taken.contains(port)).unwrap_or(34873);
            project["sync_port"] = Value::from(free);
        }
        kept.push(project);
    }
    if kept.len() > before {
        if let Ok(bytes) = serde_json::to_vec_pretty(&kept) {
            let _ = fs::write(into, bytes);
        }
    }
}

fn swap_prefix(value: &mut Value, old: &Path, new: &Path) -> bool {
    let Some(inside) = value.as_str().and_then(|path| Path::new(path).strip_prefix(old).ok()) else {
        return false;
    };
    // Joining an empty remainder would leave a trailing separator.
    let moved = if inside.as_os_str().is_empty() { new.to_path_buf() } else { new.join(inside) };
    *value = Value::String(moved.to_string_lossy().into_owned());
    true
}

fn rewrite(file: &Path, change: impl Fn(&mut Value) -> bool) {
    let Some(mut content) = fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return;
    };
    if change(&mut content) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&content) {
            let _ = fs::write(file, bytes);
        }
    }
}

/// Moves the folder of projects and of the asset bank, `old`, to `new`, and
/// points the registered projects at their new place. Returns what happened,
/// for the journal; `None` when there was nothing to move.
///
/// The move fails as a whole while a program holds a file in there, Studio
/// with a place open for instance. The old folder then stays in use, and the
/// move is tried again on the next start.
fn move_home_between(old: &Path, new: &Path, data_dir: &Path) -> Option<Result<(), String>> {
    if !old.is_dir() || new.exists() {
        return None;
    }
    if let Err(error) = fs::rename(old, new) {
        return Some(Err(error.to_string()));
    }

    rewrite(&data_dir.join("projects.json"), |projects| {
        let mut changed = false;
        for project in projects.as_array_mut().into_iter().flatten() {
            changed |= swap_prefix(&mut project["path"], old, new);
        }
        changed
    });
    rewrite(&data_dir.join("settings.json"), |settings| {
        settings.get_mut("projects_dir").is_some_and(|dir| swap_prefix(dir, old, new))
    });
    Some(Ok(()))
}

fn documents() -> PathBuf {
    dirs::document_dir().or_else(dirs::home_dir).unwrap_or_default()
}

pub fn move_home(new: &Path, data_dir: &Path) {
    let old = documents().join(OLD);
    match move_home_between(&old, new, data_dir) {
        Some(Ok(())) => crate::log::info(format!(
            "Le dossier {} s'appelle désormais {}",
            old.display(),
            new.display()
        )),
        Some(Err(error)) => crate::log::info(format!(
            "{} reste à sa place pour l'instant ({error}) : il sera renommé à un démarrage où Studio est fermé",
            old.display()
        )),
        None => {}
    }
}

/// The old folder, for as long as it couldn't be moved.
pub fn home_in_use(new: &Path) -> Option<PathBuf> {
    let old = documents().join(OLD);
    (!new.exists() && old.is_dir()).then_some(old)
}

/// A project's own traces of the old name: its folder of saved prompts, and
/// the name of the MCP server in the instructions its agents read.
pub fn rename_in_project(dir: &Path) {
    let (old, new) = (dir.join(".essaim"), dir.join(".rovibe"));
    if old.is_dir() && !new.exists() {
        let _ = fs::rename(old, new);
    }

    let docs = dir.join("AGENTS.md");
    if let Ok(text) = fs::read_to_string(&docs) {
        let renamed = text.replace("MCP `essaim`", "MCP `rovibe`").replace("Essaim Sync", "RoVibe Sync");
        if renamed != text {
            let _ = fs::write(docs, renamed);
        }
    }
}

/// Where a project's "reviewed up to here" mark used to be kept.
pub const OLD_REVIEWED: &str = "refs/essaim/reviewed";

/// Removes the Studio plugin under its old file name, once the new one is
/// installed: both would connect to the app and answer every call twice.
pub fn remove_old_plugin(plugins: &Path) -> bool {
    fs::remove_file(plugins.join(OLD_PLUGIN)).is_ok()
}

pub fn old_plugin_installed(plugins: &Path) -> bool {
    plugins.join(OLD_PLUGIN).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_come_over_from_the_old_folder_only_once() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join("old"), root.path().join("new"));
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        assert!(!copy_settings(&old, &new));

        fs::write(old.join("projects.json"), "[]").unwrap();
        fs::write(old.join("token"), "t").unwrap();
        fs::write(old.join("essaim.log"), "x").unwrap();
        assert!(copy_settings(&old, &new));
        assert_eq!(fs::read_to_string(new.join("token")).unwrap(), "t");
        assert!(!new.join("essaim.log").exists());
    }

    #[test]
    fn two_former_settings_folders_end_up_as_one() {
        let root = tempfile::tempdir().unwrap();
        let (mine, theirs, home) = (root.path().join("a"), root.path().join("b"), root.path().join("home"));
        for dir in [&mine, &theirs, &home] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(mine.join("token"), "jeton-a").unwrap();
        fs::write(mine.join("settings.json"), "{}").unwrap();
        fs::write(
            mine.join("projects.json"),
            json!([{ "id": "1", "name": "test", "path": "C:/jeux/test", "sync_port": 34873 }]).to_string(),
        )
        .unwrap();
        fs::write(theirs.join("token"), "jeton-b").unwrap();
        fs::write(
            theirs.join("projects.json"),
            json!([
                { "id": "2", "name": "Demo", "path": "C:/jeux/Demo", "sync_port": 34873 },
                { "id": "3", "name": "Jeu en ligne", "path": "C:/jeux/Live", "sync_port": 34874, "protected": true },
                { "id": "9", "name": "test, vu d'ailleurs", "path": "C:/jeux/test", "sync_port": 34880 }
            ])
            .to_string(),
        )
        .unwrap();

        // The first one found gives everything, token included.
        adopt_appdata(&mine, &home, false);
        assert_eq!(fs::read_to_string(home.join("token")).unwrap(), "jeton-a");
        // A program that isn't a starting server leaves the second alone.
        adopt_appdata(&theirs, &home, false);
        assert!(!fs::read_to_string(home.join("projects.json")).unwrap().contains("Demo"));
        // A starting server adds the projects the first didn't have.
        adopt_appdata(&theirs, &home, true);
        assert_eq!(fs::read_to_string(home.join("token")).unwrap(), "jeton-a");
        let projects: Value = serde_json::from_slice(&fs::read(home.join("projects.json")).unwrap()).unwrap();
        let listed: Vec<(&str, u64, bool)> = projects
            .as_array()
            .unwrap()
            .iter()
            .map(|project| (project["name"].as_str().unwrap(), project["sync_port"].as_u64().unwrap(), project["protected"] == true))
            .collect();
        // Demo shared a port with `test` and got another; protection travels.
        assert_eq!(listed, [("test", 34873, false), ("Demo", 34874, false), ("Jeu en ligne", 34875, true)]);

        // Each folder is taken over once: what the user removes stays removed.
        fs::write(home.join("projects.json"), "[]").unwrap();
        adopt_appdata(&mine, &home, true);
        adopt_appdata(&theirs, &home, true);
        assert_eq!(fs::read_to_string(home.join("projects.json")).unwrap(), "[]");
    }

    #[test]
    fn moving_the_home_folder_takes_the_projects_along() {
        let root = tempfile::tempdir().unwrap();
        let (old, new, data) = (root.path().join("Essaim"), root.path().join("RoVibe"), root.path().join("data"));
        fs::create_dir_all(old.join("Demo/src")).unwrap();
        fs::create_dir_all(&data).unwrap();
        let elsewhere = root.path().join("Ailleurs/Jeu");
        fs::write(
            data.join("projects.json"),
            json!([{ "id": "a", "path": old.join("Demo") }, { "id": "b", "path": elsewhere }]).to_string(),
        )
        .unwrap();
        fs::write(data.join("settings.json"), json!({ "projects_dir": old, "claude_model": "x" }).to_string()).unwrap();

        assert_eq!(move_home_between(&old, &new, &data), Some(Ok(())));
        assert!(new.join("Demo/src").is_dir() && !old.exists());
        let projects: Value = serde_json::from_slice(&fs::read(data.join("projects.json")).unwrap()).unwrap();
        assert_eq!(projects[0]["path"], json!(new.join("Demo")));
        assert_eq!(projects[1]["path"], json!(elsewhere));
        let settings: Value = serde_json::from_slice(&fs::read(data.join("settings.json")).unwrap()).unwrap();
        assert_eq!((&settings["projects_dir"], &settings["claude_model"]), (&json!(new), &json!("x")));

        // Nothing left to move, and an existing new folder is never replaced.
        assert_eq!(move_home_between(&old, &new, &data), None);
        fs::create_dir_all(&old).unwrap();
        assert_eq!(move_home_between(&old, &new, &data), None);
    }

    #[test]
    fn a_project_drops_the_old_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".essaim")).unwrap();
        fs::write(dir.path().join(".essaim/consignes.json"), "[]").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "via Essaim Sync.\n## Outils MCP `essaim`\nMon essaim à moi.").unwrap();

        rename_in_project(dir.path());
        assert!(dir.path().join(".rovibe/consignes.json").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(),
            "via RoVibe Sync.\n## Outils MCP `rovibe`\nMon essaim à moi."
        );
    }
}
