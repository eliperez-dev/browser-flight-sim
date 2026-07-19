//! Multiplayer networking. WebSocket-only (the client runs as wasm in the
//! browser, which cannot open raw TCP sockets), talking the `protocol` wire
//! format to a `server` instance. Movement is client-authoritative: this
//! client simulates its own aircraft locally and just broadcasts its
//! transform; the server relays other players' updates back to us.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use protocol::{ClientToServer, ControlSurfaces, PlayerId, PlayerState, ServerToClient};

use crate::plane::Airplane;
use crate::terrain::WorldGenConfig;

/// Local address for `cargo run -p server`. Only local play is wired up for
/// now; the multiplayer-tab server list will replace this with a chosen
/// address later.
const SERVER_URL: &str = "ws://127.0.0.1:7777";

/// How often (seconds) the local aircraft's state is broadcast to the server.
const STATE_SEND_INTERVAL: f32 = 1.0 / 20.0;

#[derive(Resource, Default)]
pub struct NetworkStatus {
    pub connected: bool,
    pub your_id: Option<PlayerId>,
}

/// A remote player's last-known state, used to drive their ghost aircraft.
#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
    pub name: String,
}

/// Marker so remote ghost aircraft can be spawned/despawned without touching
/// the local player's own `Airplane` entity.
#[derive(Component)]
pub struct RemotePlayerVisual;

#[derive(Resource, Default)]
struct StateSendTimer(f32);

