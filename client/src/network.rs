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
pub const DEFAULT_MASTER_HTTP: &str = "http://127.0.0.1:7777";
pub const DEFAULT_MASTER_WS: &str = "ws://127.0.0.1:7777";
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
        Self {
            http_url: DEFAULT_MASTER_HTTP.to_string(),
            ws_url: DEFAULT_MASTER_WS.to_string(),
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

/// Remote-only light markers, parallel to the local `NavLightLeft` etc. in
/// `lights.rs`. Kept distinct so the local-player light systems (which query
/// by marker alone, not by parent) never pick up a remote player's lights.
#[derive(Component)] struct RemoteNavLeft;
#[derive(Component)] struct RemoteNavRight;
#[derive(Component)] struct RemoteNavTail;
#[derive(Component)] struct RemoteStrobeLeft;
#[derive(Component)] struct RemoteStrobeRight;
#[derive(Component)] struct RemoteStrobeTail;
#[derive(Component)] struct RemoteBeacon;
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
    mut status: ResMut<NetworkStatus>,
    name: Res<LocalPlayerName>,
    mut world_gen_cfg: ResMut<WorldGenConfig>,
    mut pending_reset: ResMut<PendingRunwayReset>,
    mut remotes: Query<(&RemotePlayer, &mut RemoteTarget)>,
) {
    ensure_joined(&mut status, &name.0);

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
                            target.position = Vec3::new(position.x, position.y, position.z);
                            target.rotation = Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w);
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

/// Component holding the latest server-reported state for a remote player;
/// `spawn_remote_players`/interpolation systems read from this rather than
/// snapping the Transform directly, so movement stays smooth between updates.
#[derive(Component)]
pub struct RemoteTarget {
    pub position: Vec3,
    pub rotation: Quat,
    /// Not used for ghost interpolation (which only needs position/rotation)
    /// — kept so the Multiplayer tab's teleport-to-player can hand the local
    /// aircraft a sensible starting velocity instead of snapping to zero.
    pub velocity: Vec3,
    pub control_surfaces: ControlSurfaces,
    pub lights: LightSwitches,
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
        // therefore be visual-only: strip every physics/control component
        // spawn_aircraft added, keeping just Transform/Visibility and the
        // mesh/propeller/light children.
        let root = spawn_aircraft(&mut commands, &asset_server, &cfg, position);
        commands.entity(root)
            .insert(Transform::from_translation(position).with_rotation(rotation).with_scale(Vec3::splat(0.1)))
            .insert(RemotePlayer { id: player.id, name: player.name })
            .insert(RemoteTarget {
                position,
                rotation,
                velocity,
                control_surfaces: player.control_surfaces,
                lights: player.lights,
            })
            .insert(RemotePlayerVisual)
            .insert(RemoteLightTimers::default())
            .remove::<Airplane>()
            .remove::<crate::plane::PlaneState>()
            .remove::<AircraftRoot>()
            .remove::<avian3d::prelude::RigidBody>()
            .remove::<avian3d::prelude::Mass>()
            .remove::<avian3d::prelude::AngularInertia>()
            .remove::<avian3d::prelude::LinearVelocity>()
            .remove::<avian3d::prelude::AngularVelocity>()
            .remove::<avian3d::prelude::AngularDamping>()
            .remove::<avian3d::prelude::CenterOfMass>()
            .remove::<avian3d::prelude::TransformInterpolation>();

        let light_entities = spawn_aircraft_lights(&mut commands, &cfg);
        // Replace each local light marker with the remote equivalent so the
        // local-only light-animation systems in lights.rs (which query by
        // marker with no parent filter) never touch these.
        let [nav_l, nav_r, nav_t, str_l, str_r, str_t, beacon, landing] = light_entities;
        commands.entity(nav_l).remove::<NavLightLeft>().insert(RemoteNavLeft);
        commands.entity(nav_r).remove::<NavLightRight>().insert(RemoteNavRight);
        commands.entity(nav_t).remove::<NavLightTail>().insert(RemoteNavTail);
        commands.entity(str_l).remove::<StrobeLeft>().insert(RemoteStrobeLeft);
        commands.entity(str_r).remove::<StrobeRight>().insert(RemoteStrobeRight);
        commands.entity(str_t).remove::<StrobeTail>().insert(RemoteStrobeTail);
        commands.entity(beacon).remove::<Beacon>().insert(RemoteBeacon);
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
#[allow(clippy::too_many_arguments)]
fn animate_remote_lights(
    time: Res<Time>,
    cfg: Res<FlightModelConfig>,
    mut remotes: Query<(&RemoteTarget, &mut RemoteLightTimers, &Children), With<RemotePlayerVisual>>,
    mut nav_l_q: Query<&mut PointLight, (With<RemoteNavLeft>, Without<RemoteNavRight>, Without<RemoteNavTail>, Without<RemoteStrobeLeft>, Without<RemoteStrobeRight>, Without<RemoteStrobeTail>, Without<RemoteBeacon>)>,
    mut nav_r_q: Query<&mut PointLight, (With<RemoteNavRight>, Without<RemoteNavLeft>, Without<RemoteNavTail>, Without<RemoteStrobeLeft>, Without<RemoteStrobeRight>, Without<RemoteStrobeTail>, Without<RemoteBeacon>)>,
    mut nav_t_q: Query<&mut PointLight, (With<RemoteNavTail>, Without<RemoteNavLeft>, Without<RemoteNavRight>, Without<RemoteStrobeLeft>, Without<RemoteStrobeRight>, Without<RemoteStrobeTail>, Without<RemoteBeacon>)>,
    mut str_l_q: Query<&mut PointLight, (With<RemoteStrobeLeft>, Without<RemoteNavLeft>, Without<RemoteNavRight>, Without<RemoteNavTail>, Without<RemoteStrobeRight>, Without<RemoteStrobeTail>, Without<RemoteBeacon>)>,
    mut str_r_q: Query<&mut PointLight, (With<RemoteStrobeRight>, Without<RemoteNavLeft>, Without<RemoteNavRight>, Without<RemoteNavTail>, Without<RemoteStrobeLeft>, Without<RemoteStrobeTail>, Without<RemoteBeacon>)>,
    mut str_t_q: Query<&mut PointLight, (With<RemoteStrobeTail>, Without<RemoteNavLeft>, Without<RemoteNavRight>, Without<RemoteNavTail>, Without<RemoteStrobeLeft>, Without<RemoteStrobeRight>, Without<RemoteBeacon>)>,
    mut beacon_q: Query<&mut PointLight, (With<RemoteBeacon>, Without<RemoteNavLeft>, Without<RemoteNavRight>, Without<RemoteNavTail>, Without<RemoteStrobeLeft>, Without<RemoteStrobeRight>, Without<RemoteStrobeTail>)>,
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
            if let Ok(mut l) = nav_l_q.get_mut(child) { l.intensity = nav_intensity; }
            if let Ok(mut l) = nav_r_q.get_mut(child) { l.intensity = nav_intensity; }
            if let Ok(mut l) = nav_t_q.get_mut(child) { l.intensity = nav_intensity; }
            if let Ok(mut l) = str_l_q.get_mut(child) { l.intensity = strobe_intensity; }
            if let Ok(mut l) = str_r_q.get_mut(child) { l.intensity = strobe_intensity; }
            if let Ok(mut l) = str_t_q.get_mut(child) { l.intensity = strobe_intensity; }
            if let Ok(mut l) = beacon_q.get_mut(child) { l.intensity = beacon_intensity; }
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
    timer.0 = 0.0;

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
