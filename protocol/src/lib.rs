use serde::{Deserialize, Serialize};

pub type PlayerId = u32;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// Control-surface deflection (radians) and engine state, enough for another
/// client to recreate this aircraft's visual pose and animation — surface
/// angles, propeller spin, and light switches — without re-simulating flight.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ControlSurfaces {
    pub aileron: f32,
    pub elevator: f32,
    pub rudder: f32,
    pub flap: f32,
    /// Engine speed (revolutions per second), drives propeller spin rate.
    pub engine_rps: f32,
}

/// Exterior light switch state. Nav/strobe/beacon *animation* (blink phase)
/// is derived locally from each client's own elapsed time rather than
/// streamed, since it's a pure function of time — only the on/off switches
/// (set rarely, by cockpit input) need to cross the wire.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LightSwitches {
    pub nav_on: bool,
    pub strobe_on: bool,
    pub beacon_on: bool,
    pub landing_light_on: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: PlayerId,
    pub name: String,
    /// Identifies which aircraft asset to render for this player, e.g.
    /// `"low-poly-airplane"`. Only one model exists today, so every client
    /// sends the same string — this just avoids a protocol break whenever a
    /// model picker is added later.
    pub model: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub control_surfaces: ControlSurfaces,
    pub lights: LightSwitches,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientToServer {
    Join { name: String, model: String },
    StateUpdate {
        position: Vec3,
        rotation: Quat,
        velocity: Vec3,
        control_surfaces: ControlSurfaces,
        lights: LightSwitches,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerToClient {
    /// Sent once, right after a successful Join.
    Welcome {
        your_id: PlayerId,
        seed: u32,
        other_players: Vec<PlayerState>,
    },
    PlayerJoined(PlayerState),
    PlayerStateUpdate {
        id: PlayerId,
        position: Vec3,
        rotation: Quat,
        velocity: Vec3,
        control_surfaces: ControlSurfaces,
        lights: LightSwitches,
    },
    PlayerLeft { id: PlayerId },
    Kick { reason: String },
}

impl ClientToServer {
    pub fn encode(&self) -> Vec<u8> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .expect("ClientToServer encode is infallible")
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(value, _)| value)
    }
}

impl ServerToClient {
    pub fn encode(&self) -> Vec<u8> {
        bincode::serde::encode_to_vec(self, bincode::config::standard())
            .expect("ServerToClient encode is infallible")
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::serde::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(value, _)| value)
    }
}
