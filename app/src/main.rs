#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The window. The server that runs the agents is another process, started
//! from here when none is running: closing, crashing or updating the window
//! leaves the agents at work, and the next window finds them again.

use std::{path::PathBuf, sync::Arc, time::Duration};

use serde_json::{json, Value};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::Notify;

const PORT: u16 = rovibe_core::DEFAULT_PORT;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn address(path: &str) -> String {
    format!("http://127.0.0.1:{PORT}{path}")
}

/// The version of the RoVibe server listening on the port, if one is.
async fn ping() -> Option<String> {
    let reply: Value = reqwest::Client::new()
        .get(address("/api/ping"))
        .timeout(Duration::from_millis(1500))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    (reply["app"] == "rovibe").then(|| reply["version"].as_str().unwrap_or_default().to_owned())
}

async fn wait_for_server(up: bool, limit: Duration) -> bool {
    let until = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < until {
        if ping().await.is_some() == up {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// The server as the window's program reaches it: over HTTP, with the token
/// it left in the settings folder.
#[derive(Clone)]
struct Server {
    token: String,
    client: reqwest::Client,
}

impl Server {
    fn open() -> Option<Self> {
        let token = std::fs::read_to_string(rovibe_core::data_dir().ok()?.join("token")).ok()?;
        Some(Self { token: token.trim().to_owned(), client: reqwest::Client::new() })
    }

    async fn get(&self, path: &str, limit: Duration) -> Option<Value> {
        self.client.get(address(path)).bearer_auth(&self.token).timeout(limit).send().await.ok()?.json().await.ok()
    }

    async fn post(&self, path: &str, body: Value) -> Option<Value> {
        self.client
            .post(address(path))
            .bearer_auth(&self.token)
            .json(&body)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()
    }

    /// How many sessions are running: what stopping the server would end.
    async fn sessions(&self) -> usize {
        let state = self.get("/api/state", Duration::from_secs(5)).await.unwrap_or_default();
        state["sessions"]
            .as_array()
            .map_or(0, |sessions| sessions.iter().filter(|session| session["exited"] != true).count())
    }

    async fn stop(&self) {
        let _ = self.post("/api/shutdown", json!({})).await;
        wait_for_server(false, Duration::from_secs(8)).await;
    }
}

/// Starts the server as a process of its own. It runs from a copy kept with
/// the settings: an update can then replace the installed files while the
/// server, and the agents it runs, carry on.
fn spawn_server() -> Result<(), String> {
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .ok_or("dossier de l'app introuvable")?;
    if !installed.join("rovibe-cli.exe").exists() {
        return Err("rovibe-cli.exe manque à côté de l'app".into());
    }

    let copies = rovibe_core::data_dir().map_err(|error| error.to_string())?.join("runtime");
    let own = copies.join(VERSION);
    std::fs::create_dir_all(&own).map_err(|error| error.to_string())?;
    for name in ["rovibe-cli.exe", "rovibe-sync.exe"] {
        let (from, to) = (installed.join(name), own.join(name));
        let same = match (from.metadata(), to.metadata()) {
            (Ok(from), Ok(to)) => from.len() == to.len() && from.modified().ok() <= to.modified().ok(),
            _ => false,
        };
        if from.exists() && !same {
            std::fs::copy(&from, &to).map_err(|error| format!("{name} : {error}"))?;
        }
    }
    // Copies of other versions are of no use once nothing runs from them.
    for other in std::fs::read_dir(&copies).into_iter().flatten().flatten() {
        if other.path() != own {
            let _ = std::fs::remove_dir_all(other.path());
        }
    }

    let start = |flags: u32| {
        let mut command = std::process::Command::new(own.join("rovibe-cli.exe"));
        command
            .arg("serve")
            // The code checkers stay where the app is installed.
            .env("ROVIBE_TOOLS_DIR", installed.join("tools"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, flags);
        #[cfg(not(windows))]
        let _ = flags;
        command.spawn()
    };
    const NO_WINDOW: u32 = 0x0800_0000;
    const OWN_GROUP: u32 = 0x0000_0200;
    // Out of whatever job this program is in, so that it outlives it; where
    // that isn't allowed, it starts inside it all the same.
    const OUT_OF_JOB: u32 = 0x0100_0000;
    start(NO_WINDOW | OWN_GROUP | OUT_OF_JOB)
        .or_else(|_| start(NO_WINDOW | OWN_GROUP))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// How the server this window talks to came to be.
struct Link {
    server: Server,
    /// The server is of an older version, kept because it has sessions.
    outdated: bool,
    /// The server runs inside this program: the fallback when it couldn't be
    /// started on its own. It then ends with the window, as it used to.
    _inside: Option<rovibe_core::Handle>,
}

async fn start_server() -> Result<Link, String> {
    let mut inside = None;
    if let Err(error) = spawn_server() {
        rovibe_core::log::warn(format!("Serveur à part impossible ({error}) : il tourne dans la fenêtre"));
    }
    if !wait_for_server(true, Duration::from_secs(15)).await {
        inside = Some(rovibe_core::start(PORT, VERSION).await.map_err(|error| error.to_string())?);
    }
    let server = Server::open().ok_or("jeton du serveur introuvable")?;
    Ok(Link { server, outdated: false, _inside: inside })
}

/// Finds the running server or starts one. A server of another version is
/// replaced at once if it has nothing running, and kept otherwise.
async fn connect() -> Result<Link, String> {
    match ping().await {
        Some(version) if version == VERSION => {
            let server = Server::open().ok_or("jeton du serveur introuvable")?;
            Ok(Link { server, outdated: false, _inside: None })
        }
        Some(_) => {
            let server = Server::open().ok_or("jeton du serveur introuvable")?;
            if server.sessions().await == 0 {
                server.stop().await;
                start_server().await
            } else {
                let _ = server.post("/api/host/pending", json!({ "version": VERSION })).await;
                Ok(Link { server, outdated: true, _inside: None })
            }
        }
        None => start_server().await,
    }
}

/// Brings the window back from wherever it is: hidden in the notification
/// area, minimized, or behind other windows.
fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Stops the agents and everything else, then the window's program.
async fn quit(app: AppHandle, server: Server) {
    server.stop().await;
    app.exit(0);
}

/// Replaces the server by the one of this version, and shows it.
async fn restart_server(app: AppHandle, server: Server) {
    server.stop().await;
    if spawn_server().is_err() || !wait_for_server(true, Duration::from_secs(15)).await {
        // Starting over is the one way left to get a server.
        app.restart();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("location.reload()");
    }
}

/// Looks for a newer version once, tells the UI about it, and installs it
/// when the user asks. Every failure is silent: an app that can't reach the
/// update feed must simply keep working.
async fn watch_updates(app: AppHandle, server: Server, install: Arc<Notify>) {
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

    let _ = server.post("/api/host/update", json!({ "version": update.version })).await;
    install.notified().await;
    // Only the window is replaced and restarted. The server keeps running
    // from its copy, with its sessions, until they are done.
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => app.restart(),
        Err(error) => rovibe_core::log::warn(format!("Installation de la mise à jour {} : {error}", update.version)),
    }
}

/// Follows what the server has to say to the window's program.
async fn follow(app: AppHandle, server: Server, install: Arc<Notify>) {
    let limit = Duration::from_secs(40);
    let mut seen = loop {
        match server.get("/api/host/events", limit).await {
            Some(reply) => break reply["last"].as_u64().unwrap_or(0),
            None => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    };

    loop {
        let Some(reply) = server.get(&format!("/api/host/events?after={seen}"), limit).await else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let last = reply["last"].as_u64().unwrap_or(0);
        // A server that started again counts from the beginning.
        seen = if last < seen { 0 } else { last };

        let window = app.get_webview_window("main");
        for event in reply["events"].as_array().into_iter().flatten() {
            let text = |name: &str| event[name].as_str().unwrap_or_default().to_owned();
            let looking = window
                .as_ref()
                .is_some_and(|window| window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false));
            match text("t").as_str() {
                // An agent waiting for an answer flashes the taskbar button,
                // which is visible whatever the user is doing; it never
                // steals focus.
                "attention" if !looking => {
                    if let Some(window) = &window {
                        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Informational));
                    }
                }
                // A notification says what the flash can't: which agent,
                // and why. Only when the user isn't already looking.
                "notice" if !looking => {
                    let _ = app.notification().builder().title(text("title")).body(text("body")).show();
                }
                "window" if text("action") == "hide" => {
                    if let Some(window) = &window {
                        let _ = window.hide();
                    }
                }
                "window" => {
                    tokio::spawn(quit(app.clone(), server.clone()));
                }
                "install" => install.notify_one(),
                "restart" => {
                    tokio::spawn(restart_server(app.clone(), server.clone()));
                }
                _ => {}
            }
        }
    }
}

/// An updated app in front of an older server: the server is replaced as
/// soon as it has nothing left running, without the user having to ask.
async fn finish_update(app: AppHandle, server: Server) {
    loop {
        tokio::time::sleep(Duration::from_secs(20)).await;
        if ping().await.as_deref() == Some(VERSION) {
            return;
        }
        if server.sessions().await == 0 {
            restart_server(app, server).await;
            return;
        }
    }
}

fn main() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let link = match runtime.block_on(connect()) {
        Ok(link) => link,
        Err(error) => {
            rovibe_core::log::warn(format!("Le serveur n'a pas démarré : {error}"));
            eprintln!("RoVibe : {error}");
            return;
        }
    };
    let (server, outdated) = (link.server.clone(), link.outdated);
    let tasks = runtime.handle().clone();

    tauri::Builder::default()
        // Starting the app again while it runs in the background is how a
        // user asks for its window back.
        .plugin(tauri_plugin_single_instance::init(|app, _arguments, _folder| show(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(address("/").parse()?))
                .title("RoVibe")
                .inner_size(1440.0, 900.0)
                .min_inner_size(720.0, 480.0)
                .build()?;

            // The app's place in the notification area, where it stays while
            // agents work with the window closed.
            let open = MenuItem::with_id(app, "open", "Ouvrir RoVibe", true, None::<&str>)?;
            let stop = MenuItem::with_id(app, "quit", "Quitter et arrêter les agents", true, None::<&str>)?;
            let (quitting, runner) = (server.clone(), tasks.clone());
            let mut tray = TrayIconBuilder::with_id("rovibe")
                .tooltip("RoVibe")
                .menu(&Menu::with_items(app, &[&open, &stop])?)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "open" => show(app),
                    "quit" => {
                        runner.spawn(quit(app.clone(), quitting.clone()));
                    }
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

            // Closing the window: with sessions running, the page asks the
            // user what they meant and answers through the server; with
            // none, everything stops.
            let (closing, runner, handle, asked) = (server.clone(), tasks.clone(), app.handle().clone(), window.clone());
            window.on_window_event(move |event| {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let (server, handle, asked) = (closing.clone(), handle.clone(), asked.clone());
                    runner.spawn(async move {
                        if server.sessions().await == 0 {
                            quit(handle, server).await;
                        } else {
                            let _ = asked.eval("window.rovibeClosing && window.rovibeClosing()");
                        }
                    });
                }
            });

            let install = Arc::new(Notify::new());
            tasks.spawn(follow(app.handle().clone(), server.clone(), install.clone()));
            tasks.spawn(watch_updates(app.handle().clone(), server.clone(), install));
            if outdated {
                tasks.spawn(finish_update(app.handle().clone(), server.clone()));
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to run RoVibe")
        .run(move |_app, _event| {
            // Keeps the fallback server, if there is one, alive with the app.
            let _ = &link;
        });
}
