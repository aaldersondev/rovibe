#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_updater::UpdaterExt;

/// Looks for a newer version once, tells the UI about it, and installs it
/// when the user asks. Every failure is silent: an app that can't reach the
/// update feed must simply keep working.
async fn watch_updates(app: tauri::AppHandle, server: essaim_core::Handle) {
    let mut builder = app.updater_builder();
    // Lets a build be pointed at another feed, e.g. a local one for testing.
    // Whatever it serves still has to carry the release signature.
    if let Ok(feed) = std::env::var("ESSAIM_UPDATE_URL") {
        let Ok(feed) = feed.parse() else { return };
        let Ok(redirected) = builder.endpoints(vec![feed]) else { return };
        builder = redirected;
    }
    let Ok(updater) = builder.build() else { return };
    let Ok(Some(update)) = updater.check().await else { return };

    server.announce_update(&update.version);
    if server.install_requests().recv().await.is_err() {
        return;
    }
    if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
        app.restart();
    }
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    // When the port is taken, an Essaim server is already running (a second
    // launch, or the headless server): this window simply attaches to it.
    let server = runtime
        .block_on(essaim_core::start(essaim_core::DEFAULT_PORT, env!("CARGO_PKG_VERSION")))
        .ok();
    let url = match &server {
        Some(server) => server.url.clone(),
        None => format!("http://127.0.0.1:{}/", essaim_core::DEFAULT_PORT),
    };
    let tasks = runtime.handle().clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("Essaim")
                .inner_size(1440.0, 900.0)
                .min_inner_size(720.0, 480.0)
                .build()?;

            if let Some(server) = server {
                // An agent waiting for an answer flashes the taskbar button,
                // which is visible whatever the user is doing; it never
                // steals focus.
                let mut attention = server.attention();
                tasks.spawn(async move {
                    while attention.recv().await.is_ok() {
                        if !window.is_focused().unwrap_or(false) {
                            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
                        }
                    }
                });
                tasks.spawn(watch_updates(app.handle().clone(), server));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Essaim");
}
