//! Translates player input into control-surface deflections.
//!
//! All tunable constants are read from [`FlightModelConfig`] so they can be
//! adjusted at runtime via the debug menu without a recompile.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::camera::CameraMode;

use super::aero_surface::{AeroSurface, ControlInputType};
use super::aircraft_physics::{AircraftRoot, EngineState};
use super::flight_config::FlightModelConfig;

pub fn airplane_controller(
    keys: Res<ButtonInput<KeyCode>>,
    cfg: Res<FlightModelConfig>,
    mut aircraft_q: Query<(&Children, &Transform, &mut AircraftRoot)>,
    mut surface_q: Query<&mut AeroSurface>,
    time: Res<Time>,
    camera_mode: Res<CameraMode>,
) {
    // In free-cam, WASD/QE drive the camera, so the matching attitude controls
    // are suppressed below — but throttle, mixture, the engine state machine and
    // the RPM spool must keep running regardless of which camera is active.
    let in_free_cam = *camera_mode == CameraMode::Free;
    let Ok((children, tf, mut root)) = aircraft_q.single_mut() else { return };

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

    // --- Mixture lever (L = lean / less fuel, R = rich / more fuel) ----------
    // Full rich (1.0) suits sea level; lean toward 0 as you climb. Pulling it to
    // 0 is "idle cutoff" — it starves the engine and shuts it down.
    const MIXTURE_RATE: f32 = 0.5; // lever travel per second
    if keys.pressed(KeyCode::KeyL) {
        root.mixture = (root.mixture + MIXTURE_RATE * dt).min(1.0);
    }
    if keys.pressed(KeyCode::KeyK) {
        root.mixture = (root.mixture - MIXTURE_RATE * dt).max(0.0);
    }

    // Mixture power factor: how well the current lever matches the air density.
    // The ideal lever leans with density (≈ density ratio σ from the ISA model),
    // so full rich is right at sea level and progressively too rich with
    // altitude. Off-ideal loses power; the lean side is steep (the engine can
    // quit), the rich side gentle (runs cool, never below ~25%). Below the
    // cutoff threshold there's effectively no fuel.
    const MIXTURE_CUTOFF: f32 = 0.05;   // lever below this = no fuel
    const START_MIN_MIXTURE: f32 = 0.3; // need at least this rich to catch
    let altitude = tf.translation.y;
    let density_ratio = (1.0 - 2.2557e-5 * altitude).max(0.0).powf(4.2559);
    let ideal_mixture = density_ratio.clamp(0.05, 1.0);
    let mixture_ratio = root.mixture / ideal_mixture;
    let mixture_power = if root.mixture < MIXTURE_CUTOFF {
        0.0
    } else if mixture_ratio <= 1.0 {
        (1.0 - 1.3 * (1.0 - mixture_ratio).powi(2)).max(0.0)
    } else {
        (1.0 - 0.35 * (mixture_ratio - 1.0).powi(2)).max(0.25)
    };

    // --- Engine state machine (I = engage starter) ---------------------------
    let starter = keys.pressed(KeyCode::KeyI);
    match root.engine_state {
        EngineState::Off => {
            if starter && root.mixture >= START_MIN_MIXTURE {
                root.engine_state = EngineState::Cranking;
                root.crank_timer = 0.0;
            }
        }
        EngineState::Cranking => {
            root.crank_timer += dt;
            if !starter {
                root.engine_state = EngineState::Off; // gave up before it caught
            } else if root.crank_timer >= cfg.engine_start_secs
                && root.mixture >= START_MIN_MIXTURE
            {
                root.engine_state = EngineState::Running;
            }
        }
        EngineState::Running => {
            // Starved of fuel (idle cutoff or leaned past the point it can fire).
            if mixture_power <= 0.0 {
                root.engine_state = EngineState::Off;
            }
        }
    }

    // --- Target RPM by state, then spool the live RPM toward it ---------------
    // Throttle commands idle→redline; mixture scales the achievable RPM. Both
    // thrust and the prop's spin read `engine_rps`, so cranking, catching,
    // leaning and shutdown all show up in the engine note and the propeller.
    let target_rps = match root.engine_state {
        EngineState::Off => 0.0,
        EngineState::Cranking => cfg.engine_crank_rps,
        EngineState::Running => {
            let throttle_rps = cfg.propeller.prop_idle_rps
                + root.throttle_percent * (cfg.propeller.prop_max_rps - cfg.propeller.prop_idle_rps);
            throttle_rps * mixture_power
        }
    };
    let spool_tau = if target_rps > root.engine_rps {
        cfg.engine_spool_up_tau
    } else {
        cfg.engine_spool_down_tau
    };
    let spool_alpha = 1.0 - (-dt / spool_tau.max(1e-3)).exp();
    root.engine_rps += (target_rps - root.engine_rps) * spool_alpha;

    // Attitude inputs share keys with the free camera, so they go neutral while
    // it's active (the engine/throttle handling above still runs).
    let (pitch, roll, yaw) = if in_free_cam {
        (0.0, 0.0, 0.0)
    } else {
        (
            if keys.pressed(KeyCode::KeyW) { 1.0 } else if keys.pressed(KeyCode::KeyS) { -1.0 } else { 0.0 },
            if keys.pressed(KeyCode::KeyD) { 1.0 } else if keys.pressed(KeyCode::KeyA) { -1.0 } else { 0.0 },
            if keys.pressed(KeyCode::KeyE) { 1.0 } else if keys.pressed(KeyCode::KeyQ) { -1.0 } else { 0.0 },
        )
    };

    // Flaps: notched lever like a C172 (0/10/20/30°) on the keyboard, but
    // `flap_target` itself is continuous — the instrument panel's flap lever
    // (instrument_panel.rs) can set any degree by dragging, and this only
    // reacts to just-pressed key events rather than re-snapping every frame,
    // so it doesn't fight a value the panel set mid-drag.
    const FLAP_NOTCHES_DEG: [f32; 4] = [0.0, 10.0, 20.0, 30.0];
    if keys.just_pressed(KeyCode::Period) || keys.just_pressed(KeyCode::Comma) {
        let cur_deg = root.flap_target.to_degrees();
        let nearest = FLAP_NOTCHES_DEG
            .iter()
            .position(|&n| (n - cur_deg).abs() < 0.5)
            .unwrap_or(0);
        let notch = if keys.just_pressed(KeyCode::Period) {
            (nearest + 1).min(FLAP_NOTCHES_DEG.len() - 1)
        } else {
            nearest.saturating_sub(1)
        };
        root.flap_target = FLAP_NOTCHES_DEG[notch].to_radians();
    }
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

/// Applies gentle auto-leveling torques on axes with no active pilot input.
///
/// Roll: when no A/D input, torque nudges wings toward level.
/// Pitch: when no W/S input, torque nudges nose toward level pitch.
///
/// Both corrections scale with airspeed so they are authority-proportional —
/// they fade near stall and are firm at cruise. They never fight an active
/// input: the moment a key is pressed the correction drops to zero on that axis.
pub fn flight_assist(
    keys: Res<ButtonInput<KeyCode>>,
    cfg: Res<FlightModelConfig>,
    camera_mode: Res<CameraMode>,
    mut aircraft_q: Query<Forces, With<AircraftRoot>>,
) {
    let Ok(mut forces) = aircraft_q.single_mut() else { return };

    // Suppress attitude inputs in free-cam (same gate as airplane_controller).
    if *camera_mode == CameraMode::Free {
        return;
    }

    let rot: Quat = forces.rotation().0;
    let airspeed = forces.linear_velocity().length();

    // Body axes: nose is +Z, right wing is +X, up is +Y (local).
    let world_up = Vec3::Y;
    let body_right = rot * Vec3::X; // world-space right-wing direction

    // --- Roll auto-level ---------------------------------------------------
    // Bank angle: how far the aircraft has rolled from wings-level.
    // sin(bank) = body_right · world_up — positive when right wing is high.
    //
    // Torque axis: the body's nose direction projected onto the horizontal plane.
    // Using raw body_nose (rot * Vec3::Z) breaks at high pitch because the nose
    // points nearly straight up, turning the roll correction into a spin about
    // the vertical axis — which manifests as a violent yaw snap.
    let roll_input_active = keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::KeyD);
    if !roll_input_active {
        let bank_sin = body_right.dot(world_up);
        let body_nose = rot * Vec3::Z;
        // Use the horizontal projection of the nose as the roll torque axis so the
        // correction always rolls about the aircraft's actual longitudinal axis,
        // even when the nose is pitched steeply up or down.
        let roll_axis = Vec3::new(body_nose.x, 0.0, body_nose.z).normalize_or(Vec3::Z);
        // Fade auto-level out near stall speed (~25 m/s) so it doesn't snap-roll
        // the aircraft when sideslip and dihedral geometry already couple strongly.
        let stall_fade = ((airspeed - 25.0) / 15.0).clamp(0.0, 1.0);
        let roll_torque = -cfg.auto_level_strength * bank_sin * airspeed * stall_fade * roll_axis;
        forces.apply_torque(roll_torque);
    }

    // --- Pitch stabilization -----------------------------------------------
    // Nudges the nose back toward level (pitch = 0°) on both sides — nose-up
    // and nose-down. The corrective torque is proportional to sin(pitch), so
    // it's gentle near level and stronger at extreme attitudes.
    let pitch_input_active = keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::KeyS);
    if !pitch_input_active {
        let body_nose = rot * Vec3::Z;
        let pitch_sin = body_nose.dot(world_up); // positive = nose up, negative = nose down
        // Pitch axis: right-hand rule about +X rotates +Y toward +Z (nose UP).
        // So to correct nose-up (pitch_sin > 0) we torque about -X.
        let body_pitch_axis = rot * Vec3::NEG_X;
        // Attitude correction — nudges nose toward level.
        let pitch_torque = -cfg.pitch_assist_strength * pitch_sin * airspeed * body_pitch_axis;
        // Rate damping — opposes pitch angular velocity directly, killing phugoid oscillations.
        let ang_vel: Vec3 = forces.angular_velocity();
        let pitch_rate = (rot.inverse() * ang_vel).x; // body-frame pitch rate, positive = nose up
        let rate_torque = -cfg.pitch_rate_damp * pitch_rate * airspeed * body_pitch_axis;
        forces.apply_torque(pitch_torque + rate_torque);
    }
}
