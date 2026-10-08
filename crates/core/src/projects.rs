use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use crate::{
    import,
    state::{Project, Shared},
};

const LAYOUT_TEMPLATE: &str = "- `src/server` → `ServerScriptService.Server`, `src/client` → `StarterPlayer.StarterPlayerScripts.Client`, `src/shared` → `ReplicatedStorage.Shared`. Le mapping complet est dans `default.project.json`.";
const LAYOUT_IMPORT: &str = "- Ce projet vient d'une place existante : `src/<Service>/...` reproduit l'arborescence de la place, service par service. Seuls les scripts sont des fichiers ; les dossiers portent un `init.meta.json` qui dit à la synchro de ne pas toucher au reste. Ne supprime pas ces fichiers.\n- Les scripts listés dans `IMPORT.md`, s'il existe, sont restés dans Studio.";

const TEMPLATE: &[(&str, &str)] = &[
    (
        "default.project.json",
        include_str!("../templates/default.project.json"),
    ),
    ("AGENTS.md", include_str!("../templates/AGENTS.md")),
    ("CLAUDE.md", include_str!("../templates/CLAUDE.md")),
    (
        "src/server/init.server.luau",
        include_str!("../templates/server.luau"),
    ),
    (
        "src/client/init.client.luau",
        include_str!("../templates/client.luau"),
    ),
    (
        "src/shared/Greeting.luau",
        include_str!("../templates/shared.luau"),
    ),
];

pub fn mcp_config_path(state: &Shared, project_id: &str) -> PathBuf {
    state
        .data_dir
        .join("mcp")
        .join(format!("{project_id}.json"))
}

/// Claude Code reads its MCP servers from a file; it is rewritten on every
/// start because the port can change between runs.
pub fn write_mcp_config(state: &Shared, project_id: &str) -> std::io::Result<()> {
    let path = mcp_config_path(state, project_id);
    fs::create_dir_all(path.parent().unwrap())?;
    let config = json!({
        "mcpServers": {
            "essaim": { "type": "http", "url": state.mcp_url(project_id) }
        }
    });
    fs::write(path, serde_json::to_vec_pretty(&config)?)
}

fn slug(name: &str) -> String {
    let slug: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    slug.trim_matches('-').to_owned()
}

fn default_root() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Essaim")
}

fn write_template(dir: &Path, name: &str) -> std::io::Result<()> {
    for (relative, content) in TEMPLATE {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(
            path,
            content
                .replace("{{name}}", name)
                .replace("{{layout}}", LAYOUT_TEMPLATE),
        )?;
    }
    Ok(())
}

/// The agent instructions of an imported project: same rules, other layout.
fn write_agent_docs(dir: &Path, name: &str) -> std::io::Result<()> {
    for (relative, content) in TEMPLATE {
        if relative.ends_with(".md") {
            fs::write(
                dir.join(relative),
                content
                    .replace("{{name}}", name)
                    .replace("{{layout}}", LAYOUT_IMPORT),
            )?;
        }
    }
    Ok(())
}

fn is_empty(dir: &Path) -> bool {
    !fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some())
}

/// Registers a project. An existing folder that already holds a
/// `default.project.json` is adopted as is; anything else gets the template,
/// or the scripts of a Studio place when `export` carries one.
pub fn create(
    state: &Shared,
    name: &str,
    path: Option<String>,
    export: Option<&Value>,
) -> Result<(Project, Option<import::Report>), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Le nom du projet est vide".into());
    }

    let dir = match path.filter(|path| !path.trim().is_empty()) {
        Some(path) => PathBuf::from(path.trim()),
        None => {
            let chosen = state.settings.lock().unwrap().projects_dir.trim().to_owned();
            let root = if chosen.is_empty() { default_root() } else { PathBuf::from(chosen) };
            root.join(slug(name))
        }
    };

    if state
        .projects
        .lock()
        .unwrap()
        .iter()
        .any(|project| project.path == dir)
    {
        return Err("Ce dossier est déjà un projet Essaim".into());
    }

    let report = if let Some(export) = export {
        // An import never merges into existing files: it would be impossible
        // to tell afterwards which ones still match the place.
        if !is_empty(&dir) {
            return Err(format!("{} doit être vide pour un import", dir.display()));
        }
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let report = import::write(&dir, name, export)?;
        write_agent_docs(&dir, name).map_err(|error| error.to_string())?;
        Some(report)
    } else {
        if !dir.join("default.project.json").exists() {
            if !is_empty(&dir) {
                return Err(format!(
                    "{} n'est pas vide et ne contient pas de default.project.json",
                    dir.display()
                ));
            }
            write_template(&dir, name).map_err(|error| error.to_string())?;
        }
        None
    };

    let project = Project {
        id: uuid::Uuid::new_v4().simple().to_string()[..12].to_owned(),
        name: name.to_owned(),
        path: dir,
        sync_port: crate::sync::allocate_port(state),
        // An imported project belongs to the place it came from.
        place_id: export
            .and_then(|export| export["placeId"].as_u64())
            .filter(|place| *place != 0),
        place_name: export
            .filter(|export| export["placeId"].as_u64().unwrap_or(0) == 0)
            .and_then(|export| export["place"].as_str())
            .map(str::to_owned),
    };

    state.projects.lock().unwrap().push(project.clone());
    state.save_projects().map_err(|error| error.to_string())?;
    write_mcp_config(state, &project.id).map_err(|error| error.to_string())?;
    state.notify();
    Ok((project, report))
}
