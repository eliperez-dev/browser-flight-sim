use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use protocol::{ClientToServer, ControlSurfaces, PlayerId, PlayerState, ServerToClient};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{Mutex, broadcast, mpsc},
};
use tokio_tungstenite::tungstenite::Message;

/// World seed for this server instance. Every client generates the same
/// terrain locally from this, so it's the only "world data" that ever needs
/// to cross the wire.
const WORLD_SEED: u32 = 3;

struct SharedWorld {
    players: Mutex<HashMap<PlayerId, PlayerState>>,
    next_id: AtomicU32,
    start: Instant,
}

impl SharedWorld {
    fn world_time(&self) -> f32 {
        self.start.elapsed().as_secs_f32()
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr: SocketAddr = "127.0.0.1:7777".parse().unwrap();
    let listener = TcpListener::bind(addr).await.expect("failed to bind");
    tracing::info!("multiplayer server listening on {addr}, seed={WORLD_SEED}");

    let world = Arc::new(SharedWorld {
        players: Mutex::new(HashMap::new()),
        next_id: AtomicU32::new(1),
        start: Instant::now(),
    });

    // Broadcast channel: every connection task subscribes and gets a copy of
    // every message any other connection produces (fan-out relay).
    let (tx, _rx) = broadcast::channel::<(PlayerId, ServerToClient)>(1024);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!("accept failed: {err}");
                continue;
            }
        };
        let world = world.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, peer, world.clone(), tx).await {
                tracing::info!("connection {peer} closed: {err}");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    world: Arc<SharedWorld>,
    broadcast_tx: broadcast::Sender<(PlayerId, ServerToClient)>,
) -> anyhow::Result<()> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    // Wait for the Join message before allocating an id / admitting the player.
    let name = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Binary(bytes))) => {
                match ClientToServer::decode(&bytes) {
                    Some(ClientToServer::Join { name }) => break name,
                    Some(_) => continue, // ignore state updates before join
                    None => continue,
                }
            }
            Some(Ok(_)) => continue,
            Some(Err(err)) => return Err(err.into()),
            None => return Ok(()), // closed before joining
        }
    };

    let id = world.next_id.fetch_add(1, Ordering::Relaxed);
    tracing::info!("{peer} joined as '{name}' (id={id})");

    let initial_state = PlayerState {
        id,
        name,
        position: protocol::Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        rotation: protocol::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        velocity: protocol::Vec3 { x: 0.0, y: 0.0, z: 0.0 },
        control_surfaces: ControlSurfaces::default(),
    };

    let other_players: Vec<PlayerState> = {
        let mut players = world.players.lock().await;
        let others = players.values().cloned().collect();
        players.insert(id, initial_state.clone());
        others
    };

    let welcome = ServerToClient::Welcome {
        your_id: id,
        seed: WORLD_SEED,
        world_time: world.world_time(),
        other_players,
    };
    ws_tx.send(Message::Binary(welcome.encode().into())).await?;

    // Tell everyone else this player joined.
    let _ = broadcast_tx.send((id, ServerToClient::PlayerJoined(initial_state)));

    let mut broadcast_rx = broadcast_tx.subscribe();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(256);

    // Forward relayed broadcast messages (from other players) into this
    // connection's outgoing queue, skipping messages that originated here.
    let forward_out_tx = out_tx.clone();
    let forward_task = tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok((origin, msg)) if origin != id => {
                    if forward_out_tx
                        .send(Message::Binary(msg.encode().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let writer_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Read loop: incoming state updates from this player get sanity-clamped
    // and relayed to everyone else via the broadcast channel.
    let mut last_seen = Instant::now();
    while let Some(msg) = ws_rx.next().await {
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
        }) = ClientToServer::decode(&bytes)
        else {
            continue;
        };

        // Basic sanity clamp: reject updates arriving faster than a sane
        // tick rate from one connection (cheap anti-spam, not anti-cheat).
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
            }
        }

        let _ = broadcast_tx.send((
            id,
            ServerToClient::PlayerStateUpdate {
                id,
                position,
                rotation,
                velocity,
                control_surfaces,
            },
        ));
    }

    // Cleanup on disconnect.
    world.players.lock().await.remove(&id);
    let _ = broadcast_tx.send((id, ServerToClient::PlayerLeft { id }));
    forward_task.abort();
    writer_task.abort();
    tracing::info!("{peer} (id={id}) disconnected");

    Ok(())
}