pub struct NetworkPlugin;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkStatus>()
            .init_resource::<StateSendTimer>()
            .add_systems(Startup, connect)
            .add_systems(
                Update,
                (
                    apply_incoming_messages,
                    spawn_remote_players,
                    interpolate_remote_players,
                    send_local_state,
                    despawn_stale_remote_players,
                ),
            );
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_ws {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    use protocol::ServerToClient;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{BinaryType, MessageEvent, WebSocket};

    /// Shared inbox: the WS `onmessage` callback (JS-driven, fires outside
    /// Bevy's schedule) pushes decoded messages here; a Bevy system drains it
    /// every frame. `Mutex` is just for `Sync`, not real contention — wasm is
    /// single-threaded.
    pub struct Connection {
        socket: WebSocket,
        pub inbox: Arc<Mutex<Vec<ServerToClient>>>,
        pub open: Arc<Mutex<bool>>,
        // Keep the closures alive for the lifetime of the connection.
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_open: Closure<dyn FnMut()>,
    }

    impl Connection {
        pub fn open(url: &str) -> Result<Self, wasm_bindgen::JsValue> {
            let socket = WebSocket::new(url)?;
            socket.set_binary_type(BinaryType::Arraybuffer);

            let inbox: Arc<Mutex<Vec<ServerToClient>>> = Arc::new(Mutex::new(Vec::new()));
            let open = Arc::new(Mutex::new(false));

            let inbox_cb = inbox.clone();
            let on_message = Closure::wrap(Box::new(move |event: MessageEvent| {
                if let Ok(buf) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                    let array = js_sys::Uint8Array::new(&buf);
                    let bytes = array.to_vec();
                    if let Some(msg) = ServerToClient::decode(&bytes) {
                        inbox_cb.lock().unwrap().push(msg);
                    }
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

            let open_cb = open.clone();
            let on_open = Closure::wrap(Box::new(move || {
                *open_cb.lock().unwrap() = true;
            }) as Box<dyn FnMut()>);
            socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

            Ok(Self {
                socket,
                inbox,
                open,
                _on_message: on_message,
                _on_open: on_open,
            })
        }

        pub fn is_open(&self) -> bool {
            *self.open.lock().unwrap()
        }

        pub fn send(&self, bytes: &[u8]) {
            let _ = self.socket.send_with_u8_array(bytes);
        }
    }

    thread_local! {
        pub static CONNECTION: RefCell<Option<Connection>> = const { RefCell::new(None) };
    }
}

fn connect(mut status: ResMut<NetworkStatus>) {
    #[cfg(target_arch = "wasm32")]
    {
        match wasm_ws::Connection::open(SERVER_URL) {
            Ok(conn) => {
                wasm_ws::CONNECTION.with(|c| *c.borrow_mut() = Some(conn));
                info!("connecting to multiplayer server at {SERVER_URL}");
            }
            Err(err) => {
                warn!("failed to open multiplayer connection: {err:?}");
            }
        }
        status.connected = false;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = &mut status;
        warn!("multiplayer is only available in the wasm/browser build");
    }
}

/// Sends the Join handshake once the socket has finished opening. Cheap to
/// poll every frame until it succeeds; after that this is a no-op.
fn ensure_joined(status: &mut NetworkStatus) {
    #[cfg(target_arch = "wasm32")]
    {
        if status.connected {
            return;
        }
        wasm_ws::CONNECTION.with(|c| {
            if let Some(conn) = c.borrow().as_ref() {
                if conn.is_open() {
                    let join = ClientToServer::Join {
                        name: "Pilot".to_string(),
                    };
                    conn.send(&join.encode());
                    status.connected = true;
                }
            }
        });
    }
}

fn apply_incoming_messages(
    mut status: ResMut<NetworkStatus>,
    mut world_gen_cfg: ResMut<WorldGenConfig>,
    mut remotes: Query<(&RemotePlayer, &mut RemoteTarget)>,
    mut commands: Commands,
) {
    ensure_joined(&mut status);

    #[cfg(target_arch = "wasm32")]
    {
        let messages: Vec<ServerToClient> = wasm_ws::CONNECTION.with(|c| {
            c.borrow()
                .as_ref()
                .map(|conn| std::mem::take(&mut *conn.inbox.lock().unwrap()))
                .unwrap_or_default()
        });

        for msg in messages {
            match msg {
                ServerToClient::Welcome {
                    your_id,
                    seed,
                    other_players,
                    ..
                } => {
                    info!("joined server: id={your_id}, seed={seed}, {} other players", other_players.len());
                    status.your_id = Some(your_id);
                    if world_gen_cfg.seed != seed {
                        world_gen_cfg.seed = seed;
                    }
                    for player in other_players {
                        spawn_or_queue_remote(&mut commands, player);
                    }
                }
                ServerToClient::PlayerJoined(player) => {
                    info!("player joined: {} ({})", player.name, player.id);
                    spawn_or_queue_remote(&mut commands, player);
                }
                ServerToClient::PlayerStateUpdate {
                    id,
                    position,
                    rotation,
                    control_surfaces,
                    ..
                } => {
                    for (remote, mut target) in &mut remotes {
                        if remote.id == id {
                            target.position = Vec3::new(position.x, position.y, position.z);
                            target.rotation = Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w);
                            target.control_surfaces = control_surfaces;
                            target.stale_timer = 0.0;
                        }
                    }
                }
                ServerToClient::PlayerLeft { id } => {
                    info!("player left: {id}");
                    PENDING_REMOVAL.with(|p| p.borrow_mut().push(id));
                }
                ServerToClient::Kick { reason } => {
                    warn!("kicked from server: {reason}");
                    status.connected = false;
                }
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (&mut world_gen_cfg, &mut remotes, &mut commands);
    }
}

/// Component holding the latest server-reported state for a remote player;
/// `spawn_remote_players`/interpolation systems read from this rather than
/// snapping the Transform directly, so movement stays smooth between updates.
#[derive(Component)]
pub struct RemoteTarget {
    pub position: Vec3,
    pub rotation: Quat,
    pub control_surfaces: ControlSurfaces,
    pub stale_timer: f32,
}

thread_local! {
    static PENDING_SPAWN: std::cell::RefCell<Vec<PlayerState>> = const { std::cell::RefCell::new(Vec::new()) };
    static PENDING_REMOVAL: std::cell::RefCell<Vec<PlayerId>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn pending_removals() -> Vec<PlayerId> {
    PENDING_REMOVAL.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

fn spawn_or_queue_remote(_commands: &mut Commands, player: PlayerState) {
    PENDING_SPAWN.with(|p| p.borrow_mut().push(player));
}

/// Spawns a simple placeholder capsule for each newly-known remote player.
/// Kept intentionally minimal for v1 — swapping in the real aircraft model
/// is a follow-up, not a networking concern.
fn spawn_remote_players(
    mut commands: Commands,
    existing: Query<&RemotePlayer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let queued = PENDING_SPAWN.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for player in queued {
        if existing.iter().any(|r| r.id == player.id) {
            continue;
        }
        commands.spawn((
            RemotePlayer {
                id: player.id,
                name: player.name,
            },
            RemoteTarget {
                position: Vec3::new(player.position.x, player.position.y, player.position.z),
                rotation: Quat::from_xyzw(
                    player.rotation.x,
                    player.rotation.y,
                    player.rotation.z,
                    player.rotation.w,
                ),
                control_surfaces: player.control_surfaces,
                stale_timer: 0.0,
            },
            Transform::from_xyz(player.position.x, player.position.y, player.position.z),
            GlobalTransform::default(),
            Visibility::default(),
            RemotePlayerVisual,
            Mesh3d(meshes.add(Capsule3d::new(0.6, 3.0))),
            MeshMaterial3d(materials.add(Color::srgb(0.9, 0.2, 0.2))),
        ));
    }
}

/// Smoothly moves each ghost aircraft's `Transform` toward its latest
/// `RemoteTarget`, since state updates only arrive at `STATE_SEND_INTERVAL`
/// rather than every frame.
fn interpolate_remote_players(
    time: Res<Time>,
    mut remotes: Query<(&mut Transform, &RemoteTarget), With<RemotePlayerVisual>>,
) {
    let t = (time.delta_secs() / STATE_SEND_INTERVAL).clamp(0.0, 1.0);
    for (mut transform, target) in &mut remotes {
        transform.translation = transform.translation.lerp(target.position, t);
        transform.rotation = transform.rotation.slerp(target.rotation, t);
    }
}

fn despawn_stale_remote_players(mut commands: Commands, remotes: Query<(Entity, &RemotePlayer)>) {
    let removals = pending_removals();
    if removals.is_empty() {
        return;
    }
    for (entity, remote) in &remotes {
        if removals.contains(&remote.id) {
            commands.entity(entity).despawn();
        }
    }
}

/// Broadcasts the local aircraft's transform/velocity to the server at
/// `STATE_SEND_INTERVAL`. No-op until the Join handshake has completed.
fn send_local_state(
    time: Res<Time>,
    mut timer: ResMut<StateSendTimer>,
    status: Res<NetworkStatus>,
    local: Query<(&Transform, &LinearVelocity), With<Airplane>>,
) {
    timer.0 += time.delta_secs();
    if timer.0 < STATE_SEND_INTERVAL {
        return;
    }
    timer.0 = 0.0;

    if !status.connected {
        return;
    }
    let Ok((transform, velocity)) = local.single() else {
        return;
    };

    let update = ClientToServer::StateUpdate {
        position: protocol::Vec3 {
            x: transform.translation.x,
            y: transform.translation.y,
            z: transform.translation.z,
        },
        rotation: protocol::Quat {
            x: transform.rotation.x,
            y: transform.rotation.y,
            z: transform.rotation.z,
            w: transform.rotation.w,
        },
        velocity: protocol::Vec3 {
            x: velocity.x,
            y: velocity.y,
            z: velocity.z,
        },
        control_surfaces: ControlSurfaces::default(),
    };

    #[cfg(target_arch = "wasm32")]
    {
        wasm_ws::CONNECTION.with(|c| {
            if let Some(conn) = c.borrow().as_ref() {
                conn.send(&update.encode());
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = update;
    }
}
