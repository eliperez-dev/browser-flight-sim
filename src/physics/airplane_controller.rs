use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;

use super::aero_surface::{AeroSurface, ControlInputType};
use super::aircraft_physics::AircraftRoot;

const PITCH_SENSITIVITY: f32 = 0.65;
const ROLL_SENSITIVITY: f32 = 0.45;
const YAW_SENSITIVITY: f32 = 0.30;
const THROTTLE_RATE: f32 = 0.5;
// Servo time constant in seconds. Smaller = snappier, larger = more sluggish.
// 0.12 s gives a fast but clearly animated servo feel.
const SERVO_TAU: f32 = 0.12;

// Vs (clean stall): 55 kts
const STALL_SPEED: f32 = 28.0;
// Vno (max structural cruise): 129 kts — above here deflection ramps down.
const AUTHORITY_LIMIT_SPEED: f32 = 66.0;
// Vne (never exceed): 163 kts — full authority reduction is reached here.
const VNE_SPEED: f32 = 84.0;
// Authority fraction at Vne and above.
const VNE_AUTHORITY: f32 = 0.35;
// Static elevator trim (radians).
const ELEVATOR_TRIM: f32 = 0.0;

pub fn airplane_controller(
    keys: Res<ButtonInput<KeyCode>>,
    mut aircraft_q: Query<(&Children, &mut AircraftRoot, &LinearVelocity)>,
    mut surface_q: Query<&mut AeroSurface>,
    time: Res<Time>,
) {
    let Ok((children, mut root, lin_vel)) = aircraft_q.single_mut() else { return };

    let dt = time.delta_secs();
    let speed = lin_vel.0.length();

    // Low-speed: authority ramps 0→1 between stall and limit speed.
    // High-speed: authority ramps 1→VNE_AUTHORITY above limit speed.
    let authority = if speed < STALL_SPEED {
        speed / STALL_SPEED
    } else if speed < AUTHORITY_LIMIT_SPEED {
        1.0
    } else {
        let t = ((speed - AUTHORITY_LIMIT_SPEED) / (VNE_SPEED - AUTHORITY_LIMIT_SPEED)).clamp(0.0, 1.0);
        1.0 - t * (1.0 - VNE_AUTHORITY)
    };

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

    let alpha = 1.0 - (-dt / SERVO_TAU).exp();

    for child in children {
        let Ok(mut surface) = surface_q.get_mut(*child) else { continue };
        if !surface.is_control_surface { continue }
        let target = match surface.input_type {
            ControlInputType::Pitch => pitch * PITCH_SENSITIVITY * surface.input_multiplier * authority + ELEVATOR_TRIM,
            ControlInputType::Roll  => roll  * ROLL_SENSITIVITY  * surface.input_multiplier * authority,
            ControlInputType::Yaw   => (yaw + roll * 0.35) * YAW_SENSITIVITY * surface.input_multiplier * authority,
            ControlInputType::Flap  => 0.0,
        };
        let new_angle = surface.flap_angle + (target - surface.flap_angle) * alpha;
        surface.set_flap_angle(new_angle);
    }
}
