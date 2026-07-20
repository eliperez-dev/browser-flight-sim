mod world;
mod ws;

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, State, WebSocketUpgrade},
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use world::Registry;

/// World seed for the always-on default server. Every client generates the
/// same terrain locally from this, so it's the only "world data" that ever
/// needs to cross the wire for that world; player-created worlds get their
/// own seed from the `/create` request instead.
const DEFAULT_WORLD_SEED: u32 = 3;

#[derive(Clone)]
struct AppState {
    registry: Arc<Registry>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = "0.0.0.0:7777".parse().unwrap();
    let registry = Arc::new(Registry::default());
    registry.insert_default(DEFAULT_WORLD_SEED).await;
    tracing::info!("default world seed={DEFAULT_WORLD_SEED}");

    tokio::spawn(registry.clone().run_reaper());

    let state = AppState { registry };

    // Permissive by default so local dev (client served from a different
    // port, or wasm-server-runner) just works. Set ALLOWED_ORIGIN to the
    // real deployment's origin (e.g. "https://example.com") in production —
    // /create in particular is a write endpoint that shouldn't be callable
    // from an arbitrary origin once this is publicly reachable.
    let cors = match std::env::var("ALLOWED_ORIGIN") {
        Ok(origin) => {
            let origin: axum::http::HeaderValue = origin.parse().expect("ALLOWED_ORIGIN must be a valid header value");
            tracing::info!("CORS restricted to origin: {origin:?}");
            CorsLayer::new()
                .allow_origin(origin)
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([axum::http::header::CONTENT_TYPE])
        }
        Err(_) => {
            tracing::warn!("ALLOWED_ORIGIN not set — CORS is permissive (any origin). Set it before deploying publicly.");
            CorsLayer::permissive()
        }
    };

    let app = Router::new()
        .route("/directory", get(directory))
        .route("/create", post(create_server))
        .route("/ws/{server_id}", get(ws_upgrade))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await.expect("failed to bind");
    tracing::info!("multiplayer server listening on {addr}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("server error");
}

async fn directory(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.registry.directory().await)
}

#[derive(Deserialize)]
struct CreateServerRequest {
    seed: u32,
    #[serde(default = "default_server_name")]
    name: String,
}

fn default_server_name() -> String {
    "Player Server".to_string()
}

#[derive(Serialize)]
struct CreateServerResponse {
    id: String,
}

async fn create_server(State(state): State<AppState>, Json(req): Json<CreateServerRequest>) -> impl IntoResponse {
    let world = state.registry.create(req.seed, req.name).await;
    Json(CreateServerResponse { id: world.id.clone() })
}

async fn ws_upgrade(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let Some(world) = state.registry.get(&server_id).await else {
        return (axum::http::StatusCode::NOT_FOUND, "unknown server").into_response();
    };
    ws.on_upgrade(move |socket| ws::handle_connection(socket, peer, world, state.registry))
}
