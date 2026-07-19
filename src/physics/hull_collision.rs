//! Simple hull-vs-terrain collision.
//!
//! The airframe has no Avian [`Collider`] (see `landing_gear.rs` for why: a
//! rigid collider makes a stiff body bounce and tumble on any hard contact).
//! Instead this samples a handful of fixed hull points — nose, tail, and both
//! wingtips — against the same analytic terrain height field the landing gear
//! already queries (`WorldGenerator::get_terrain_height`), and treats any
//! point that dips below the ground as a crash: velocities are killed and a
//! `crashed` flag is set on [`PlaneState`] for the HUD/UI to react to.
//!
//! This is intentionally coarse — it is not a full convex-hull sweep, just
//! enough to catch "flew into a hill" or "cartwheeled on landing" cases that
//! the gear struts alone don't cover.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::aircraft_physics::AircraftRoot;
use crate::camera::{CameraMode, ChaseCam};
use crate::plane::{Airplane, PlaneState, reset_to_runway};
use crate::terrain::WorldGenerator;

/// Fixed hull sample points in the body frame (metres; +Z nose, +Y up, +X
/// right), matching the aircraft's local scale of 0.1 already used for the
/// aero-surface mounts in `plane.rs`. Coarse stand-ins for nose, tail, and
/// wingtips — enough to catch a hull strike without a full collider.
const HULL_POINTS: [Vec3; 4] = [
    Vec3::new(0.0, 0.0, 6.0),   // nose
    Vec3::new(0.0, 0.3, -6.0),  // tail (raised slightly for the fin/stabilizer)
    Vec3::new(-5.5, 0.0, 0.5),  // left wingtip
    Vec3::new(5.5, 0.0, 0.5),   // right wingtip
];

/// Detects hull strikes against terrain and applies a simple crash response.
///
/// Runs alongside `apply_landing_gear` in the same chained physics step. If
/// any hull point is found below the terrain height at its (x, z), the
/// aircraft is considered crashed: linear/angular velocity are zeroed (so it
/// stops dead rather than tunnelling or tumbling further) and `PlaneState`
/// gets `crashed = true` for the HUD/game-over UI to read. Once set, the flag
/// stays until the aircraft is respawned/reset elsewhere.
pub fn detect_hull_collision(
    mut aircraft_q: Query<
        (Forces, &mut PlaneState, &AircraftRoot),
        With<Airplane>,
    >,
    world_gen: Res<WorldGenerator>,
) {
    let Ok((mut forces, mut state, _root)) = aircraft_q.single_mut() else {
        return;
    };

    if state.crashed {
        return;
    }

    let origin: Vec3 = forces.position().0;
    let rot: Quat = forces.rotation().0;

    let struck = HULL_POINTS.iter().any(|&point| {
        let world_point = origin + rot * point;
        let ground_y = world_gen.get_terrain_height(world_point.x, world_point.z);
        world_point.y < ground_y
    });

    if struck {
        state.crashed = true;
        *forces.linear_velocity_mut() = Vec3::ZERO;
        *forces.angular_velocity_mut() = Vec3::ZERO;
    }
}

/// When a crash is first detected, drop the camera into chase mode so the
/// player keeps the wreck in view while still being able to look around.
/// `airplane_controller`/`flight_assist` already suppress attitude input in
/// `CameraMode::Chase`, so this alone takes the pilot off the controls without
/// a separate `crashed` gate on those systems. The flag stays sticky (see
/// `PlaneState::crashed`) until `reset_on_crash_key` or a UI "Reset Plane to
/// Runway" button clears it.
pub fn react_to_crash(
    mut camera_mode: ResMut<CameraMode>,
    aircraft_q: Query<&PlaneState, With<Airplane>>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&Transform, &mut ChaseCam), Without<Airplane>>,
    mut was_crashed: Local<bool>,
) {
    let Ok(state) = aircraft_q.single() else {
        return;
    };

    if state.crashed && !*was_crashed {
        // Seed the chase offset from wherever the camera currently sits (same
        // approach as the Orbit→Chase leg of `toggle_camera_mode`), so the
        // switch doesn't snap the view to some stale offset from last time
        // chase mode was used.
        if let (Ok((tf, mut chase)), Ok(plane_tf)) = (cam_query.single_mut(), plane_query.single()) {
            let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            chase.yaw = yaw;
            chase.pitch = pitch;
            chase.offset = tf.translation - plane_tf.translation;
        }
        *camera_mode = CameraMode::Chase;
    }
    *was_crashed = state.crashed;
}

/// While crashed, R resets the aircraft back to the runway (position,
/// velocity, throttle, and the `crashed` flag itself) — the keyboard
/// equivalent of the "Reset Plane to Runway" button, so recovering from a
/// crash doesn't require digging into a menu.
pub fn reset_on_crash_key(
    keys: Res<ButtonInput<KeyCode>>,
    world_gen: Res<WorldGenerator>,
    mut aircraft_q: Query<
        (&mut Transform, &mut LinearVelocity, &mut AngularVelocity, &mut PlaneState, &mut AircraftRoot),
        With<Airplane>,
    >,
) {
    if !keys.just_pressed(KeyCode::KeyR) {
        return;
    }
    let Ok((mut transform, mut lin_vel, mut ang_vel, mut state, mut root)) = aircraft_q.single_mut() else {
        return;
    };
    if !state.crashed {
        return;
    }
    reset_to_runway(&mut transform, &mut lin_vel, &mut ang_vel, &mut state, &mut root, world_gen.as_ref());
}
