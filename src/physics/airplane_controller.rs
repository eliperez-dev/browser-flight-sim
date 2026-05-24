use bevy::prelude::*;

use super::aero_surface::{AeroSurface, ControlInputType};
use super::aircraft_physics::AircraftRoot;

const PITCH_SENSITIVITY: f32 = 0.3;
const ROLL_SENSITIVITY: f32 = 0.3;
const YAW_SENSITIVITY: f32 = 0.15;
const THROTTLE_RATE: f32 = 0.5;

pub fn airplane_controller(
    keys: Res<ButtonInput<KeyCode>>,
    mut aircraft_q: Query<(&Children, &mut AircraftRoot)>,
    mut surface_q: Query<&mut AeroSurface>,
    time: Res<Time>,
) {
    let Ok((children, mut root)) = aircraft_q.single_mut() else { return };

    let dt = time.delta_secs();

    // Throttle
    if keys.pressed(KeyCode::Equal) || keys.pressed(KeyCode::NumpadAdd) {
        root.throttle_percent = (root.throttle_percent + THROTTLE_RATE * dt).min(1.0);
    }
    if keys.pressed(KeyCode::Minus) || keys.pressed(KeyCode::NumpadSubtract) {
        root.throttle_percent = (root.throttle_percent - THROTTLE_RATE * dt).max(0.0);
    }

    let pitch = if keys.pressed(KeyCode::KeyW) { 1.0 } else if keys.pressed(KeyCode::KeyS) { -1.0 } else { 0.0 };
    let roll  = if keys.pressed(KeyCode::KeyD) { 1.0 } else if keys.pressed(KeyCode::KeyA) { -1.0 } else { 0.0 };
    let yaw   = if keys.pressed(KeyCode::KeyE) { 1.0 } else if keys.pressed(KeyCode::KeyQ) { -1.0 } else { 0.0 };

    for child in children {
        let Ok(mut surface) = surface_q.get_mut(*child) else { continue };
        if !surface.is_control_surface { continue }
        let angle = match surface.input_type {
            ControlInputType::Pitch => pitch * PITCH_SENSITIVITY * surface.input_multiplier,
            ControlInputType::Roll  => roll  * ROLL_SENSITIVITY  * surface.input_multiplier,
            ControlInputType::Yaw   => yaw   * YAW_SENSITIVITY   * surface.input_multiplier,
            ControlInputType::Flap  => 0.0,
        };
        surface.set_flap_angle(angle);
    }
}
