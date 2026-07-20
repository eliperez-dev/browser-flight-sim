//! HTTP client for the master server's directory/create endpoints
//! (`GET /directory`, `POST /create` — see `server/src/main.rs`). Separate
//! from `network.rs`'s WebSocket game connection: this only talks to
//! whichever `MasterServer` is configured, on-demand, from the Multiplayer
//! tab's Browse/Host panes.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::network::MasterServer;

/// One row of the master's `/directory` response — mirrors
/// `server::world::DirectoryEntry`.
#[derive(Debug, Clone, Deserialize)]
pub struct DirectoryEntry {
    pub id: String,
    pub name: String,
    pub seed: u32,
    pub player_count: usize,
    pub is_default: bool,
}

#[derive(Serialize)]
struct CreateServerRequest {
    seed: u32,
    name: String,
}

#[derive(Deserialize)]
struct CreateServerResponse {
    id: String,
}

/// Result of the most recent `/directory` fetch, drained into by
/// `poll_directory_results`. `None` until the first fetch completes.
#[derive(Resource, Default)]
pub struct DirectoryState {
    pub entries: Option<Vec<DirectoryEntry>>,
    pub last_error: Option<String>,
    pub loading: bool,
}

/// Result of the most recent `/create` request, consumed once by the Host
/// tab (e.g. to auto-connect) then cleared.
#[derive(Resource, Default)]
pub struct CreateServerState {
    pub result: Option<Result<String, String>>,
    pub loading: bool,
}

/// Set to request a `/directory` refresh; drained by `poll_directory_results`
/// (paired with the wasm fetch dispatch below).
#[derive(Resource, Default)]
pub struct DirectoryRefreshRequest(pub bool);

/// Set to request a `/create`; drained the same way.
#[derive(Resource, Default)]
pub struct CreateServerRequestQueue(pub Option<(u32, String)>);

pub struct NetworkDirectoryPlugin;

impl Plugin for NetworkDirectoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DirectoryState>()
            .init_resource::<CreateServerState>()
            .init_resource::<DirectoryRefreshRequest>()
            .init_resource::<CreateServerRequestQueue>()
            .add_systems(
                Update,
                (dispatch_directory_refresh, dispatch_create_server, poll_fetch_results),
            );
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_fetch {
    use std::cell::RefCell;

    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestInit, RequestMode, Response};

    use super::{CreateServerResponse, DirectoryEntry};

    thread_local! {
        pub static DIRECTORY_RESULT: RefCell<Option<Result<Vec<DirectoryEntry>, String>>> = const { RefCell::new(None) };
        pub static CREATE_RESULT: RefCell<Option<Result<String, String>>> = const { RefCell::new(None) };
    }

    pub fn fetch_directory(base_url: String) {
        wasm_bindgen_futures::spawn_local(async move {
            let result = fetch_json_get(&format!("{base_url}/directory")).await;
            let parsed = result.and_then(|text| {
                serde_json::from_str::<Vec<DirectoryEntry>>(&text).map_err(|e| e.to_string())
            });
            DIRECTORY_RESULT.with(|r| *r.borrow_mut() = Some(parsed));
        });
    }

    pub fn create_server(base_url: String, seed: u32, name: String) {
        wasm_bindgen_futures::spawn_local(async move {
            let body = serde_json::to_string(&super::CreateServerRequest { seed, name }).expect("infallible");
            let result = fetch_json_post(&format!("{base_url}/create"), &body).await;
            let parsed = result.and_then(|text| {
                serde_json::from_str::<CreateServerResponse>(&text)
                    .map(|r| r.id)
                    .map_err(|e| e.to_string())
            });
            CREATE_RESULT.with(|r| *r.borrow_mut() = Some(parsed));
        });
    }

    async fn fetch_json_get(url: &str) -> Result<String, String> {
        let opts = RequestInit::new();
        opts.set_method("GET");
        opts.set_mode(RequestMode::Cors);
        run_fetch(url, &opts).await
    }

    async fn fetch_json_post(url: &str, body: &str) -> Result<String, String> {
        let opts = RequestInit::new();
        opts.set_method("POST");
        opts.set_mode(RequestMode::Cors);
        opts.set_body(&wasm_bindgen::JsValue::from_str(body));
        let headers = web_sys::Headers::new().map_err(|e| format!("{e:?}"))?;
        headers.set("Content-Type", "application/json").map_err(|e| format!("{e:?}"))?;
        opts.set_headers(&headers);
        run_fetch(url, &opts).await
    }

    async fn run_fetch(url: &str, opts: &RequestInit) -> Result<String, String> {
        let request = Request::new_with_str_and_init(url, opts).map_err(|e| format!("{e:?}"))?;
        let window = web_sys::window().ok_or("no window")?;
        let resp_value = JsFuture::from(window.fetch_with_request(&request))
            .await
            .map_err(|e| format!("{e:?}"))?;
        let resp: Response = resp_value.dyn_into().map_err(|e| format!("{e:?}"))?;
        if !resp.ok() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let text_promise = resp.text().map_err(|e| format!("{e:?}"))?;
        let text_value = JsFuture::from(text_promise).await.map_err(|e| format!("{e:?}"))?;
        Ok(text_value.as_string().unwrap_or_default())
    }
}

fn dispatch_directory_refresh(
    mut request: ResMut<DirectoryRefreshRequest>,
    mut state: ResMut<DirectoryState>,
    master: Res<MasterServer>,
) {
    if !request.0 {
        return;
    }
    request.0 = false;
    state.loading = true;
    #[cfg(target_arch = "wasm32")]
    wasm_fetch::fetch_directory(master.http_url.clone());
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = master;
        state.loading = false;
        state.last_error = Some("multiplayer is only available in the wasm/browser build".to_string());
    }
}

fn dispatch_create_server(
    mut queue: ResMut<CreateServerRequestQueue>,
    mut state: ResMut<CreateServerState>,
    master: Res<MasterServer>,
) {
    let Some((seed, name)) = queue.0.take() else { return };
    state.loading = true;
    state.result = None;
    #[cfg(target_arch = "wasm32")]
    wasm_fetch::create_server(master.http_url.clone(), seed, name);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (master, seed, name);
        state.loading = false;
        state.result = Some(Err("multiplayer is only available in the wasm/browser build".to_string()));
    }
}

fn poll_fetch_results(mut dir_state: ResMut<DirectoryState>, mut create_state: ResMut<CreateServerState>) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(result) = wasm_fetch::DIRECTORY_RESULT.with(|r| r.borrow_mut().take()) {
            dir_state.loading = false;
            match result {
                Ok(entries) => {
                    dir_state.entries = Some(entries);
                    dir_state.last_error = None;
                }
                Err(err) => dir_state.last_error = Some(err),
            }
        }
        if let Some(result) = wasm_fetch::CREATE_RESULT.with(|r| r.borrow_mut().take()) {
            create_state.loading = false;
            create_state.result = Some(result);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&mut dir_state, &mut create_state);
    }
}
