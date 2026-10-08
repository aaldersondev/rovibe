//! The asset bank: models saved from Studio in a folder the user can also
//! drop files into, grouped in collections, plus search over Roblox's
//! Creator Store.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const STORE: &str = "https://apis.roblox.com/toolbox-service/v1";
const THUMBNAILS: &str = "https://thumbnails.roblox.com/v1/assets";
/// Binary and XML model files; Studio reads both.
const EXTENSIONS: [&str; 2] = ["rbxm", "rbxmx"];

#[derive(Clone, Default, Serialize, Deserialize)]
struct Meta {
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    class: String,
    #[serde(default)]
    instances: u64,
    #[serde(default)]
    collection: String,
}

#[derive(Serialize)]
pub struct BankAsset {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub class: String,
    pub instances: u64,
    pub bytes: u64,
    pub collection: String,
    /// Whether a preview image exists for the asset.
    pub thumb: bool,
}

pub fn bank_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Essaim")
        .join("Banque")
}

fn load_index(dir: &Path) -> HashMap<String, Meta> {
    fs::read(dir.join("index.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_index(dir: &Path, index: &HashMap<String, Meta>) -> io::Result<()> {
    fs::write(dir.join("index.json"), serde_json::to_vec_pretty(index)?)
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what.to_owned())
}

/// Ids double as file names, so anything that could leave the bank folder is
/// refused rather than cleaned up.
fn check_id(id: &str) -> io::Result<()> {
    let safe = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if safe {
        Ok(())
    } else {
        Err(invalid("identifiant d'asset invalide"))
    }
}

/// The file of an asset, whichever of the two formats it is in. A new asset
/// is binary.
fn file_in(dir: &Path, id: &str) -> io::Result<PathBuf> {
    check_id(id)?;
    let existing = EXTENSIONS
        .iter()
        .map(|extension| dir.join(format!("{id}.{extension}")))
        .find(|path| path.exists());
    Ok(existing.unwrap_or_else(|| dir.join(format!("{id}.rbxm"))))
}

fn slug(name: &str) -> String {
    let slug: String = name
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "asset".to_owned()
    } else {
        slug.to_owned()
    }
}

/// An id nothing in the bank uses yet: saving under a taken name keeps both.
fn free_id(dir: &Path, name: &str) -> io::Result<String> {
    let base = slug(name);
    let mut id = base.clone();
    let mut suffix = 2;
    while file_in(dir, &id)?.exists() {
        id = format!("{base}-{suffix}");
        suffix += 1;
    }
    Ok(id)
}

fn list_in(dir: &Path) -> Vec<BankAsset> {
    let index = load_index(dir);
    let mut assets: Vec<BankAsset> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?.to_lowercase();
            if !EXTENSIONS.contains(&extension.as_str()) {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_owned();
            // The folder is the source of truth: a file dropped in by hand
            // has no index entry and is named after itself.
            let meta = index.get(&id).cloned().unwrap_or_default();
            Some(BankAsset {
                name: if meta.name.is_empty() {
                    id.clone()
                } else {
                    meta.name
                },
                thumb: dir.join(format!("{id}.jpg")).exists(),
                id,
                tags: meta.tags,
                class: meta.class,
                instances: meta.instances,
                collection: meta.collection,
                bytes: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
            })
        })
        .collect();
    assets.sort_by_key(|asset| (asset.collection.to_lowercase(), asset.name.to_lowercase()));
    assets
}

pub fn list() -> Vec<BankAsset> {
    list_in(&bank_dir())
}

