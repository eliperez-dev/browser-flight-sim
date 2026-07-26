//! Multiplayer networking. WebSocket-only (the client runs as wasm in the
//! browser, which cannot open raw TCP sockets), talking the `protocol` wire
//! format to a `server` instance. Movement is client-authoritative: this
//! client simulates its own aircraft locally and just broadcasts its
//! transform, engine speed, and light switches; the server relays other
//! players' updates back to us, and we recreate their aircraft — same model,
//! same lights, same propeller animation — from that alone.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use protocol::{ClientToServer, ControlSurfaces, LightSwitches, PlayerId, PlayerState, ServerToClient};

use crate::physics::aircraft_physics::{AircraftRoot, EngineState};
use crate::physics::flight_config::FlightModelConfig;
use crate::plane::{Airplane, PlaneState, Propeller, reset_to_runway, spawn_aircraft};
use crate::lights::{Beacon, LandingLight, LightTimers, NavLightLeft, NavLightRight, NavLightTail,
    StrobeLeft, StrobeRight, StrobeTail, spawn_aircraft_lights};
use crate::terrain::{WorldGenConfig, WorldGenerator};

/// The only aircraft model that currently exists. Sent in `Join` and stamped
/// on every `PlayerState`; other clients don't act on it yet since there's
/// nothing to pick between, but it keeps the protocol stable once a model
/// picker exists.
const LOCAL_MODEL_ID: &str = "low-poly-airplane";

/// The official master/directory server, baked into the client. It hosts the
/// always-on default world (`/ws/default`) and the `/directory` + `/create`
/// HTTP endpoints the Multiplayer tab uses to browse and host servers.
/// Overridable at runtime via `MasterServer` for players who want to point
/// at a self-hosted master instead (see the Multiplayer tab's advanced
/// settings).
///
/// Points at the DigitalOcean App Platform deployment (see
/// `server/Dockerfile`, `.do/app.yaml`), which terminates TLS for us — the
/// backend itself only speaks plain HTTP/WS. A page served over https (as
/// any real portfolio embed is) cannot open a plain `ws://` connection to
/// it; browsers block that as mixed content, hence `https://`/`wss://` here
/// rather than the `http://127.0.0.1:7777` used for local dev.
pub const DEFAULT_MASTER_HTTP: &str = "https://browser-flight-sim-server-6kxx5.ondigitalocean.app";
pub const DEFAULT_MASTER_WS: &str = "wss://browser-flight-sim-server-6kxx5.ondigitalocean.app";
/// Local dev target — a `cargo run -p server` instance on its default port.
/// Debug builds default here instead of prod so the Multiplayer tab's
/// `/directory` fetch isn't rejected by the deployed server's `ALLOWED_ORIGIN`
/// CORS policy, which only allows the real portfolio origin.
const DEV_MASTER_HTTP: &str = "http://127.0.0.1:7777";
const DEV_MASTER_WS: &str = "ws://127.0.0.1:7777";
const DEFAULT_SERVER_ID: &str = "default";

/// The master server the directory/create HTTP calls target. Changing this
/// only affects the Multiplayer tab's browse/host requests, not an
/// already-open game connection.
#[derive(Resource)]
pub struct MasterServer {
    pub http_url: String,
    pub ws_url: String,
}

impl Default for MasterServer {
    fn default() -> Self {
        if cfg!(debug_assertions) {
            Self {
                http_url: DEV_MASTER_HTTP.to_string(),
                ws_url: DEV_MASTER_WS.to_string(),
            }
        } else {
            Self {
                http_url: DEFAULT_MASTER_HTTP.to_string(),
                ws_url: DEFAULT_MASTER_WS.to_string(),
            }
        }
    }
}

/// The name sent in `Join`. Owned by the Multiplayer tab's Settings pane;
/// lives here since `send_local_state`/`connect` both need to read it.
#[derive(Resource)]
pub struct LocalPlayerName(pub String);

impl Default for LocalPlayerName {
    fn default() -> Self {
        // A bare "Pilot" for every new player makes the Players list/labels
        // useless until someone bothers to rename themselves — tack on a
        // random 4-digit suffix so first-time joiners are distinguishable
        // out of the box.
        Self(format!("Pilot{:04}", rand::random_range(0..10_000)))
    }
}

