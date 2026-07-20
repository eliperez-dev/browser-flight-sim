//! Per-connection WebSocket handling for a single joined world. Logic is
//! unchanged from the original single-world relay — only the transport
//! (axum's `WebSocket` instead of a raw `tokio-tungstenite` stream) and the
//! fact that `world` now comes from the registry lookup in `main.rs` rather
//! than being the one global.

use std::{net::SocketAddr, sync::Arc, time::Duration, time::Instant};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientToServer, ControlSurfaces, PlayerState, ServerToClient};
use tokio::sync::{broadcast, mpsc};

use crate::world::{Registry, SharedWorld};

/// Longest silence tolerated from a connection before it's treated as dead.
/// Comfortably above the client's ~50ms state-update cadence so normal jitter
/// never trips it, but short enough that a stale ghost from an abruptly
/// closed tab doesn't linger for other players.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest a single outbound write may take before the connection is
/// considered stuck. Without this, a client whose write path stalls (a
/// throttled/backgrounded tab, a congested link) blocks `writer_task`
/// forever; since `forward_task` blocks in turn on the now-full `out_tx`
/// channel, it stops draining `broadcast_rx` entirely. `HEARTBEAT_TIMEOUT`
/// alone doesn't catch this — it only watches the *inbound* read side, which
/// can keep ticking over fine while the outbound side is wedged — so this
/// stuck connection never got cleaned up: everyone else's client saw that
/// player silently stop updating (frozen), while the stuck client itself,
/// once its own heartbeat eventually lapsed, dropped out and made itself
/// vanish for everyone else. A write timeout bounds that stall so the
/// connection dies (and gets cleaned up) instead of hanging indefinitely.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn handle_connection(socket: WebSocket, peer: SocketAddr, world: Arc<SharedWorld>, registry: Arc<Registry>) {
    if let Err(err) = run(socket, peer, world.clone(), registry.clone()).await {
        tracing::info!("connection {peer} on '{}' closed: {err}", world.id);
    }
}

async fn run(socket: WebSocket, peer: SocketAddr, world: Arc<SharedWorld>, registry: Arc<Registry>) -> anyhow::Result<()> {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Wait for the Join message before allocating an id / admitting the player.
    let (name, model) = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Binary(bytes))) => match ClientToServer::decode(&bytes) {
                Some(ClientToServer::Join { name, model }) => break (name, model),
                Some(_) => continue,
                None => continue,
            },
            Some(Ok(_)) => continue,
            Some(Err(err)) => return Err(err.into()),
            None => return Ok(()),
        }
    };

    let id = world.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("{peer} joined '{}' as '{name}' (id={id}, model={model})", world.id);

    let initial_state = PlayerState {
        id,
        name,
        model,
        position: protocol::Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        rotation: protocol::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        velocity: protocol::Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        control_surfaces: ControlSurfaces::default(),
        lights: protocol::LightSwitches::default(),
    };

    let other_players: Vec<PlayerState> = {
        let mut players = world.players.lock().await;
        let others = players.values().cloned().collect();
        players.insert(id, initial_state.clone());
        others
    };
    registry.notify_membership_changed(&world).await;

    let welcome = ServerToClient::Welcome {
        your_id: id,
        seed: world.seed,
        world_time: world.world_time(),
        other_players,
    };
    ws_tx.send(Message::Binary(welcome.encode().into())).await?;

    let _ = world.broadcast_tx.send((id, ServerToClient::PlayerJoined(initial_state)));

    let mut broadcast_rx = world.broadcast_tx.subscribe();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(256);

    let forward_out_tx = out_tx.clone();
    let forward_task = tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok((origin, msg)) if origin != id => {
                    if forward_out_tx.send(Message::Binary(msg.encode().into())).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let mut writer_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            match tokio::time::timeout(WRITE_TIMEOUT, ws_tx.send(msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => break,
            }
        }
    });

    let mut last_seen = Instant::now();
    loop {
        // Race the next inbound message against the writer task dying. Without
        // this, a stuck outbound write (see `WRITE_TIMEOUT`'s doc comment)
        // only kills `writer_task`/`forward_task` — this read loop would keep
        // idling on `ws_rx.next()` for up to another `HEARTBEAT_TIMEOUT`
        // before noticing anything's wrong, during which every other client
        // still sees this player frozen in place.
        let next = tokio::select! {
            // The client sends a StateUpdate roughly every 50ms while connected
            // (see client's STATE_SEND_INTERVAL), so silence past this is either
            // a dead browser tab or a lost connection whose Close frame never
            // arrived (e.g. abrupt navigation) — treat it the same as a clean
            // disconnect rather than blocking `ws_rx.next()` forever.
            result = tokio::time::timeout(HEARTBEAT_TIMEOUT, ws_rx.next()) => match result {
                Ok(next) => next,
                Err(_) => {
                    tracing::info!("{peer} (id={id}) timed out on '{}'", world.id);
                    break;
                }
            },
            _ = &mut writer_task => {
                tracing::info!("{peer} (id={id}) writer died on '{}'", world.id);
                break;
            }
        };
        let Some(msg) = next else { break };
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };
        let bytes = match msg {
            Message::Binary(b) => b,
            Message::Close(_) => break,
            _ => continue,
        };
        let Some(ClientToServer::StateUpdate {
            position,
            rotation,
            velocity,
            control_surfaces,
            lights,
        }) = ClientToServer::decode(&bytes)
        else {
            continue;
        };

        if last_seen.elapsed() < Duration::from_millis(10) {
            continue;
        }
        last_seen = Instant::now();

        {
            let mut players = world.players.lock().await;
            if let Some(p) = players.get_mut(&id) {
                p.position = position;
                p.rotation = rotation;
                p.velocity = velocity;
                p.control_surfaces = control_surfaces;
                p.lights = lights;
            }
        }

        let _ = world.broadcast_tx.send((
            id,
            ServerToClient::PlayerStateUpdate {
                id,
                position,
                rotation,
                velocity,
                control_surfaces,
                lights,
            },
        ));
    }

    world.players.lock().await.remove(&id);
    registry.notify_membership_changed(&world).await;
    let _ = world.broadcast_tx.send((id, ServerToClient::PlayerLeft { id }));
    forward_task.abort();
    writer_task.abort();
    tracing::info!("{peer} (id={id}) disconnected from '{}'", world.id);

    Ok(())
}