fn matches(asset: &BankAsset, query: &str, collection: Option<&str>) -> bool {
    if collection.is_some_and(|wanted| asset.collection.to_lowercase() != wanted.to_lowercase()) {
        return false;
    }
    let haystack = format!(
        "{} {} {} {}",
        asset.name,
        asset.tags.join(" "),
        asset.class,
        asset.collection
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

pub fn search_bank(query: &str, collection: Option<&str>, limit: usize) -> Vec<BankAsset> {
    list()
        .into_iter()
        .filter(|asset| matches(asset, query, collection))
        .take(limit)
        .collect()
}

pub struct NewAsset<'a> {
    pub name: &'a str,
    pub tags: Vec<String>,
    pub class: &'a str,
    pub instances: u64,
    pub collection: &'a str,
}

fn save_in(dir: &Path, asset: NewAsset, data: &[u8]) -> io::Result<String> {
    fs::create_dir_all(dir)?;
    let id = free_id(dir, asset.name)?;
    fs::write(file_in(dir, &id)?, data)?;

    let mut index = load_index(dir);
    index.insert(
        id.clone(),
        Meta {
            name: asset.name.trim().to_owned(),
            tags: asset.tags,
            class: asset.class.to_owned(),
            instances: asset.instances,
            collection: asset.collection.trim().to_owned(),
        },
    );
    save_index(dir, &index)?;
    Ok(id)
}

pub fn save(asset: NewAsset, data: &[u8]) -> io::Result<String> {
    save_in(&bank_dir(), asset, data)
}

pub fn read(id: &str) -> io::Result<Vec<u8>> {
    fs::read(file_in(&bank_dir(), id)?)
}

pub fn thumb_path(id: &str) -> io::Result<PathBuf> {
    check_id(id)?;
    Ok(bank_dir().join(format!("{id}.jpg")))
}

fn remove_in(dir: &Path, id: &str) -> io::Result<()> {
    fs::remove_file(file_in(dir, id)?)?;
    let _ = fs::remove_file(dir.join(format!("{id}.jpg")));
    let mut index = load_index(dir);
    index.remove(id);
    save_index(dir, &index)
}

pub fn remove(id: &str) -> io::Result<()> {
    remove_in(&bank_dir(), id)
}

/// What the user, or the app once it has opened the asset in Studio, can
/// change about an asset. `None` leaves a field as it is.
#[derive(Default, Deserialize)]
pub struct Edit {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub collection: Option<String>,
    #[serde(skip)]
    pub class: Option<String>,
    #[serde(skip)]
    pub instances: Option<u64>,
}

fn edit_in(dir: &Path, id: &str, edit: Edit) -> io::Result<()> {
    if !file_in(dir, id)?.exists() {
        return Err(invalid("asset inconnu"));
    }
    let mut index = load_index(dir);
    let meta = index.entry(id.to_owned()).or_insert_with(|| Meta {
        name: id.to_owned(),
        ..Meta::default()
    });
    if let Some(name) = edit.name.filter(|name| !name.trim().is_empty()) {
        meta.name = name.trim().to_owned();
    }
    if let Some(tags) = edit.tags {
        meta.tags = tags;
    }
    if let Some(collection) = edit.collection {
        meta.collection = collection.trim().to_owned();
    }
    if let Some(class) = edit.class {
        meta.class = class;
    }
    if let Some(instances) = edit.instances {
        meta.instances = instances;
    }
    save_index(dir, &index)
}

pub fn edit(id: &str, edit: Edit) -> io::Result<()> {
    edit_in(&bank_dir(), id, edit)
}

/// Copies every model file found under `source` into the bank. Files in a
/// sub-folder land in a collection named after it, so a pack keeps the
/// grouping its author gave it.
fn import_in(dir: &Path, source: &Path, collection: &str) -> io::Result<usize> {
    if !source.is_dir() {
        return Err(invalid("ce dossier n'existe pas"));
    }
    fs::create_dir_all(dir)?;
    let mut index = load_index(dir);
    let mut imported = 0;
    let mut pending = vec![(source.to_path_buf(), collection.trim().to_owned())];

    while let Some((folder, group)) = pending.pop() {
        for entry in fs::read_dir(&folder)?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                let nested = if group.is_empty() {
                    name
                } else {
                    format!("{group} / {name}")
                };
                pending.push((path, nested));
                continue;
            }
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if !EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }

            let title = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("asset");
            let id = free_id(dir, title)?;
            fs::copy(&path, dir.join(format!("{id}.{extension}")))?;
            index.insert(
                id,
                Meta {
                    name: title.to_owned(),
                    collection: group.clone(),
                    ..Meta::default()
                },
            );
            imported += 1;
        }
    }
    save_index(dir, &index)?;
    Ok(imported)
}