/// A pending connection-lifecycle action, set by the Multiplayer tab (or
/// startup) and drained by `handle_connection_requests` next frame. A plain
/// resource rather than bevy's observer/trigger `Event` system, since this
/// is just a one-shot "do this next frame" request with no listener side —
/// exactly one system ever acts on it.
#[derive(Resource, Default)]
pub struct PendingConnectionAction(pub Option<ConnectionAction>);

pub enum ConnectionAction {
    /// (Re)connect to a specific game-world WebSocket URL, e.g.
    /// `ws://host:7777/ws/{server_id}`. Tears down any existing connection
    /// and all remote-player ghosts first.
    Connect(String),
    /// Drop the current connection without opening a new one.
    Disconnect,
}

/// How often (seconds) the local aircraft's state is broadcast to the server.
const STATE_SEND_INTERVAL: f32 = 1.0 / 20.0;

#[derive(Resource, Default)]
pub struct NetworkStatus {
    /// True once the socket is open and the Join handshake has been sent.
    pub connected: bool,
    pub your_id: Option<PlayerId>,
    /// The `ws://.../ws/{id}` URL of the currently connected (or
    /// connecting) world, if any. Cleared on disconnect.
    pub server_url: Option<String>,
}

/// Set on `Welcome` to the newly-joined world's seed; cleared once
/// `apply_pending_runway_reset` has snapped the local aircraft back to the
/// runway using that seed's terrain. Deferred (rather than resetting
/// immediately in `apply_incoming_messages`) because `WorldGenerator` — the
/// terrain-height source `reset_to_runway` needs — only catches up to a new
/// `WorldGenConfig.seed` after `regenerate_terrain`'s debounce, so resetting
/// on the same frame as `Welcome` would place the plane using the *old*
/// world's terrain height.
#[derive(Resource, Default)]
struct PendingRunwayReset(Option<u32>);

/// A remote player's identity, on their ghost aircraft's root entity.
#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
    pub name: String,
}

/// Marker for a remote player's root aircraft entity (holds `RemoteTarget`,
/// distinguishes it from the local `Airplane` for querying/despawning).
#[derive(Component)]
pub struct RemotePlayerVisual;

/// Which of a remote player's `PointLight` children this is, so
/// `animate_remote_lights` can drive them all from one query instead of one
/// per light. Nav/strobe each cover 3 physical lights (left/right/tail) that
/// always share one intensity, so the side doesn't need its own variant.
/// Kept distinct from the local `NavLightLeft` etc. markers in `lights.rs` so
/// the local-player light systems (which query by marker alone, not by
/// parent) never pick up a remote player's lights. Landing light is a
/// separate marker below since it's a `SpotLight`, not a `PointLight`.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum RemoteLightKind {
    Nav,
    Strobe,
    Beacon,
}
#[derive(Component)] struct RemoteLandingLight;

/// Per-remote-player light animation phase, mirroring `LightTimers` but
/// driven from networked switches instead of local input/engine state.
#[derive(Component, Default)]
struct RemoteLightTimers {
    strobe_t: f32,
    beacon_t: f32,
}

#[derive(Resource, Default)]
struct StateSendTimer(f32);

