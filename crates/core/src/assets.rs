//! The asset bank: models saved from Studio as `.rbxm` files in a folder the
//! user can also drop files into, plus search over Roblox's Creator Store.

use std::{collections::HashMap, fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const STORE: &str = "https://apis.roblox.com/toolbox-service/v1";

#[derive(Clone, Default, Serialize, Deserialize)]
struct Meta {
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    class: String,
    #[serde(default)]
    instances: u64,
}

#[derive(Serialize)]
pub struct BankAsset {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub class: String,
    pub instances: u64,
    pub bytes: u64,
    /// Whether a preview image was captured when the asset was saved.
    pub thumb: bool,
}

pub fn bank_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Essaim")
        .join("Banque")
}

fn index_path() -> PathBuf {
    bank_dir().join("index.json")
}

fn load_index() -> HashMap<String, Meta> {
    fs::read(index_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_index(index: &HashMap<String, Meta>) -> io::Result<()> {
    fs::write(index_path(), serde_json::to_vec_pretty(index)?)
}

/// Ids double as file names, so anything that could leave the bank folder is
/// refused rather than cleaned up.
fn file_for(id: &str) -> io::Result<PathBuf> {
    let safe = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if !safe {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "identifiant d'asset invalide",
        ));
    }
    Ok(bank_dir().join(format!("{id}.rbxm")))
}

/// Lists every `.rbxm` in the bank. The folder is the source of truth: a file
/// dropped in by hand shows up without an index entry, named after itself.
pub fn list() -> Vec<BankAsset> {
    let index = load_index();
    let mut assets: Vec<BankAsset> = fs::read_dir(bank_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != "rbxm" {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_owned();
            let meta = index.get(&id).cloned().unwrap_or_default();
            Some(BankAsset {
                name: if meta.name.is_empty() {
                    id.clone()
                } else {
                    meta.name
                },
                id,
                tags: meta.tags,
                class: meta.class,
                instances: meta.instances,
                bytes: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                thumb: path.with_extension("jpg").exists(),
            })
        })
        .collect();
    assets.sort_by_key(|asset| asset.name.to_lowercase());
    assets
}

pub fn search_bank(query: &str, limit: usize) -> Vec<BankAsset> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    list()
        .into_iter()
        .filter(|asset| {
            let haystack =
                format!("{} {} {}", asset.name, asset.tags.join(" "), asset.class).to_lowercase();
            words.iter().all(|word| haystack.contains(word))
        })
        .take(limit)
        .collect()
}

pub fn save(
    name: &str,
    tags: Vec<String>,
    class: &str,
    instances: u64,
    data: &[u8],
) -> io::Result<String> {
    fs::create_dir_all(bank_dir())?;

    let base: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "asset" } else { base };

    // Saving under a name that is taken keeps both assets.
    let mut id = base.to_owned();
    let mut suffix = 2;
    while file_for(&id)?.exists() {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }

    fs::write(file_for(&id)?, data)?;
    let mut index = load_index();
    index.insert(
        id.clone(),
        Meta {
            name: name.trim().to_owned(),
            tags,
            class: class.to_owned(),
            instances,
        },
    );
    save_index(&index)?;
    Ok(id)
}

pub fn read(id: &str) -> io::Result<Vec<u8>> {
    fs::read(file_for(id)?)
}

pub fn thumb_path(id: &str) -> io::Result<PathBuf> {
    Ok(file_for(id)?.with_extension("jpg"))
}

pub fn remove(id: &str) -> io::Result<()> {
    fs::remove_file(file_for(id)?)?;
    let _ = fs::remove_file(thumb_path(id)?);
    let mut index = load_index();
    index.remove(id);
    save_index(&index)
}

fn store_category(kind: &str) -> Option<u32> {
    match kind {
        "model" => Some(10),
        "audio" => Some(3),
        "decal" => Some(13),
        "mesh" => Some(40),
        _ => None,
    }
}

async fn get_json(url: &str, query: &[(&str, String)]) -> Result<Value, String> {
    let response = reqwest::Client::new()
        .get(url)
        .query(query)
        .send()
        .await
        .map_err(|error| format!("Creator Store injoignable : {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Creator Store : erreur {}", response.status()));
    }
    response.json().await.map_err(|error| error.to_string())
}

/// Searches the public Creator Store. Only free assets are returned: a paid
/// one can't be inserted without buying it first.
pub async fn search_store(query: &str, kind: &str, limit: usize) -> Result<String, String> {
    let category = store_category(kind).ok_or("Type inconnu : model, audio, decal ou mesh")?;

    let found = get_json(
        &format!("{STORE}/marketplace/{category}"),
        &[("keyword", query.to_owned()), ("limit", limit.to_string())],
    )
    .await?;

    let ids: Vec<String> = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_u64())
        .map(|id| id.to_string())
        .collect();
    if ids.is_empty() {
        return Ok(String::new());
    }

    let details = get_json(
        &format!("{STORE}/items/details"),
        &[("assetIds", ids.join(","))],
    )
    .await?;

    let mut text = String::new();
    for item in details["data"].as_array().into_iter().flatten() {
        if item["fiatProduct"]["isFree"] == false {
            continue;
        }
        let asset = &item["asset"];
        text.push_str(&format!(
            "{} | {} | par {}{}",
            asset["id"],
            asset["name"].as_str().unwrap_or("?"),
            item["creator"]["name"].as_str().unwrap_or("?"),
            if item["creator"]["isVerifiedCreator"] == true {
                " (vérifié)"
            } else {
                ""
            },
        ));
        if let Some(percent) = item["voting"]["upVotePercent"].as_u64() {
            text.push_str(&format!(
                " | {percent} % positifs sur {} votes",
                item["voting"]["voteCount"]
            ));
        }
        if kind == "model" {
            if let Some(triangles) =
                asset["modelTechnicalDetails"]["objectMeshSummary"]["triangles"].as_u64()
            {
                text.push_str(&format!(" | {triangles} triangles"));
            }
            text.push_str(if asset["hasScripts"] == true {
                " | CONTIENT DES SCRIPTS"
            } else {
                " | sans script"
            });
        }
        text.push('\n');
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_asset_id_cannot_leave_the_bank() {
        for id in ["../secret", "..", "a/b", r"a\b", "C:evil", "", "x.rbxm"] {
            assert!(read(id).is_err(), "{id}");
            assert!(thumb_path(id).is_err(), "{id}");
            assert!(remove(id).is_err(), "{id}");
        }
    }

    #[test]
    fn store_types_map_to_roblox_categories() {
        assert_eq!(store_category("model"), Some(10));
        assert_eq!(store_category("audio"), Some(3));
        assert_eq!(store_category("plugin"), None);
    }
}