pub fn import_folder(source: &Path, collection: &str) -> io::Result<usize> {
    let bank = bank_dir();
    if source.starts_with(&bank) {
        return Err(invalid("ce dossier est déjà dans la banque"));
    }
    import_in(&bank, source, collection)
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
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| format!("Creator Store injoignable : {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Creator Store : erreur {}", response.status()));
    }
    response.json().await.map_err(|error| error.to_string())
}

#[derive(Serialize)]
pub struct StoreItem {
    pub id: u64,
    pub name: String,
    pub creator: String,
    pub verified: bool,
    /// Share of positive votes, and how many votes there are.
    pub votes: Option<(u64, u64)>,
    pub triangles: Option<u64>,
    pub has_scripts: bool,
    /// Address of a small preview image, when Roblox has one.
    pub thumbnail: Option<String>,
}

/// Preview images for store assets, by asset id.
pub async fn store_thumbnails(ids: &[u64], size: u32) -> HashMap<u64, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let joined: Vec<String> = ids.iter().map(u64::to_string).collect();
    let found = get_json(
        THUMBNAILS,
        &[
            ("assetIds", joined.join(",")),
            ("size", format!("{size}x{size}")),
            ("format", "Png".to_owned()),
        ],
    )
    .await
    .unwrap_or_default();

    found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some((
                item["targetId"].as_u64()?,
                item["imageUrl"]
                    .as_str()
                    .filter(|url| !url.is_empty())?
                    .to_owned(),
            ))
        })
        .collect()
}

