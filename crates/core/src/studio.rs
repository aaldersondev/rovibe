use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header::ORIGIN, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::state::{Project, Shared};

type Reply = Result<Value, String>;

/// The Studio side of every tool, sent to the plugin on each connection so
/// that updating a tool never requires restarting Studio.
const METHODS: &str = include_str!("../plugin/methods.luau");

pub struct Studio {
    pub id: u64,
    pub place_id: u64,
    pub name: String,
    pub context: String,
    outgoing: mpsc::UnboundedSender<String>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next_call: AtomicU64,
    closed: AtomicBool,
}

#[derive(Serialize)]
pub struct StudioInfo {
    pub id: u64,
    pub place_id: u64,
    pub name: String,
    pub context: String,
}

impl Studio {
    pub fn info(&self) -> StudioInfo {
        StudioInfo {
            id: self.id,
            place_id: self.place_id,
            name: self.name.clone(),
            context: self.context.clone(),
        }
    }

    pub async fn call(&self, method: &str, params: Value, timeout: Duration) -> Reply {
        let id = self.next_call.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, sender);

        let request = json!({ "id": id, "method": method, "params": params }).to_string();
        if self.outgoing.send(request).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err("Studio s'est déconnecté".into());
        }

        match tokio::time::timeout(timeout, receiver).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("Studio s'est déconnecté pendant l'appel".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!(
                    "Studio n'a pas répondu en {} s",
                    timeout.as_secs()
                ))
            }
        }
    }
}

/// Picks the Studio connection a tool call should go to.
pub fn pick(state: &Shared, project: Option<&Project>, context: &str) -> Result<Arc<Studio>, String> {
    let studios = state.studios.lock().unwrap();
    let in_context = || studios.values().filter(|studio| studio.context == context);

    // A published place is recognised by its id in every DataModel. A local
    // file only has its name, and only the editor reports it reliably.
    let place_id = project.and_then(|project| project.place_id).filter(|id| *id != 0);
    let place_name = project.and_then(|project| project.place_name.as_deref());

    let mut candidates: Vec<&Arc<Studio>> = match (place_id, place_name) {
        (Some(id), _) => in_context().filter(|studio| studio.place_id == id).collect(),
        (None, Some(name)) => {
            let named: Vec<_> = in_context().filter(|studio| studio.name == name).collect();
            if named.is_empty() && context != "edit" {
                // The server of a local file's playtest calls itself "Game":
                // any unpublished place is then a possible match, which stays
                // unambiguous as long as only one of them is being tested.
                in_context().filter(|studio| studio.place_id == 0).collect()
            } else {
                named
            }
        }
        (None, None) => in_context().collect(),
    };

    let bound_elsewhere = project.is_some_and(|project| project.is_bound())
        && candidates.is_empty()
        && in_context().next().is_some();

    match candidates.len() {
        1 => Ok(candidates.remove(0).clone()),
        0 if studios.is_empty() => Err(
            "Aucun Roblox Studio connecté. Ouvre la place dans Studio avec le plugin RoVibe Studio installé."
                .into(),
        ),
        0 if bound_elsewhere => Err(format!(
            "Ce projet est lié à la place « {} », qui n'est pas ouverte dans Studio. Ouvre-la, ou change la liaison avec le menu « Place » de l'app.",
            project.map(|project| project.binding_label()).unwrap_or_default()
        )),
        0 => Err(format!(
            "Aucun Studio en contexte `{context}`. Les contextes `server` et `client` n'existent que pendant un playtest."
        )),
        _ => Err(format!(
            "Plusieurs Studios correspondent en contexte `{context}`. Lie le projet à une place avec le menu « Place » de l'app ; si deux places jamais publiées sont testées en même temps, arrête l'un des tests."
        )),
    }
}

#[derive(Deserialize)]
pub struct Hello {
    #[serde(rename = "placeId", default)]
    place_id: u64,
    #[serde(default)]
    context: String,
    #[serde(default)]
    name: String,
}

