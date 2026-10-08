//! Headless entry point: runs the server and leaves the UI to a browser.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port = std::env::var("ESSAIM_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(essaim_core::DEFAULT_PORT);

    let handle = essaim_core::start(port, env!("CARGO_PKG_VERSION")).await?;
    println!("Essaim écoute sur {}", handle.url);

    tokio::signal::ctrl_c().await?;
    Ok(())
}
