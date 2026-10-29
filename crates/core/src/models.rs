//! The models Codex offers the signed-in account. Codex keeps the list it
//! last fetched in its own folder: reading it shows exactly what
//! `codex --model` accepts there, without asking OpenAI ourselves.

use std::{path::PathBuf, sync::Mutex, time::SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Model {
    pub slug: String,
    pub name: String,
    pub description: String,
}

#[derive(Deserialize)]
struct Cache {
    #[serde(default)]
    models: Vec<Cached>,
}

#[derive(Deserialize)]
struct Cached {
    slug: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    description: String,
    /// "list" for what Codex's own picker shows; "hide" for the rest.
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    priority: i64,
}

fn cache_file() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))?;
    Some(home.join("models_cache.json"))
}

/// The models of Codex's picker, in its order; none before Codex has run
/// once. The state asks on every refresh, so the file, half a megabyte, is
/// only read again once Codex has rewritten it.
pub fn codex() -> Vec<Model> {
    static CACHE: Mutex<Option<(SystemTime, Vec<Model>)>> = Mutex::new(None);
    let Some(file) = cache_file() else { return Vec::new() };
    let Ok(modified) = file.metadata().and_then(|meta| meta.modified()) else { return Vec::new() };
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, known)) = &*cache {
        if *at == modified {
            return known.clone();
        }
    }
    let models = std::fs::read(&file).map(|bytes| parse(&bytes)).unwrap_or_default();
    *cache = Some((modified, models.clone()));
    models
}

fn parse(bytes: &[u8]) -> Vec<Model> {
    let Ok(cache) = serde_json::from_slice::<Cache>(bytes) else { return Vec::new() };
    let mut listed: Vec<Cached> = cache.models.into_iter().filter(|model| model.visibility == "list").collect();
    listed.sort_by_key(|model| model.priority);
    listed
        .into_iter()
        .map(|model| Model {
            name: if model.display_name.is_empty() { model.slug.clone() } else { model.display_name },
            slug: model.slug,
            description: model.description,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_what_codex_shows_in_its_order() {
        let cache = br#"{"fetched_at": "x", "models": [
            {"slug": "b", "display_name": "B", "visibility": "list", "priority": 2, "context_window": 1},
            {"slug": "hidden", "display_name": "H", "visibility": "hide", "priority": 0},
            {"slug": "a", "visibility": "list", "priority": 1, "description": "first"}
        ]}"#;
        let models = parse(cache);
        let slugs: Vec<&str> = models.iter().map(|model| model.slug.as_str()).collect();
        assert_eq!(slugs, ["a", "b"]);
        assert_eq!((models[0].name.as_str(), models[0].description.as_str()), ("a", "first"));
        assert_eq!(models[1].name, "B");
    }

    #[test]
    fn an_unreadable_cache_offers_nothing() {
        assert!(parse(b"not json").is_empty());
        assert!(parse(b"{}").is_empty());
    }
}
