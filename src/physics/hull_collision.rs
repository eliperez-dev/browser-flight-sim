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
use crate::plane::{Airplane, PlaneState};
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
