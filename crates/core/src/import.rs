//! Turns the scripts of a place open in Studio into a project on disk.
//!
//! Only scripts become files. Every folder on the way to a script is marked
//! "ignore unknown instances", so that syncing the project back leaves the
//! map, the interfaces and everything else in the place untouched.

use std::{
    collections::BTreeSet,
    fs,
    path::Path,
};

use serde_json::{json, Map, Value};

pub struct Report {
    pub scripts: usize,
    pub skipped: Vec<String>,
}

const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Suffixes the sync server reads as a file type; a name ending in one would
/// come back as a different kind of instance.
const MEANINGFUL_ENDINGS: &[&str] = &[".server", ".client", ".meta", ".project", ".model", ".plugin"];

fn script_suffix(class: &str) -> Option<&'static str> {
    match class {
        "Script" => Some(".server.luau"),
        "LocalScript" => Some(".client.luau"),
        "ModuleScript" => Some(".luau"),
        _ => None,
    }
}

fn check_name(name: &str) -> Result<(), String> {
    let lower = name.to_lowercase();
    let stem = lower.split('.').next().unwrap_or_default();

    let invalid = name.is_empty()
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || lower == "init"
        || RESERVED.contains(&stem)
        || MEANINGFUL_ENDINGS.iter().any(|ending| lower.ends_with(ending));

    if invalid {
        Err(format!("le nom « {name} » ne peut pas devenir un fichier"))
    } else {
        Ok(())
    }
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    fs::write(path, text).map_err(|error| format!("{} : {error}", path.display()))
}

fn write_script(root: &Path, script: &Value) -> Result<(), String> {
    let chain = script["chain"].as_array().ok_or("script sans chemin")?;
    let (leaf, ancestors) = chain.split_last().ok_or("script sans chemin")?;

    for link in chain {
        check_name(link["name"].as_str().unwrap_or_default())?;
    }

    let mut path = root.to_path_buf();
    for ancestor in ancestors {
        path.push(ancestor["name"].as_str().unwrap_or_default());
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;

        let class = ancestor["class"].as_str().unwrap_or("Folder");
        // A script that has children writes its own metadata as a leaf.
        if script_suffix(class).is_none() {
            let mut meta = Map::new();
            if class != "Folder" {
                meta.insert("className".into(), json!(class));
            }
            meta.insert("ignoreUnknownInstances".into(), json!(true));
            write_json(&path.join("init.meta.json"), &Value::Object(meta))?;
        }
    }

    let name = leaf["name"].as_str().unwrap_or_default();
    let class = leaf["class"].as_str().unwrap_or_default();
    let suffix = script_suffix(class).ok_or_else(|| format!("classe {class} non gérée"))?;
    let source = script["source"].as_str().unwrap_or_default();

    let mut properties = Map::new();
    if script["disabled"] == true {
        properties.insert("Disabled".into(), json!(true));
    }
    if let Some(context) = script["runContext"].as_str().filter(|context| *context != "Legacy") {
        properties.insert("RunContext".into(), json!(context));
    }

    if script["children"].as_u64().unwrap_or(0) > 0 {
        // Children can't hang off a file, and without the flag the sync
        // server would delete the ones that aren't scripts.
        path.push(name);
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        fs::write(path.join(format!("init{suffix}")), source).map_err(|error| error.to_string())?;

        let mut meta = Map::new();
        meta.insert("ignoreUnknownInstances".into(), json!(true));
        if !properties.is_empty() {
            meta.insert("properties".into(), Value::Object(properties));
        }
        write_json(&path.join("init.meta.json"), &Value::Object(meta))
    } else {
        fs::write(path.join(format!("{name}{suffix}")), source).map_err(|error| error.to_string())?;
        if properties.is_empty() {
            Ok(())
        } else {
            write_json(
                &path.join(format!("{name}.meta.json")),
                &json!({ "properties": properties }),
            )
        }
    }
}