pub struct NetworkPlugin;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkStatus>()
            .init_resource::<StateSendTimer>()
            .init_resource::<MasterServer>()
            .init_resource::<LocalPlayerName>()
            .init_resource::<PendingConnectionAction>()
            .init_resource::<PendingRunwayReset>()
            .add_systems(Startup, connect_on_startup)
            .add_systems(
                Update,
                (
                    detect_dead_connection,
                    handle_connection_requests,
                    apply_incoming_messages,
                    spawn_remote_players,
                    despawn_remote_aero_surfaces,
                    interpolate_remote_players,
                    animate_remote_propellers,
                    animate_remote_lights,
                    apply_pending_runway_reset,
                    send_local_state,
                    despawn_stale_remote_players,
                )
                    .chain(),
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

    /// A decoded message paired with the real wall-clock time (browser
    /// `performance.now()`, milliseconds) it actually arrived in the
    /// `onmessage` callback — *not* whenever the Bevy system next happens to
    /// drain the inbox. Multiple WS messages can arrive between two Bevy
    /// frames (frame drops, tab-focus hitches, GC pauses), and draining them
    /// all in one system call would otherwise stamp them all with the same
    /// `Time::elapsed_secs()` — which is exactly what caused the interpolation
    /// jitter: bursts of same-timestamp samples followed by large gaps,
    /// instead of the real, much smaller network-level jitter.
    pub struct TimestampedMessage {
        pub arrival_perf_ms: f64,
        pub message: ServerToClient,
    }

    /// Shared inbox: the WS `onmessage` callback (JS-driven, fires outside
    /// Bevy's schedule) pushes timestamped messages here; a Bevy system
    /// drains it every frame. `Mutex` is just for `Sync`, not real
    /// contention — wasm is single-threaded.
    pub struct Connection {
        socket: WebSocket,
        pub inbox: Arc<Mutex<Vec<TimestampedMessage>>>,
        pub open: Arc<Mutex<bool>>,
        /// Set by `onclose`/`onerror`. The browser still fires these for a
        /// backgrounded tab even though Bevy's own frame loop (and thus every
        /// Bevy-side system, including whatever would otherwise notice a
        /// dead connection) is throttled/paused while hidden — so this is
        /// the only signal that survives that throttling. Polled once a
        /// frame resumes by `detect_dead_connection`, which requeues a
        /// reconnect instead of leaving `NetworkStatus` stuck reporting
        /// "connected" against a socket the server already dropped (see the
        /// server's `HEARTBEAT_TIMEOUT`).
        pub closed: Arc<Mutex<bool>>,
        // Keep the closures alive for the lifetime of the connection.
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_open: Closure<dyn FnMut()>,
        _on_close: Closure<dyn FnMut()>,
        _on_error: Closure<dyn FnMut()>,
    }

    impl Connection {
        pub fn open(url: &str) -> Result<Self, wasm_bindgen::JsValue> {
            let socket = WebSocket::new(url)?;
            socket.set_binary_type(BinaryType::Arraybuffer);

            let inbox: Arc<Mutex<Vec<TimestampedMessage>>> = Arc::new(Mutex::new(Vec::new()));
            let open = Arc::new(Mutex::new(false));
            let closed = Arc::new(Mutex::new(false));

            let inbox_cb = inbox.clone();
            let on_message = Closure::wrap(Box::new(move |event: MessageEvent| {
                if let Ok(buf) = event.data().dyn_into::<js_sys::ArrayBuffer>() {
                    let array = js_sys::Uint8Array::new(&buf);
                    let bytes = array.to_vec();
                    if let Some(message) = ServerToClient::decode(&bytes) {
                        let arrival_perf_ms = web_sys::window()
                            .and_then(|w| w.performance())
                            .map(|p| p.now())
                            .unwrap_or(0.0);
                        inbox_cb.lock().unwrap().push(TimestampedMessage { arrival_perf_ms, message });
                    }
                }
            }) as Box<dyn FnMut(MessageEvent)>);
            socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

            let open_cb = open.clone();
            let on_open = Closure::wrap(Box::new(move || {
                *open_cb.lock().unwrap() = true;
            }) as Box<dyn FnMut()>);
            socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

            let closed_cb = closed.clone();
            let on_close = Closure::wrap(Box::new(move || {
                *closed_cb.lock().unwrap() = true;
            }) as Box<dyn FnMut()>);
            socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

            let closed_cb2 = closed.clone();
            let on_error = Closure::wrap(Box::new(move || {
                *closed_cb2.lock().unwrap() = true;
            }) as Box<dyn FnMut()>);
            socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));

            Ok(Self {
                socket,
                inbox,
                open,
                closed,
                _on_message: on_message,
                _on_open: on_open,
                _on_close: on_close,
                _on_error: on_error,
            })
        }

        pub fn is_open(&self) -> bool {
            *self.open.lock().unwrap()
        }

        pub fn is_closed(&self) -> bool {
            *self.closed.lock().unwrap()
        }

        pub fn send(&self, bytes: &[u8]) {
            let _ = self.socket.send_with_u8_array(bytes);
        }

        pub fn close(&self) {
            let _ = self.socket.close();
        }
    }

    thread_local! {
        pub static CONNECTION: RefCell<Option<Connection>> = const { RefCell::new(None) };
    }
}

/// Queues the initial connection to the default master's default world,
/// once at startup; picked up by `handle_connection_requests` next frame.
fn connect_on_startup(mut pending: ResMut<PendingConnectionAction>, master: Res<MasterServer>) {
    pending.0 = Some(ConnectionAction::Connect(format!("{}/ws/{DEFAULT_SERVER_ID}", master.ws_url)));
}

