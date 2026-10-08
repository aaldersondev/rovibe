#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

/// Looks for a newer version once, tells the UI about it, and installs it
/// when the user asks. Every failure is silent: an app that can't reach the
/// update feed must simply keep working.
async fn watch_updates(app: tauri::AppHandle, server: rovibe_core::Handle) {
    let mut builder = app.updater_builder();
    // Lets a build be pointed at another feed, e.g. a local one for testing.
    // Whatever it serves still has to carry the release signature.
    if let Ok(feed) = std::env::var("ROVIBE_UPDATE_URL") {
        let Ok(feed) = feed.parse() else { return };
        let Ok(redirected) = builder.endpoints(vec![feed]) else { return };
        builder = redirected;
    }
    let Ok(updater) = builder.build() else { return };
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return,
        Err(error) => {
            rovibe_core::log::warn(format!("Recherche de mise à jour impossible : {error}"));
            return;
        }
    };

    rovibe_core::log::info(format!("Mise à jour {} disponible", update.version));
    server.announce_update(&update.version);
    if server.install_requests().recv().await.is_err() {
        return;
    }
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => app.restart(),
        Err(error) => rovibe_core::log::warn(format!("Installation de la mise à jour {} : {error}", update.version)),
    }
}

/// Brings the window back from wherever it is: hidden in the notification
/// area, minimized, or behind other windows.
fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    // When the port is taken, an RoVibe server is already running (the
    // headless one): this window simply attaches to it.
    let server = runtime
        .block_on(rovibe_core::start(rovibe_core::DEFAULT_PORT, env!("CARGO_PKG_VERSION")))
        .ok();
    let url = match &server {
        Some(server) => server.url.clone(),
        None => format!("http://127.0.0.1:{}/", rovibe_core::DEFAULT_PORT),
    };
    let tasks = runtime.handle().clone();

    tauri::Builder::default()
        // Starting the app again while it runs in the background is how a
        // user asks for its window back.
        .plugin(tauri_plugin_single_instance::init(|app, _arguments, _folder| show(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("RoVibe")
                .inner_size(1440.0, 900.0)
                .min_inner_size(720.0, 480.0)
                .build()?;

            // The app's place in the notification area, where it stays while
            // agents work with the window closed.
            let open = MenuItem::with_id(app, "open", "Ouvrir RoVibe", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quitter et arrêter les agents", true, None::<&str>)?;
            let mut tray = TrayIconBuilder::with_id("rovibe")
                .tooltip("RoVibe")
                .menu(&Menu::with_items(app, &[&open, &quit])?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if let Some(server) = server {
                // Closing the window with sessions running would end them.
                // The page asks the user what they meant, and answers through
                // the server; with nothing running, closing just closes.
                let running = server.clone();
                let asked = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        if running.running_sessions() > 0 {
                            api.prevent_close();
                            let _ = asked.eval("window.rovibeClosing && window.rovibeClosing()");
                        }
                    }
                });

                let mut answers = server.window_requests();
                let handle = app.handle().clone();
                let hidden = window.clone();
                tasks.spawn(async move {
                    while let Ok(answer) = answers.recv().await {
                        match answer.as_str() {
                            "hide" => {
                                let _ = hidden.hide();
                            }
                            _ => handle.exit(0),
                        }
                    }
                });

                // An agent waiting for an answer flashes the taskbar button,
                // which is visible whatever the user is doing; it never
                // steals focus.
                let mut attention = server.attention();
                let flashing = window.clone();
                tasks.spawn(async move {
                    while attention.recv().await.is_ok() {
                        if !flashing.is_focused().unwrap_or(false) {
                            let _ = flashing.request_user_attention(Some(tauri::UserAttentionType::Informational));
                        }
                    }
                });
                // A notification says what the taskbar flash can't: which
                // agent, and why. Only when the user isn't already looking.
                let mut notices = server.notices();
                let notifier = app.handle().clone();
                let watched = window.clone();
                tasks.spawn(async move {
                    while let Ok(notice) = notices.recv().await {
                        let looking = watched.is_visible().unwrap_or(false) && watched.is_focused().unwrap_or(false);
                        if !looking {
                            let _ = notifier.notification().builder().title(&notice.title).body(&notice.body).show();
                        }
                    }
                });
                tasks.spawn(watch_updates(app.handle().clone(), server));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run RoVibe");
}