pub fn write(dir: &Path, name: &str, export: &Value) -> Result<Report, String> {
    let mut skipped: Vec<String> = export["skipped"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|line| line.as_str().map(str::to_owned))
        .collect();

    let mut services = BTreeSet::new();
    let mut scripts = 0;

    for script in export["scripts"].as_array().into_iter().flatten() {
        let service = script["service"].as_str().unwrap_or_default();
        let root = dir.join("src").join(service);
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;

        match write_script(&root, script) {
            Ok(()) => {
                scripts += 1;
                services.insert(service.to_owned());
            }
            Err(reason) => {
                let location: Vec<&str> = script["chain"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|link| link["name"].as_str())
                    .collect();
                skipped.push(format!("game.{service}.{} : {reason}", location.join(".")));
            }
        }
    }

    if scripts == 0 {
        return Err("Aucun script à importer dans cette place".into());
    }

    let mut tree = Map::new();
    tree.insert("$className".into(), json!("DataModel"));
    for service in &services {
        tree.insert(
            service.clone(),
            json!({ "$path": format!("src/{service}"), "$ignoreUnknownInstances": true }),
        );
    }
    write_json(
        &dir.join("default.project.json"),
        &json!({ "name": name, "tree": tree }),
    )?;

    if !skipped.is_empty() {
        let mut text = String::from(
            "# Scripts restés dans Studio\n\nCes scripts n'ont pas pu être transformés en fichiers. Ils existent toujours dans la place et se modifient depuis Studio (ou avec l'outil `run_luau`).\n\n",
        );
        for line in &skipped {
            text.push_str(&format!("- {line}\n"));
        }
        fs::write(dir.join("IMPORT.md"), text).map_err(|error| error.to_string())?;
    }

    Ok(Report { scripts, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(service: &str, chain: &[(&str, &str)], extra: Value) -> Value {
        let chain: Vec<Value> = chain
            .iter()
            .map(|(name, class)| json!({ "name": name, "class": class }))
            .collect();
        let mut script = json!({ "service": service, "chain": chain, "source": "return 1\n", "children": 0, "disabled": false });
        for (key, value) in extra.as_object().into_iter().flatten() {
            script[key] = value.clone();
        }
        script
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn scripts_become_files_and_everything_on_the_way_is_protected() {
        let dir = tempfile::tempdir().unwrap();
        let export = json!({
            "skipped": ["game.ServerScriptService.Twin : plusieurs instances"],
            "scripts": [
                script("ServerScriptService", &[("Systems", "Folder"), ("Economy", "ModuleScript")], json!({})),
                script("ServerScriptService", &[("Net", "Script")], json!({ "children": 2, "disabled": true, "runContext": "Client" })),
                script("StarterGui", &[("Hud", "ScreenGui"), ("Panel", "Frame"), ("Controller", "LocalScript")], json!({})),
            ],
        });

        let report = write(dir.path(), "Jeu", &export).unwrap();
        assert_eq!(report.scripts, 3);
        assert_eq!(report.skipped.len(), 1);

        let server = dir.path().join("src/ServerScriptService");
        assert_eq!(fs::read_to_string(server.join("Systems/Economy.luau")).unwrap(), "return 1\n");
        // A plain folder only needs the flag that keeps its other children.
        assert_eq!(read_json(&server.join("Systems/init.meta.json")), json!({ "ignoreUnknownInstances": true }));

        // A script with children is a folder, so they have somewhere to hang.
        assert!(server.join("Net/init.server.luau").exists());
        assert_eq!(
            read_json(&server.join("Net/init.meta.json")),
            json!({ "ignoreUnknownInstances": true, "properties": { "Disabled": true, "RunContext": "Client" } })
        );

        // Anything that isn't a folder keeps its class, or syncing would
        // replace the interface with an empty folder.
        let gui = dir.path().join("src/StarterGui/Hud");
        assert_eq!(read_json(&gui.join("init.meta.json")), json!({ "className": "ScreenGui", "ignoreUnknownInstances": true }));
        assert_eq!(read_json(&gui.join("Panel/init.meta.json")), json!({ "className": "Frame", "ignoreUnknownInstances": true }));
        assert!(gui.join("Panel/Controller.client.luau").exists());

        let project = read_json(&dir.path().join("default.project.json"));
        assert_eq!(project["name"], "Jeu");
        for service in ["ServerScriptService", "StarterGui"] {
            assert_eq!(project["tree"][service]["$ignoreUnknownInstances"], true, "{service}");
            assert_eq!(project["tree"][service]["$path"], format!("src/{service}"));
        }
        assert!(project["tree"].get("Workspace").is_none());
        assert!(fs::read_to_string(dir.path().join("IMPORT.md")).unwrap().contains("Twin"));
    }

    #[test]
    fn names_that_cannot_be_files_are_left_in_studio() {
        let dir = tempfile::tempdir().unwrap();
        let export = json!({
            "skipped": [],
            "scripts": [
                script("ServerScriptService", &[("Good", "Script")], json!({})),
                script("ServerScriptService", &[("bad:name", "Script")], json!({})),
                script("ServerScriptService", &[("init", "ModuleScript")], json!({})),
                script("ServerScriptService", &[("Api.server", "ModuleScript")], json!({})),
                script("ServerScriptService", &[("CON", "Folder"), ("Inner", "ModuleScript")], json!({})),
                script("ServerScriptService", &[("Trailing.", "ModuleScript")], json!({})),
            ],
        });

        let report = write(dir.path(), "Jeu", &export).unwrap();
        assert_eq!(report.scripts, 1);
        assert_eq!(report.skipped.len(), 5);
        assert!(report.skipped.iter().any(|line| line.contains("game.ServerScriptService.bad:name")));

        let written: Vec<String> = fs::read_dir(dir.path().join("src/ServerScriptService"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(written, ["Good.server.luau"]);
    }

    #[test]
    fn a_place_without_scripts_is_not_a_project() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write(dir.path(), "Vide", &json!({ "skipped": [], "scripts": [] })).is_err());
        assert!(!dir.path().join("default.project.json").exists());
    }
}