pub async fn ws_handler(
    State(state): State<Shared>,
    Query(hello): Query<Hello>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Browsers always send Origin on a WebSocket handshake and Studio never
    // does, which keeps web pages from posing as a Studio on this
    // unauthenticated endpoint.
    if headers.contains_key(ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    upgrade.on_upgrade(move |socket| serve(state, hello, socket))
}

async fn serve(state: Shared, hello: Hello, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (outgoing, mut queued) = mpsc::unbounded_channel::<String>();

    let studio = Arc::new(Studio {
        id: state.next_id(),
        place_id: hello.place_id,
        name: hello.name,
        context: if hello.context.is_empty() {
            "edit".into()
        } else {
            hello.context
        },
        outgoing,
        pending: Mutex::default(),
        next_call: AtomicU64::new(1),
        closed: AtomicBool::new(false),
    });

    // The connection only becomes visible to tools once it can answer them.
    let loading = studio.clone();
    let registry = state.clone();
    tokio::spawn(async move {
        let loaded = loading
            .call("__load", json!({ "source": METHODS }), Duration::from_secs(20))
            .await;
        if let Err(error) = &loaded {
            crate::log::warn(format!("Studio « {} » ({}) : outils non chargés, {error}", loading.name, loading.context));
        } else {
            crate::log::info(format!("Studio « {} » connecté ({})", loading.name, loading.context));
        }
        if let Err(error) = loaded {
            registry.logs.lock().unwrap().push(
                "error".into(),
                loading.context.clone(),
                loading.place_id,
                format!("[rovibe] Outils non chargés dans Studio ({error}). Mets à jour le plugin Studio depuis l'app, puis redémarre Studio."),
            );
        }

        let mut studios = registry.studios.lock().unwrap();
        if !loading.closed.load(Ordering::Relaxed) {
            studios.insert(loading.id, loading.clone());
        }
        drop(studios);
        registry.notify();
    });

    let writer = tokio::spawn(async move {
        while let Some(text) = queued.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(message)) = stream.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        if payload["event"] == "log" {
            let mut logs = state.logs.lock().unwrap();
            for entry in payload["entries"].as_array().into_iter().flatten() {
                logs.push(
                    entry["l"].as_str().unwrap_or("output").to_owned(),
                    studio.context.clone(),
                    studio.place_id,
                    entry["m"].as_str().unwrap_or_default().to_owned(),
                );
            }
        } else if let Some(id) = payload["id"].as_u64() {
            if let Some(sender) = studio.pending.lock().unwrap().remove(&id) {
                let reply = if payload["ok"] == true {
                    Ok(payload["result"].clone())
                } else {
                    Err(payload["error"]
                        .as_str()
                        .unwrap_or("erreur inconnue")
                        .to_owned())
                };
                let _ = sender.send(reply);
            }
        }
    }

    writer.abort();
    {
        // Set under the lock so the loading task can't register the
        // connection after it has been removed.
        let mut studios = state.studios.lock().unwrap();
        studio.closed.store(true, Ordering::Relaxed);
        studios.remove(&studio.id);
        crate::log::info(format!("Studio « {} » déconnecté ({})", studio.name, studio.context));
    }
    // Dropping the senders wakes every caller still waiting on this Studio.
    studio.pending.lock().unwrap().clear();
    state.notify();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    fn state_with(studios: &[(u64, &str, &str)]) -> Shared {
        let state = Arc::new(AppState::new(0, "t".into(), std::env::temp_dir(), "0".into()));
        for (index, (place_id, name, context)) in studios.iter().enumerate() {
            let (outgoing, _) = mpsc::unbounded_channel();
            state.studios.lock().unwrap().insert(
                index as u64,
                Arc::new(Studio {
                    id: index as u64,
                    place_id: *place_id,
                    name: (*name).into(),
                    context: (*context).into(),
                    outgoing,
                    pending: Mutex::default(),
                    next_call: AtomicU64::new(1),
                    closed: AtomicBool::new(false),
                }),
            );
        }
        state
    }

    fn project(place_id: Option<u64>, place_name: Option<&str>) -> Project {
        Project {
            id: "p1".into(),
            name: "Jeu".into(),
            path: "C:/Jeux/Demo".into(),
            sync_port: 34873,
            place_id,
            place_name: place_name.map(str::to_owned),
        }
    }

    fn picked(state: &Shared, project: &Project, context: &str) -> Result<String, String> {
        pick(state, Some(project), context).map(|studio| studio.name.clone())
    }

    #[test]
    fn a_lone_studio_serves_an_unbound_project() {
        let state = state_with(&[(0, "A.rbxl", "edit")]);
        assert_eq!(picked(&state, &project(None, None), "edit").unwrap(), "A.rbxl");
    }

    #[test]
    fn two_studios_and_no_binding_is_refused_rather_than_guessed() {
        let state = state_with(&[(0, "A.rbxl", "edit"), (0, "B.rbxl", "edit")]);
        assert!(picked(&state, &project(None, None), "edit").unwrap_err().contains("Plusieurs"));
    }

    #[test]
    fn a_published_place_is_found_by_its_id_in_every_context() {
        let state = state_with(&[(111, "Mine", "edit"), (222, "Other", "edit"), (111, "Mine", "server"), (222, "Other", "server")]);
        let bound = project(Some(111), None);
        assert_eq!(picked(&state, &bound, "edit").unwrap(), "Mine");
        assert_eq!(picked(&state, &bound, "server").unwrap(), "Mine");
    }

    #[test]
    fn a_local_file_is_found_by_its_name() {
        let state = state_with(&[(0, "A.rbxl", "edit"), (0, "B.rbxl", "edit")]);
        assert_eq!(picked(&state, &project(None, Some("B.rbxl")), "edit").unwrap(), "B.rbxl");
    }

    #[test]
    fn the_test_server_of_a_local_file_is_found_although_it_is_called_game() {
        let state = state_with(&[(0, "A.rbxl", "edit"), (0, "B.rbxl", "edit"), (0, "Game", "server")]);
        assert_eq!(picked(&state, &project(None, Some("A.rbxl")), "server").unwrap(), "Game");

        // Two local places under test at once can't be told apart.
        let state = state_with(&[(0, "A.rbxl", "edit"), (0, "Game", "server"), (0, "Game", "server")]);
        assert!(picked(&state, &project(None, Some("A.rbxl")), "server").is_err());
    }

    #[test]
    fn a_project_bound_to_a_closed_place_says_so() {
        let state = state_with(&[(0, "A.rbxl", "edit")]);
        let error = picked(&state, &project(None, Some("Closed.rbxl")), "edit").unwrap_err();
        assert!(error.contains("Closed.rbxl") && error.contains("liaison"), "{error}");
    }

    #[test]
    fn test_contexts_only_exist_during_a_test() {
        let state = state_with(&[(0, "A.rbxl", "edit")]);
        assert!(picked(&state, &project(None, None), "server").unwrap_err().contains("playtest"));
        let empty = state_with(&[]);
        assert!(picked(&empty, &project(None, None), "edit").unwrap_err().contains("Aucun Roblox Studio"));
    }
}