/// Searches the public Creator Store. Only free assets are returned: a paid
/// one can't be inserted without buying it first.
pub async fn search_store(query: &str, kind: &str, limit: usize) -> Result<Vec<StoreItem>, String> {
    let category = store_category(kind).ok_or("Type inconnu : model, audio, decal ou mesh")?;

    let found = get_json(
        &format!("{STORE}/marketplace/{category}"),
        &[("keyword", query.to_owned()), ("limit", limit.to_string())],
    )
    .await?;
    let ids: Vec<u64> = found["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_u64())
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let joined: Vec<String> = ids.iter().map(u64::to_string).collect();
    let details_url = format!("{STORE}/items/details");
    let details_query = [("assetIds", joined.join(","))];
    let (details, thumbnails) = tokio::join!(
        get_json(&details_url, &details_query),
        store_thumbnails(&ids, 150)
    );
    let details = details?;

    Ok(details["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["fiatProduct"]["isFree"] != false)
        .filter_map(|item| {
            let asset = &item["asset"];
            let id = asset["id"].as_u64()?;
            Some(StoreItem {
                id,
                name: asset["name"].as_str().unwrap_or("?").to_owned(),
                creator: item["creator"]["name"].as_str().unwrap_or("?").to_owned(),
                verified: item["creator"]["isVerifiedCreator"] == true,
                votes: item["voting"]["upVotePercent"]
                    .as_u64()
                    .zip(item["voting"]["voteCount"].as_u64()),
                triangles: asset["modelTechnicalDetails"]["objectMeshSummary"]["triangles"]
                    .as_u64(),
                has_scripts: asset["hasScripts"] == true,
                thumbnail: thumbnails.get(&id).cloned(),
            })
        })
        .collect())
}

/// The bytes of a store asset's preview, large enough for an agent to judge
/// the asset before inserting it.
pub async fn store_preview(id: u64) -> Result<Vec<u8>, String> {
    let url = store_thumbnails(&[id], 420)
        .await
        .remove(&id)
        .ok_or("Roblox n'a pas d'aperçu pour cet asset")?;
    let image = reqwest::Client::new()
        .get(url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .bytes()
        .await
        .map_err(|error| error.to_string())?;
    Ok(image.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(name: &'static str, collection: &'static str) -> NewAsset<'static> {
        NewAsset {
            name,
            tags: vec!["nature".into()],
            class: "Model",
            instances: 4,
            collection,
        }
    }

    #[test]
    fn an_asset_id_cannot_leave_the_bank() {
        for id in ["../secret", "..", "a/b", r"a\b", "C:evil", "", "x.rbxm"] {
            assert!(read(id).is_err(), "{id}");
            assert!(thumb_path(id).is_err(), "{id}");
            assert!(remove(id).is_err(), "{id}");
            assert!(edit(id, Edit::default()).is_err(), "{id}");
        }
    }

    #[test]
    fn store_types_map_to_roblox_categories() {
        assert_eq!(store_category("model"), Some(10));
        assert_eq!(store_category("audio"), Some(3));
        assert_eq!(store_category("plugin"), None);
    }

    #[test]
    fn saving_twice_under_one_name_keeps_both() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_in(dir.path(), new("Arbre low poly", "Nature"), b"a").unwrap();
        let second = save_in(dir.path(), new("Arbre low poly", "Nature"), b"b").unwrap();
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("Arbre-low-poly", "Arbre-low-poly-2")
        );
        assert_eq!(list_in(dir.path()).len(), 2);
    }

    #[test]
    fn a_search_reads_names_tags_and_collections() {
        let dir = tempfile::tempdir().unwrap();
        save_in(dir.path(), new("Arbre", "Nature"), b"a").unwrap();
        save_in(dir.path(), new("Lampadaire", "Ville"), b"b").unwrap();
        let assets = list_in(dir.path());

        let found = |query: &str, collection: Option<&str>| -> Vec<&str> {
            assets
                .iter()
                .filter(|asset| matches(asset, query, collection))
                .map(|asset| asset.name.as_str())
                .collect()
        };
        assert_eq!(found("", None), ["Arbre", "Lampadaire"]);
        assert_eq!(found("ville", None), ["Lampadaire"]);
        assert_eq!(found("nature arbre", None), ["Arbre"]);
        assert_eq!(found("", Some("nature")), ["Arbre"]);
        assert!(found("arbre", Some("Ville")).is_empty());
    }

    #[test]
    fn a_pack_is_imported_with_the_grouping_of_its_folders() {
        let pack = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(pack.path().join("Arbres")).unwrap();
        std::fs::write(pack.path().join("Rocher.rbxm"), b"1").unwrap();
        std::fs::write(pack.path().join("Arbres/Pin.rbxmx"), b"2").unwrap();
        std::fs::write(pack.path().join("Arbres/lisez-moi.txt"), b"3").unwrap();

        let dir = tempfile::tempdir().unwrap();
        save_in(dir.path(), new("Pin", ""), b"0").unwrap();
        assert_eq!(import_in(dir.path(), pack.path(), "Pack forêt").unwrap(), 2);

        let assets = list_in(dir.path());
        let summary: Vec<(&str, &str, &str)> = assets
            .iter()
            .map(|asset| (asset.id.as_str(), asset.name.as_str(), asset.collection.as_str()))
            .collect();
        // The pack's "Pin" doesn't replace the one already in the bank.
        assert_eq!(
            summary,
            [
                ("Pin", "Pin", ""),
                ("Rocher", "Rocher", "Pack forêt"),
                ("Pin-2", "Pin", "Pack forêt / Arbres")
            ]
        );
        // Both formats are read back.
        assert_eq!(
            std::fs::read(file_in(dir.path(), "Pin-2").unwrap()).unwrap(),
            b"2"
        );
        assert!(import_in(dir.path(), &pack.path().join("absent"), "").is_err());
    }

    #[test]
    fn editing_and_removing_an_asset() {
        let dir = tempfile::tempdir().unwrap();
        let id = save_in(dir.path(), new("Arbre", ""), b"a").unwrap();
        std::fs::write(dir.path().join(format!("{id}.jpg")), b"jpg").unwrap();

        let change = Edit {
            collection: Some("Nature".into()),
            instances: Some(9),
            ..Edit::default()
        };
        edit_in(dir.path(), &id, change).unwrap();
        let asset = &list_in(dir.path())[0];
        assert_eq!(
            (asset.collection.as_str(), asset.instances, asset.name.as_str(), asset.thumb),
            ("Nature", 9, "Arbre", true)
        );
        assert!(edit_in(dir.path(), "absent", Edit::default()).is_err());

        remove_in(dir.path(), &id).unwrap();
        assert!(list_in(dir.path()).is_empty());
        assert!(!dir.path().join(format!("{id}.jpg")).exists());
    }
}
