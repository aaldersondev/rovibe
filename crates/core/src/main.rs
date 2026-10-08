//! `rovibe-cli`: RoVibe without its window. Runs the server and leaves the
//! UI to a browser, or asks a RoVibe that is already running what it is doing.

use serde_json::Value;

const HELP: &str = "\
rovibe-cli — RoVibe sans sa fenêtre

Usage :
  rovibe-cli [serve]   lance le serveur ; l'interface s'ouvre dans un navigateur
  rovibe-cli status    projets, agents et Studios d'un RoVibe déjà lancé
  rovibe-cli version
  rovibe-cli help

ROVIBE_PORT change le port (34880 par défaut).";

fn port() -> u16 {
    std::env::var("ROVIBE_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(rovibe_core::DEFAULT_PORT)
}

async fn serve() -> anyhow::Result<()> {
    let handle = rovibe_core::start(port(), env!("CARGO_PKG_VERSION")).await?;
    println!("RoVibe écoute sur {}", handle.url);
    tokio::signal::ctrl_c().await?;
    Ok(())
}

fn names(list: &Value, describe: impl Fn(&Value) -> String) -> String {
    let lines: Vec<String> = list.as_array().into_iter().flatten().map(describe).collect();
    if lines.is_empty() {
        "  (aucun)".to_owned()
    } else {
        lines.join("\n")
    }
}

async fn status() -> anyhow::Result<()> {
    let token = std::fs::read_to_string(rovibe_core::data_dir()?.join("token"))
        .map_err(|_| anyhow::anyhow!("RoVibe n'a jamais été lancé sur ce PC"))?;
    let state: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/api/state", port()))
        .bearer_auth(token.trim())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Aucun RoVibe ne répond sur le port {}", port()))?
        .error_for_status()?
        .json()
        .await?;

    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    println!("RoVibe {}", text(&state["version"]));
    println!("Projets :\n{}", names(&state["projects"], |project| {
        let agents = state["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|session| session["project_id"] == project["id"] && session["exited"] != true)
            .count();
        format!("  {} — {} agent(s) — {}", text(&project["name"]), agents, text(&project["path"]))
    }));
    println!("Agents :\n{}", names(&state["sessions"], |session| {
        format!("  {} — {} {}", text(&session["title"]), text(&session["status"]), text(&session["detail"]))
    }));
    println!("Studios connectés :\n{}", names(&state["studios"], |studio| {
        format!("  {} ({})", text(&studio["name"]), text(&studio["context"]))
    }));
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("status") => status().await,
        Some("version" | "--version" | "-V") => {
            println!("rovibe-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => {
            eprintln!("Commande inconnue : {other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}
