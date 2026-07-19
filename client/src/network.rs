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
use crate::plane::{Airplane, Propeller, spawn_aircraft};
use crate::lights::{Beacon, LandingLight, LightTimers, NavLightLeft, NavLightRight, NavLightTail,
    StrobeLeft, StrobeRight, StrobeTail, spawn_aircraft_lights};
use crate::terrain::WorldGenConfig;

/// The only aircraft model that currently exists. Sent in `Join` and stamped
/// on every `PlayerState`; other clients don't act on it yet since there's
/// nothing to pick between, but it keeps the protocol stable once a model
/// picker exists.
const LOCAL_MODEL_ID: &str = "low-poly-airplane";

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

/// A remote player's identity, on their ghost aircraft's root entity.
#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
    #[allow(dead_code, reason = "not shown in UI yet; kept for a future player list/nameplate")]
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
            .add_systems(Startup, connect)
            .add_systems(
                Update,
                (
                    apply_incoming_messages,
                    spawn_remote_players,
                    despawn_remote_aero_surfaces,
                    interpolate_remote_players,
                    animate_remote_propellers,
                    animate_remote_lights,
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
                        model: LOCAL_MODEL_ID.to_string(),
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
                    control_surfaces,
                    lights,
                    ..
                } => {
                    for (remote, mut target) in &mut remotes {
                        if remote.id == id {
                            target.position = Vec3::new(position.x, position.y, position.z);
                            target.rotation = Quat::from_xyzw(rotation.x, rotation.y, rotation.z, rotation.w);
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
