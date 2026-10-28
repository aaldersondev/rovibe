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
const LAYOUT_TYPESCRIPT: &str = "- `src/server` → `ServerScriptService.TS`, `src/client` → `StarterPlayer.StarterPlayerScripts.TS`, `src/shared` → `ReplicatedStorage.TS`. Le mapping complet est dans `default.project.json`, qui pointe vers `out/`, la sortie du compilateur.";
const CODE_LUAU: &str = "- Suffixes : `.server.luau` (Script), `.client.luau` (LocalScript), `.luau` (ModuleScript). `init.*.luau` donne son type au dossier.";
const CODE_TYPESCRIPT: &str = "- Le jeu est écrit en **TypeScript**, compilé en Luau par roblox-ts. Écris le code en `.ts` dans `src/`. RoVibe compile à chaque enregistrement : `out/` (le Luau) et `include/` (bibliothèques de roblox-ts) sont refaits par le compilateur, n'y touche jamais.\n- Suffixes : `.server.ts` (Script), `.client.ts` (LocalScript), `.ts` (ModuleScript). `init.server.ts`, `init.client.ts` ou `init.ts` donne son type au dossier.\n- Imports à partir de `src` : `import { forServer } from \"shared/greeting\"`. Services : `import { Players } from \"@rbxts/services\"`. Les types de toute l'API Roblox sont connus : profite-en, évite `any`.\n- roblox-ts refuse quelques constructions de JavaScript (`null` → `undefined`, `==` → `===`, `typeof` → `typeIs()`, pas d'`arguments`) ; ses messages proposent la correction. Les tableaux s'indexent à partir de 0, le compilateur traduit.\n- Paquets : `npm install @rbxts/<nom>` (par exemple `@rbxts/net`, `@rbxts/react`). Ils arrivent dans Studio sous `ReplicatedStorage.rbxts_include.node_modules`.\n- Du Luau écrit à la main peut vivre dans `src/` (fichier `.luau`, copié tel quel). Pour l'importer depuis TypeScript, ajoute à côté un `.d.ts` qui décrit ses exports.\n- Après chaque fichier `.ts` écrit, les erreurs de compilation te reviennent : un fichier qui ne compile pas n'arrive pas dans Studio. `run_luau` exécute du Luau dans Studio ; le code qui y tourne est celui de `out/`.";

/// The language agents write in. TypeScript is the default: roblox-ts
/// checks every type of Roblox's API before the code runs, and turns it into
/// Luau as it is saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Typescript,
    Luau,
}

const TEMPLATE_TYPESCRIPT: &[(&str, &str)] = &[
    (
        "default.project.json",
        include_str!("../templates/typescript/default.project.json"),
    ),
    ("package.json", include_str!("../templates/typescript/package.json")),
    ("tsconfig.json", include_str!("../templates/typescript/tsconfig.json")),
    (".gitignore", include_str!("../templates/typescript/gitignore")),
    ("AGENTS.md", include_str!("../templates/AGENTS.md")),
    ("CLAUDE.md", include_str!("../templates/CLAUDE.md")),
    (
        "src/server/main.server.ts",
        include_str!("../templates/typescript/main.server.ts"),
    ),
    (
        "src/client/main.client.ts",
        include_str!("../templates/typescript/main.client.ts"),
    ),
    (
        "src/shared/greeting.ts",
        include_str!("../templates/typescript/greeting.ts"),
    ),
];

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
            "rovibe": { "type": "http", "url": state.mcp_url(project_id) }
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

/// `Documents\RoVibe`, where new projects and the asset bank go.
pub fn documents_home() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("RoVibe")
}

fn default_root() -> PathBuf {
    let home = documents_home();
    crate::legacy::home_in_use(&home).unwrap_or(home)
}

/// npm only takes lowercase names without spaces for a package.
fn package_name(name: &str) -> String {
    let package = slug(name).to_lowercase();
    if package.is_empty() || !package.is_ascii() {
        "roblox-game".into()
    } else {
        package
    }
}

fn write_template(dir: &Path, name: &str, language: Language) -> std::io::Result<()> {
    let (files, layout, code) = match language {
        Language::Typescript => (TEMPLATE_TYPESCRIPT, LAYOUT_TYPESCRIPT, CODE_TYPESCRIPT),
        Language::Luau => (TEMPLATE, LAYOUT_TEMPLATE, CODE_LUAU),
    };
    for (relative, content) in files {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(
            path,
            content
                .replace("{{name}}", name)
                .replace("{{package}}", &package_name(name))
                .replace("{{layout}}", layout)
                .replace("{{code}}", code),
        )?;
    }
    Ok(())
}

/// The agent instructions of an imported project: same rules, other layout.
/// An imported place stays in Luau, the language its scripts are in.
fn write_agent_docs(dir: &Path, name: &str) -> std::io::Result<()> {
    for (relative, content) in TEMPLATE {
        if relative.ends_with(".md") {
            fs::write(
                dir.join(relative),
                content
                    .replace("{{name}}", name)
                    .replace("{{layout}}", LAYOUT_IMPORT)
                    .replace("{{code}}", CODE_LUAU),
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
    language: Language,
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
        return Err("Ce dossier est déjà un projet RoVibe".into());
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
            if language == Language::Typescript && !crate::typescript::node_available() {
                return Err("Un projet TypeScript a besoin de Node.js (nodejs.org) pour son compilateur : installe-le puis relance RoVibe, ou choisis Luau".into());
            }
            write_template(&dir, name, language).map_err(|error| error.to_string())?;
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
        protected: false,
    };

    state.projects.lock().unwrap().push(project.clone());
    state.save_projects().map_err(|error| error.to_string())?;
    write_mcp_config(state, &project.id).map_err(|error| error.to_string())?;
    state.notify();
    Ok((project, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typescript_project_is_one_roblox_ts_recognises() {
        let dir = tempfile::tempdir().unwrap();
        write_template(dir.path(), "Obby Rush", Language::Typescript).unwrap();
        assert!(crate::typescript::is_project(dir.path()));

        let package: Value = serde_json::from_str(&fs::read_to_string(dir.path().join("package.json")).unwrap()).unwrap();
        assert_eq!(package["name"], "obby-rush");
        let project: Value = serde_json::from_str(&fs::read_to_string(dir.path().join("default.project.json")).unwrap()).unwrap();
        assert_eq!(project["name"], "Obby Rush");
        assert_eq!(project["tree"]["ServerScriptService"]["TS"]["$path"], "out/server");

        // What the compiler makes is never versioned, so never someone's work.
        let ignored = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        for generated in ["/node_modules/", "/out/", "/include/", "*.rbxl"] {
            assert!(ignored.contains(generated), "{generated}");
        }
        let agents = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains("TypeScript") && !agents.contains("{{"), "{agents}");
    }

    #[test]
    fn a_luau_project_keeps_its_own_rules() {
        let dir = tempfile::tempdir().unwrap();
        write_template(dir.path(), "Jeu", Language::Luau).unwrap();
        assert!(!crate::typescript::is_project(dir.path()));
        let agents = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains(".server.luau") && !agents.contains("{{"), "{agents}");
    }

    #[test]
    fn npm_gets_a_name_it_accepts() {
        assert_eq!(package_name("Grow a Datacenter!"), "grow-a-datacenter");
        assert_eq!(package_name("Été"), "roblox-game");
        assert_eq!(package_name("***"), "roblox-game");
    }
}
