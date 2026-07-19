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

/// Control-surface deflection, enough to render another player's aircraft
/// looking roughly correct instead of just an interpolated rigid hull.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ControlSurfaces {
    pub aileron: f32,
    pub elevator: f32,
    pub rudder: f32,
    pub flap: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: PlayerId,
    pub name: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub control_surfaces: ControlSurfaces,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientToServer {
    Join { name: String },
    StateUpdate {
        position: Vec3,
        rotation: Quat,
        velocity: Vec3,
        control_surfaces: ControlSurfaces,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerToClient {
    /// Sent once, right after a successful Join.
    Welcome {
        your_id: PlayerId,
        seed: u32,
        world_time: f32,
        other_players: Vec<PlayerState>,
    },
    PlayerJoined(PlayerState),
    PlayerStateUpdate {
        id: PlayerId,
        position: Vec3,
        rotation: Quat,
        velocity: Vec3,
        control_surfaces: ControlSurfaces,
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