/// Drains `PendingConnectionAction`: tears down any existing socket and
/// remote-player ghosts, then (for `Connect`) opens the new one. Runs before
/// `apply_incoming_messages` so a switch takes effect within the same frame
/// it's requested.
fn handle_connection_requests(
    mut commands: Commands,
    mut pending: ResMut<PendingConnectionAction>,
    mut status: ResMut<NetworkStatus>,
    remotes: Query<Entity, With<RemotePlayer>>,
) {
    let Some(action) = pending.0.take() else { return };

    // Tear down the old connection and every remote ghost regardless of
    // which action was requested — a switch is a disconnect-then-connect.
    #[cfg(target_arch = "wasm32")]
    {
        wasm_ws::CONNECTION.with(|c| {
            if let Some(conn) = c.borrow_mut().take() {
                conn.close();
            }
        });
    }
    for entity in &remotes {
        commands.entity(entity).despawn();
    }
    status.connected = false;
    status.your_id = None;
    status.server_url = None;

    let ConnectionAction::Connect(url) = action else { return };

    #[cfg(target_arch = "wasm32")]
    {
        match wasm_ws::Connection::open(&url) {
            Ok(conn) => {
                wasm_ws::CONNECTION.with(|c| *c.borrow_mut() = Some(conn));
                status.server_url = Some(url.clone());
                info!("connecting to multiplayer server at {url}");
            }
            Err(err) => {
                warn!("failed to open multiplayer connection to {url}: {err:?}");
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
        warn!("multiplayer is only available in the wasm/browser build");
    }
}

/// Notices when the live socket has actually died (server-side heartbeat
/// timeout, network drop, or — most commonly — the server dropping a
/// connection that went silent while its tab was backgrounded and Bevy's
/// own frame loop was throttled) and requeues a reconnect to the same URL.
///
/// Without this, `NetworkStatus.connected` stays stuck `true` forever once
/// the underlying socket is gone: nothing else in this module ever flips it
/// back, since every other system only reacts to messages that can no
/// longer arrive. That's what made a backgrounded-then-restored tab appear
/// "still connected" while showing no other players — the socket was dead,
/// the client just never noticed.
fn detect_dead_connection(status: Res<NetworkStatus>, mut pending: ResMut<PendingConnectionAction>) {
    if pending.0.is_some() {
        return;
    }
    let Some(url) = status.server_url.clone() else { return };

    #[cfg(target_arch = "wasm32")]
    {
        let dead = wasm_ws::CONNECTION.with(|c| c.borrow().as_ref().is_none_or(|conn| conn.is_closed()));
        if dead {
            warn!("multiplayer connection to {url} was lost — reconnecting");
            pending.0 = Some(ConnectionAction::Connect(url));
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
    }
}

/// Sends the Join handshake once the socket has finished opening. Cheap to
/// poll every frame until it succeeds; after that this is a no-op.
fn ensure_joined(status: &mut NetworkStatus, name: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if status.connected {
            return;
        }
        wasm_ws::CONNECTION.with(|c| {
            if let Some(conn) = c.borrow().as_ref() {
                if conn.is_open() {
                    let join = ClientToServer::Join {
                        name: name.to_string(),
                        model: LOCAL_MODEL_ID.to_string(),
                    };
                    conn.send(&join.encode());
                    status.connected = true;
                }
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = name;
    }
}

fn apply_incoming_messages(
    time: Res<Time>,
    mut status: ResMut<NetworkStatus>,
    name: Res<LocalPlayerName>,
    mut world_gen_cfg: ResMut<WorldGenConfig>,
    mut pending_reset: ResMut<PendingRunwayReset>,
    mut remotes: Query<(&RemotePlayer, &mut RemoteTarget)>,
) {
    ensure_joined(&mut status, &name.0);

    #[cfg(target_arch = "wasm32")]
    {
        let messages: Vec<wasm_ws::TimestampedMessage> = wasm_ws::CONNECTION.with(|c| {
            c.borrow()
                .as_ref()
                .map(|conn| std::mem::take(&mut *conn.inbox.lock().unwrap()))
                .unwrap_or_default()
        });

        // Convert each message's real `performance.now()` arrival time into
        // Bevy's clock domain. Both tick at the same real rate, so the
        // offset between "Bevy time right now" and "performance.now() right
        // now" is stable enough to apply to arrivals from earlier this same
        // frame — this is what lets messages that arrived at different real
        // moments (but got drained together) keep distinct, accurate
        // timestamps instead of all collapsing onto `time.elapsed_secs()`.
        let perf_now_ms = web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0);
        let bevy_to_perf_offset_secs = time.elapsed_secs() - (perf_now_ms as f32 / 1000.0);

        for wasm_ws::TimestampedMessage { arrival_perf_ms, message: msg } in messages {
            let arrival_bevy_secs = (arrival_perf_ms as f32 / 1000.0) + bevy_to_perf_offset_secs;
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
                    pending_reset.0 = Some(seed);
                    for player in other_players {
                        spawn_or_queue_remote(player);
                    }
                }
                ServerToClient::PlayerJoined(player) => {
                    info!("player joined: {} ({})", player.name, player.id);
                    spawn_or_queue_remote(player);
                }
                ServerToClient::PlayerStateUpdate {
                    id,
                    position,
                    rotation,
                    velocity,
                    control_surfaces,
                    lights,
                } => {
                    for (remote, mut target) in &mut remotes {
                        if remote.id == id {
                            let new_position = Vec3::new(position.x, position.y, position.z);
                            let new_rotation = Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w);
                            target.push_sample(arrival_bevy_secs, new_position, new_rotation);
                            target.velocity = Vec3::new(velocity.x, velocity.y, velocity.z);
                            target.control_surfaces = control_surfaces;
                            target.lights = lights;
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
        let _ = (&mut world_gen_cfg, &mut remotes);
    }
}

/// One timestamped position/rotation sample, as received in a
/// `PlayerStateUpdate`. `t` is `Time::elapsed_secs()` at the moment the
/// message was applied client-side (not a server timestamp — we only need
/// relative spacing between local arrivals, not cross-client sync).
#[derive(Clone, Copy)]
pub struct RemoteSample {
    pub t: f32,
    pub position: Vec3,
    pub rotation: Quat,
}

/// How far in the past `interpolate_remote_players` renders remote ghosts,
/// relative to `Time::elapsed_secs()`. Rendering at `now - RENDER_DELAY`
/// instead of assuming updates land exactly every `STATE_SEND_INTERVAL`
/// means there are (almost) always two *real* buffered samples straddling
/// the render timestamp to interpolate between, regardless of network
/// jitter — a fixed-interval lerp (position it replaced) instead held
/// perfectly still whenever an update arrived late, and snapped whenever one
/// arrived early.
///
/// Originally set to 4 intervals to paper over a since-fixed bug where
/// arrival timestamps were captured at Bevy-frame-drain time instead of
/// real WS `onmessage` time (see `wasm_ws::TimestampedMessage`), which made
/// gaps between buffered samples look far jitterier than real network
/// delivery (observed 0-476ms swings against a 50ms nominal cadence). With
/// that fixed, real observed gaps cluster in the 25-90ms range, so 3
/// intervals (150ms) still comfortably covers them with margin to spare,
/// for less input lag on remote ghosts than the original 4-interval value.
const RENDER_DELAY: f32 = STATE_SEND_INTERVAL * 3.0;

/// Component holding recent server-reported state for a remote player;
/// `spawn_remote_players`/interpolation systems read from this rather than
/// snapping the Transform directly, so movement stays smooth between
/// updates. `samples` is a small ring buffer of timestamped positions (see
/// `RENDER_DELAY`), oldest first; capped at `MAX_SAMPLES` so a long-lived
/// ghost doesn't grow the buffer forever.
#[derive(Component)]
pub struct RemoteTarget {
    /// Latest known position/rotation — used by teleport-to-player and as
    /// the interpolation target when the render timestamp runs past the
    /// newest sample (e.g. after a network stall).
    pub position: Vec3,
    pub rotation: Quat,
    pub samples: Vec<RemoteSample>,
    /// Not used for ghost interpolation (which only needs position/rotation)
    /// — kept so the Multiplayer tab's teleport-to-player can hand the local
    /// aircraft a sensible starting velocity instead of snapping to zero.
    pub velocity: Vec3,
    pub control_surfaces: ControlSurfaces,
    pub lights: LightSwitches,
}

// Needs to comfortably outlast RENDER_DELAY (3 update-intervals) so the
// buffer doesn't evict samples the render timestamp still needs; extra
// margin absorbs jitter without constantly brushing the fallback path.
const MAX_SAMPLES: usize = 10;

impl RemoteTarget {
    fn push_sample(&mut self, t: f32, position: Vec3, rotation: Quat) {
        self.position = position;
        self.rotation = rotation;
        self.samples.push(RemoteSample { t, position, rotation });
        if self.samples.len() > MAX_SAMPLES {
            self.samples.remove(0);
        }
    }

    /// Interpolated position/rotation at `render_t`, found by locating the
    /// two buffered samples that straddle it. Falls back to the newest
    /// sample if `render_t` runs past everything buffered (e.g. after a
    /// stall with no updates for a while) rather than extrapolating.
    fn sample_at(&self, render_t: f32) -> (Vec3, Quat) {
        let Some(newest) = self.samples.last() else {
            return (self.position, self.rotation);
        };
        if render_t >= newest.t {
            return (newest.position, newest.rotation);
        }
        for pair in self.samples.windows(2) {
            let [a, b] = pair else { unreachable!() };
            if render_t >= a.t && render_t <= b.t {
                let span = (b.t - a.t).max(1e-6);
                let frac = ((render_t - a.t) / span).clamp(0.0, 1.0);
                return (a.position.lerp(b.position, frac), a.rotation.slerp(b.rotation, frac));
            }
        }
        // render_t is older than every buffered sample (large stall or a
        // very fresh ghost) — hold at the oldest known sample.
        let oldest = self.samples[0];
        (oldest.position, oldest.rotation)
    }
}

thread_local! {
    static PENDING_SPAWN: std::cell::RefCell<Vec<PlayerState>> = const { std::cell::RefCell::new(Vec::new()) };
    static PENDING_REMOVAL: std::cell::RefCell<Vec<PlayerId>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn pending_removals() -> Vec<PlayerId> {
    PENDING_REMOVAL.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

fn spawn_or_queue_remote(player: PlayerState) {
    PENDING_SPAWN.with(|p| p.borrow_mut().push(player));
}

/// Spawns the real aircraft model + exterior lights for each newly-known
/// remote player, reusing the same builders the local player uses so a
/// remote pilot looks identical to a local one.
fn spawn_remote_players(
    time: Res<Time>,
    mut commands: Commands,
    existing: Query<&RemotePlayer>,
    asset_server: Res<AssetServer>,
    cfg: Res<FlightModelConfig>,
) {
    let queued = PENDING_SPAWN.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for player in queued {
        if existing.iter().any(|r| r.id == player.id) {
            continue;
        }
        let position = Vec3::new(player.position.x, player.position.y, player.position.z);
        let velocity = Vec3::new(player.velocity.x, player.velocity.y, player.velocity.z);
        let rotation = Quat::from_xyzw(
            player.rotation.x,
            player.rotation.y,
            player.rotation.z,
            player.rotation.w,
        );

        // spawn_aircraft builds the full body/propeller/aero-surface rig
        // used by the local player, including the exact components that
        // `airplane_controller`/`apply_aero_forces`/`apply_landing_gear`
        // look up with `Query<...>.single_mut()` (several of them scoped
        // only by `AircraftRoot`, not by the `Airplane` marker). A second
        // entity carrying `AircraftRoot` makes those queries ambiguous and
        // they silently return `Err` and skip — which is why, before this
        // fix, *both* aircraft lost all control and fell through the
        // terrain the moment a remote ghost spawned. A remote ghost must
        // therefore be visual-only: strip_physics_components removes every
        // physics/control component spawn_aircraft added, keeping just
        // Transform/Visibility and the mesh/propeller/light children.
        let root = spawn_aircraft(&mut commands, &asset_server, &cfg, position);
        let mut root_cmds = commands.entity(root);
        root_cmds
            .insert(Transform::from_translation(position).with_rotation(rotation).with_scale(Vec3::splat(0.1)))
            .insert(RemotePlayer { id: player.id, name: player.name })
            .insert(RemoteTarget {
                position,
                rotation,
                samples: vec![RemoteSample { t: time.elapsed_secs(), position, rotation }],
                velocity,
                control_surfaces: player.control_surfaces,
                lights: player.lights,
            })
            .insert(RemotePlayerVisual)
            .insert(RemoteLightTimers::default());
        crate::plane::strip_physics_components(&mut root_cmds);

        let light_entities = spawn_aircraft_lights(&mut commands, &cfg);
        // Replace each local light marker with the remote equivalent so the
        // local-only light-animation systems in lights.rs (which query by
        // marker with no parent filter) never touch these.
        let [nav_l, nav_r, nav_t, str_l, str_r, str_t, beacon, landing] = light_entities;
        commands.entity(nav_l).remove::<NavLightLeft>().insert(RemoteLightKind::Nav);
        commands.entity(nav_r).remove::<NavLightRight>().insert(RemoteLightKind::Nav);
        commands.entity(nav_t).remove::<NavLightTail>().insert(RemoteLightKind::Nav);
        commands.entity(str_l).remove::<StrobeLeft>().insert(RemoteLightKind::Strobe);
        commands.entity(str_r).remove::<StrobeRight>().insert(RemoteLightKind::Strobe);
        commands.entity(str_t).remove::<StrobeTail>().insert(RemoteLightKind::Strobe);
        commands.entity(beacon).remove::<Beacon>().insert(RemoteLightKind::Beacon);
        commands.entity(landing).remove::<LandingLight>().insert(RemoteLandingLight);
        commands.entity(root).add_children(&light_entities);
    }
}

/// Despawns the wing/aileron/elevator/rudder/fin `AeroSurface` children that
/// `spawn_aircraft` attached to each newly-spawned remote ghost. They only
/// exist to feed `AircraftRoot`-driven physics, which the ghost no longer
/// has (see `spawn_remote_players`), so left in place they'd just be dead
/// weight — harmless to other systems (nothing queries them without also
/// requiring `AircraftRoot`) but pointless to keep. Runs the frame after
/// spawn since the root's `Children` aren't populated until the spawn
/// command above has been applied.
fn despawn_remote_aero_surfaces(
    mut commands: Commands,
    new_remotes: Query<&Children, Added<RemotePlayerVisual>>,
    surfaces: Query<Entity, With<crate::physics::aero_surface::AeroSurface>>,
) {
    for children in &new_remotes {
        for &child in children {
            if surfaces.contains(child) {
                commands.entity(child).despawn();
            }
        }
    }
}

/// Smoothly moves each ghost aircraft's `Transform` by rendering
/// `RENDER_DELAY` seconds in the past, interpolated between whichever two
/// buffered `RemoteTarget` samples straddle that timestamp. See
/// `RENDER_DELAY`'s doc comment for why this — rather than a fixed-interval
/// lerp assuming updates land exactly every `STATE_SEND_INTERVAL` — is what
/// actually stays smooth under real network jitter.
fn interpolate_remote_players(
    time: Res<Time>,
    mut remotes: Query<(&mut Transform, &RemoteTarget), With<RemotePlayerVisual>>,
) {
    let render_t = time.elapsed_secs() - RENDER_DELAY;
    for (mut transform, target) in &mut remotes {
        let (position, rotation) = target.sample_at(render_t);
        transform.translation = position;
        transform.rotation = rotation;
    }
}

/// Spins each remote aircraft's propeller at its reported `engine_rps`,
/// mirroring the local `spin_propeller` system.
fn animate_remote_propellers(
    time: Res<Time>,
    cfg: Res<FlightModelConfig>,
    remotes: Query<(&RemoteTarget, &Children), With<RemotePlayerVisual>>,
    mut prop_q: Query<&mut Transform, With<Propeller>>,
) {
    let axis = cfg.propeller.prop_spin_axis.normalize_or(Vec3::Z);
    for (target, children) in &remotes {
        let angle = target.control_surfaces.engine_rps * std::f32::consts::TAU * time.delta_secs();
        let delta = Quat::from_axis_angle(axis, angle);
        for &child in children {
            if let Ok(mut transform) = prop_q.get_mut(child) {
                transform.rotation *= delta;
            }
        }
    }
}

/// Drives a remote player's nav/strobe/beacon/landing lights from their
/// `RemoteTarget::lights` switches, animating strobe flash and beacon pulse
/// locally from elapsed time exactly like the local-player equivalents in
/// `lights.rs` — only the on/off switch itself is networked.
fn animate_remote_lights(
    time: Res<Time>,
    cfg: Res<FlightModelConfig>,
    mut remotes: Query<(&RemoteTarget, &mut RemoteLightTimers, &Children), With<RemotePlayerVisual>>,
    mut light_q: Query<(&RemoteLightKind, &mut PointLight)>,
    mut landing_q: Query<&mut SpotLight, With<RemoteLandingLight>>,
) {
    let lc = &cfg.lights;
    for (target, mut timers, children) in &mut remotes {
        let lights = target.lights;

        timers.strobe_t = (timers.strobe_t + time.delta_secs()) % lc.strobe_period;
        let flash_on = timers.strobe_t < lc.strobe_on_time;
        let strobe_intensity = if lights.strobe_on && flash_on { lc.strobe_intensity } else { 0.0 };

        if lights.beacon_on {
            timers.beacon_t = (timers.beacon_t + time.delta_secs()) % lc.beacon_period;
        }
        let beacon_intensity = if lights.beacon_on {
            let phase = (timers.beacon_t / lc.beacon_period) * std::f32::consts::TAU;
            (phase.cos() * 0.5 + 0.5).powf(3.0) * lc.beacon_intensity
        } else {
            0.0
        };

        let nav_intensity = if lights.nav_on { lc.nav_intensity } else { 0.0 };
        let landing_intensity = if lights.landing_light_on { lc.landing_intensity } else { 0.0 };

        for &child in children {
            if let Ok((kind, mut l)) = light_q.get_mut(child) {
                l.intensity = match kind {
                    RemoteLightKind::Nav => nav_intensity,
                    RemoteLightKind::Strobe => strobe_intensity,
                    RemoteLightKind::Beacon => beacon_intensity,
                };
            }
            if let Ok(mut l) = landing_q.get_mut(child) { l.intensity = landing_intensity; }
        }
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

/// Snaps the local aircraft back to the runway once `WorldGenerator` has
/// caught up to the seed of the world we just joined (see
/// `PendingRunwayReset`'s doc comment for why this can't happen immediately
/// on `Welcome`). Runs every frame but is a no-op until that seed match
/// occurs, which is typically within one `regenerate_terrain` debounce cycle.
fn apply_pending_runway_reset(
    mut pending: ResMut<PendingRunwayReset>,
    generator: Res<WorldGenerator>,
    mut plane_q: Query<
        (&mut Transform, &mut LinearVelocity, &mut avian3d::prelude::AngularVelocity, &mut PlaneState, &mut AircraftRoot),
        With<Airplane>,
    >,
) {
    let Some(seed) = pending.0 else { return };
    if generator.seed() != seed {
        return;
    }
    if let Ok((mut transform, mut lin_vel, mut ang_vel, mut state, mut root)) = plane_q.single_mut() {
        reset_to_runway(&mut transform, &mut lin_vel, &mut ang_vel, &mut state, &mut root, &generator);
    }
    pending.0 = None;
}

/// Broadcasts the local aircraft's transform, velocity, engine speed, and
/// light switches to the server at `STATE_SEND_INTERVAL`. No-op until the
/// Join handshake has completed.
fn send_local_state(
    time: Res<Time>,
    mut timer: ResMut<StateSendTimer>,
    status: Res<NetworkStatus>,
    local: Query<(&Transform, &LinearVelocity, &AircraftRoot, &LightTimers), With<Airplane>>,
) {
    timer.0 += time.delta_secs();
    if timer.0 < STATE_SEND_INTERVAL {
        return;
    }
    // Subtract rather than zero: a frame almost never lands exactly on the
    // 50ms boundary, so it always overshoots by a few ms. Zeroing threw that
    // overshoot away every single interval, which drifted the real send
    // cadence away from STATE_SEND_INTERVAL over time — and the receiver's
    // interpolation (network.rs's RENDER_DELAY/sample_at) assumes that
    // constant is the actual cadence. Carrying the overshoot forward keeps
    // the long-run average locked to exactly 50ms instead of drifting.
    timer.0 -= STATE_SEND_INTERVAL;

    if !status.connected {
        return;
    }
    let Ok((transform, velocity, root, light_timers)) = local.single() else {
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
        control_surfaces: ControlSurfaces {
            aileron: 0.0,
            elevator: 0.0,
            rudder: root.yaw_input,
            flap: root.flap_setting,
            engine_rps: root.engine_rps,
        },
        lights: LightSwitches {
            nav_on: light_timers.nav_on,
            strobe_on: light_timers.strobe_on,
            beacon_on: root.engine_state == EngineState::Running
                || root.engine_state == EngineState::Cranking,
            landing_light_on: light_timers.landing_light_on,
        },
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
