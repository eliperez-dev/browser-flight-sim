//! Applies aerodynamic forces and thrust to the rigid body each physics step.
//!
//! All tunable constants are read from [`FlightModelConfig`] so they can be
//! adjusted at runtime via the debug menu without a recompile.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::aero_surface::AeroSurface;
use super::bi_vector3::BiVector3;
use super::flight_config::FlightModelConfig;
use crate::plane::PlaneState;

/// Per-instance aircraft state. Only runtime-mutable values live here;
/// fixed parameters (thrust_max, sensitivities, etc.) live in [`FlightModelConfig`].
#[derive(Component)]
pub struct AircraftRoot {
    pub throttle_percent: f32,
}

impl Default for AircraftRoot {
    fn default() -> Self {
        Self { throttle_percent: 0.5 }
    }
}

pub fn apply_aero_forces(
    mut aircraft_q: Query<(
        Forces,
        &Mass,
        &AngularInertia,
        &Children,
        &AircraftRoot,
        &mut PlaneState,
    )>,
    surface_q: Query<(&AeroSurface, &GlobalTransform)>,
    cfg: Res<FlightModelConfig>,
    time: Res<Time>,
) {
    let Ok((mut forces, mass, inertia, children, root, mut state)) = aircraft_q.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    let lin_vel: Vec3 = forces.linear_velocity();
    let ang_vel: Vec3 = forces.angular_velocity();
    let com: Vec3 = forces.position().0;
    let rot: Quat = forces.rotation().0;

    // nose = aircraft local +Z in world space (model faces +Z, parent scale 0.1)
    let nose = rot * Vec3::Z;

    let frame_ft = sum_aero_forces(lin_vel, ang_vel, com, children, &surface_q, cfg.air_density);

    // thrust_max comes from the config; root only tracks the live throttle position
    let thrust_force = nose * cfg.thrust_max * root.throttle_percent;

    // Predict velocity (trapezoidal, matching Unity AircraftPhysics.cs)
    let vel_pred = lin_vel
        + dt * cfg.prediction_fraction
            * ((frame_ft.force + thrust_force) / mass.0 + Vec3::NEG_Y * cfg.gravity);

    // Predict angular velocity
    let inertia_world_rot = rot * inertia.local_frame;
    let torque_local = inertia_world_rot.inverse() * frame_ft.torque;
    let accel_local = torque_local / inertia.principal;
    let ang_accel = inertia_world_rot * accel_local;
    let ang_vel_pred = ang_vel + dt * cfg.prediction_fraction * ang_accel;

    let pred_ft = sum_aero_forces(vel_pred, ang_vel_pred, com, children, &surface_q, cfg.air_density);

    let final_ft = (frame_ft + pred_ft) * 0.5;

    // Rotational drag: torque = -aero_damp * airspeed * ang_vel.
    // See flight_config.rs for the damping ratio derivation.
    let aero_damp = -ang_vel * cfg.aero_damp * lin_vel.length();

    forces.apply_force(final_ft.force + thrust_force);
    forces.apply_torque(final_ft.torque + aero_damp);

    // Update shared PlaneState for HUD / camera
    state.speed = lin_vel.length();
    state.thrust = cfg.thrust_max * root.throttle_percent;
    state.drag = final_ft.force.dot(-lin_vel.normalize_or_zero()).max(0.0);
    let lift_vertical = final_ft.force.dot(Vec3::Y).max(0.0);
    state.lift_pct = lift_vertical / (mass.0 * cfg.gravity).max(1.0);
}

fn sum_aero_forces(
    lin_vel: Vec3,
    ang_vel: Vec3,
    com: Vec3,
    children: &Children,
    surface_q: &Query<(&AeroSurface, &GlobalTransform)>,
    air_density: f32,
) -> BiVector3 {
    let mut total = BiVector3::default();
    for child in children {
        let Ok((surface, gtf)) = surface_q.get(*child) else { continue };
        let surface_pos = gtf.translation();
        let rel_pos = surface_pos - com;
        // air velocity at this surface = -body velocity - angular contribution
        let world_air_vel = -lin_vel - ang_vel.cross(rel_pos);
        let (_, rot, _) = gtf.to_scale_rotation_translation();
        total += surface.calculate_forces(world_air_vel, air_density, rel_pos, rot);
    }
    total
}
