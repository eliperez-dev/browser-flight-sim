//! Translates player input into control-surface deflections.
//!
//! All tunable constants are read from [`FlightModelConfig`] so they can be
//! adjusted at runtime via the debug menu without a recompile.

use bevy::prelude::*;

use crate::camera::CameraMode;

use super::aero_surface::{AeroSurface, ControlInputType};
use super::aircraft_physics::AircraftRoot;
use super::flight_config::FlightModelConfig;

pub fn airplane_controller(
    keys: Res<ButtonInput<KeyCode>>,
    cfg: Res<FlightModelConfig>,
    mut aircraft_q: Query<(&Children, &mut AircraftRoot)>,
    mut surface_q: Query<&mut AeroSurface>,
    time: Res<Time>,
    camera_mode: Res<CameraMode>,
) {
    if *camera_mode == CameraMode::Free {
        return;
    }
    let Ok((children, mut root)) = aircraft_q.single_mut() else { return };

    let dt = time.delta_secs();

    // Note: control inputs command a fixed *deflection angle*, not a speed-scaled
    // one. The surfaces are real aero surfaces whose force already scales with
    // dynamic pressure q = ½ρv², so they naturally go mushy at low speed and
    // bite hard at high speed — no separate "authority" gain (that double-counted
    // speed). The only limit is the physical deflection clamp in set_flap_angle.

    // Throttle
    if keys.pressed(KeyCode::Equal) || keys.pressed(KeyCode::NumpadAdd) {
        root.throttle_percent = (root.throttle_percent + cfg.throttle_rate * dt).min(1.0);
    }
    if keys.pressed(KeyCode::Minus) || keys.pressed(KeyCode::NumpadSubtract) {
        root.throttle_percent = (root.throttle_percent - cfg.throttle_rate * dt).max(0.0);
    }

    let pitch = if keys.pressed(KeyCode::KeyW) { 1.0 } else if keys.pressed(KeyCode::KeyS) { -1.0 } else { 0.0 };
    let roll  = if keys.pressed(KeyCode::KeyD) { 1.0 } else if keys.pressed(KeyCode::KeyA) { -1.0 } else { 0.0 };
    let yaw   = if keys.pressed(KeyCode::KeyE) { 1.0 } else if keys.pressed(KeyCode::KeyQ) { -1.0 } else { 0.0 };

    // Flaps: notched lever like a C172 (0/10/20/30°). Period (>) extends a
    // notch, Comma (<) retracts. The commanded notch is `flap_target`; the
    // actual `flap_setting` chases it at a finite rate so flaps don't snap.
    const FLAP_NOTCHES_DEG: [f32; 4] = [0.0, 10.0, 20.0, 30.0];
    let cur_deg = root.flap_target.to_degrees();
    let mut notch = FLAP_NOTCHES_DEG
        .iter()
        .position(|&n| (n - cur_deg).abs() < 0.5)
        .unwrap_or(0);
    if keys.just_pressed(KeyCode::Period) {
        notch = (notch + 1).min(FLAP_NOTCHES_DEG.len() - 1);
    }
    if keys.just_pressed(KeyCode::Comma) {
        notch = notch.saturating_sub(1);
    }
    root.flap_target = FLAP_NOTCHES_DEG[notch].to_radians();
    let flap_rate = 15_f32.to_radians(); // flap travel speed (rad/s)
    let flap_step = (root.flap_target - root.flap_setting).clamp(-flap_rate * dt, flap_rate * dt);
    root.flap_setting += flap_step;
    let flap_setting = root.flap_setting;

    // First-order lag coefficient for the servo animation.
    let alpha = 1.0 - (-dt / cfg.servo_tau).exp();

    for child in children {
        let Ok(mut surface) = surface_q.get_mut(*child) else { continue };
        if !surface.is_control_surface { continue }
        let target = match surface.input_type {
            ControlInputType::Pitch => pitch * cfg.pitch_sensitivity * surface.input_multiplier + cfg.elevator_trim,
            ControlInputType::Roll  => roll  * cfg.roll_sensitivity  * surface.input_multiplier,
            // Rudder gets a small coordinated-turn mix from the ailerons.
            // Kept light: a large mix makes every roll input swing the nose,
            // which reads as the aircraft pivoting about a point ahead of it.
            ControlInputType::Yaw   => (yaw + roll * 0.2) * cfg.yaw_sensitivity * surface.input_multiplier,
            // Flaps deflect symmetrically to the commanded setting (no speed
            // authority scaling — they're a configuration, not a flight control).
            ControlInputType::Flap  => flap_setting * surface.input_multiplier,
        };
        let new_angle = surface.flap_angle + (target - surface.flap_angle) * alpha;
        surface.set_flap_angle(new_angle);
    }
}
