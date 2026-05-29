//! Applies aerodynamic forces and thrust to the rigid body each physics step.
//!
//! All tunable constants are read from [`FlightModelConfig`] so they can be
//! adjusted at runtime via the debug menu without a recompile.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::aero_surface::AeroSurface;
use super::bi_vector3::BiVector3;
use super::flight_config::FlightModelConfig;
use super::landing_gear::GROUND_Y;
use crate::plane::PlaneState;

/// The aircraft entity bakes `scale(0.1)` into its Transform, but Avian's
/// `Position`/`Rotation` are world-space, so child local positions (and the
/// local center of mass) must be multiplied by this to reach metres.
pub const ROOT_SCALE: f32 = 0.1;

/// Per-instance aircraft state. Only runtime-mutable values live here;
/// fixed parameters (thrust_max, sensitivities, etc.) live in [`FlightModelConfig`].
/// Running state of the piston engine — a small state machine driven in
/// `airplane_controller`. `Off` is cold/dark, `Cranking` while the starter is
/// engaged, `Running` once it catches and fires on its own.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum EngineState {
    Off,
    Cranking,
    #[default]
    Running,
}

#[derive(Component)]
pub struct AircraftRoot {
    pub throttle_percent: f32,
    /// Live engine speed (revolutions per second). The throttle sets a *target*
    /// RPM; this value spools toward it with inertia (see `airplane_controller`),
    /// and both thrust and the propeller's visual spin are driven from it — so
    /// chopping the throttle winds the engine (and thrust) down over a few
    /// seconds rather than cutting instantly.
    pub engine_rps: f32,
    /// Engine running state (off / cranking / running).
    pub engine_state: EngineState,
    /// Seconds the starter has been cranking, used to time the "catch".
    pub crank_timer: f32,
    /// Mixture lever, 0 = idle cutoff (no fuel → engine dies) to 1 = full rich.
    /// Must be matched to the air density: full rich near sea level, leaned as
    /// you climb. Mis-set loses power; pulled to cutoff it stops the engine.
    pub mixture: f32,
    /// Commanded flap deflection (radians) — the notch the lever is set to.
    pub flap_target: f32,
    /// Actual flap deflection (radians), which moves toward `flap_target` at a
    /// finite rate so flaps extend/retract over a couple of seconds.
    pub flap_setting: f32,
}

impl Default for AircraftRoot {
    fn default() -> Self {
        Self {
            throttle_percent: 1.0,
            engine_rps: 0.0,
            engine_state: EngineState::default(),
            crank_timer: 0.0,
            mixture: 1.0,
            flap_target: 0.0,
            flap_setting: 0.0,
        }
    }
}

pub fn apply_aero_forces(
    mut aircraft_q: Query<(
        Forces,
        &Mass,
        &AngularInertia,
        &CenterOfMass,
        &Children,
        &AircraftRoot,
        &mut PlaneState,
    )>,
    surface_q: Query<(&AeroSurface, &Transform), Without<AircraftRoot>>,
    cfg: Res<FlightModelConfig>,
    time: Res<Time>,
) {
    let Ok((mut forces, mass, inertia, center_of_mass, children, root, mut state)) = aircraft_q.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    let lin_vel: Vec3 = forces.linear_velocity();
    let ang_vel: Vec3 = forces.angular_velocity();
    // Avian's `position()` is the transform origin, *not* the center of mass.
    // The body rotates about its CoM and Avian's velocities are CoM-relative, so
    // every aerodynamic moment arm must be measured from the world CoM:
    //   com_world = origin + rot * center_of_mass
    // matching how Avian itself derives global_center_of_mass. We read the live
    // `CenterOfMass` (already in metres, and shifted by the current loadout via
    // apply_config_to_entities) so trim/stability track the load. Using the
    // origin here (as the old code did) made the CoM cancel out entirely.
    let origin: Vec3 = forces.position().0;
    let rot: Quat = forces.rotation().0;
    let com: Vec3 = origin + rot * center_of_mass.0;

    // nose = aircraft local +Z in world space (model faces +Z, parent scale 0.1)
    let nose = rot * Vec3::Z;

    let frame_ft = sum_aero_forces(lin_vel, ang_vel, origin, com, rot, ROOT_SCALE, children, &surface_q, cfg.air_density, cfg.ground_effect_strength, cfg.ground_effect_span);

    // Thrust follows engine RPM, not the throttle directly: the throttle sets a
    // target RPM that the engine spools toward (airplane_controller), so thrust
    // builds and decays with the engine instead of snapping. A fixed-pitch prop's
    // static thrust scales roughly with RPM², so square the normalised RPM —
    // full RPM = thrust_max, and idle (~650 rpm ≈ 24% of redline) makes only a
    // gentle ~6% of max thrust, so the aircraft just creeps at idle.
    let rpm_fraction = (root.engine_rps / cfg.propeller.prop_max_rps.max(1e-3)).clamp(0.0, 1.0);
    let thrust_factor = rpm_fraction * rpm_fraction;
    let thrust_force = nose * cfg.thrust_max * thrust_factor;

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

    let pred_ft = sum_aero_forces(vel_pred, ang_vel_pred, origin, com, rot, ROOT_SCALE, children, &surface_q, cfg.air_density, cfg.ground_effect_strength, cfg.ground_effect_span);

    let final_ft = (frame_ft + pred_ft) * 0.5;

    // Rotational drag: torque = -aero_damp * airspeed * ang_vel, per body axis.
    // aero_damp is indexed (x=roll, y=yaw, z=pitch) by intent, but the BODY axes
    // are X=pitch, Y=yaw, Z=roll (nose is +Z, wings span ±X), so the roll and
    // pitch coefficients must be routed to the correct axes. Rotate the
    // world-space angular velocity into body frame, damp per axis, rotate back.
    let ang_vel_body = rot.inverse() * ang_vel;
    let damp_body = Vec3::new(cfg.aero_damp.z, cfg.aero_damp.y, cfg.aero_damp.x); // → (pitch, yaw, roll)
    let aero_damp = rot * (-ang_vel_body * damp_body * lin_vel.length());

    // Fuselage form drag ("drag box"): the bare body produces drag per body
    // axis proportional to the air it presents there. The nose is streamlined
    // (small Z·CdA) but the flanks (X) and belly/top (Y) are not, so a high-AoA
    // pull or a skid broadsides the body and sheds energy. Force acts at the
    // CoM (no moment); the fin/stabilizer supply the weathervaning. Per-axis
    // v·|v| keeps the sign and gives the usual v² magnitude.
    let v_body = rot.inverse() * lin_vel;
    let drag_body = -0.5 * cfg.air_density * Vec3::new(
        cfg.fuselage_drag.x * v_body.x * v_body.x.abs(),
        cfg.fuselage_drag.y * v_body.y * v_body.y.abs(),
        cfg.fuselage_drag.z * v_body.z * v_body.z.abs(),
    );
    let fuselage_drag = rot * drag_body;

    forces.apply_force(final_ft.force + thrust_force + fuselage_drag);
    forces.apply_torque(final_ft.torque + aero_damp);

    // Update shared PlaneState for HUD / camera
    state.speed = lin_vel.length();
    state.thrust = cfg.thrust_max * thrust_factor;
    let drag_dir = -lin_vel.normalize_or_zero();
    state.drag_surface = final_ft.force.dot(drag_dir).max(0.0) * 0.85;
    state.drag_fuselage = fuselage_drag.dot(drag_dir).max(0.0);
    state.drag = state.drag_surface + state.drag_fuselage;
    let lift_vertical = final_ft.force.dot(Vec3::Y).max(0.0);
    state.lift_pct = lift_vertical / (mass.0 * cfg.gravity).max(1.0);
}

#[allow(clippy::too_many_arguments)]
fn sum_aero_forces(
    lin_vel: Vec3,
    ang_vel: Vec3,
    origin: Vec3,
    com: Vec3,
    root_rot: Quat,
    root_scale: f32,
    children: &Children,
    surface_q: &Query<(&AeroSurface, &Transform), Without<AircraftRoot>>,
    air_density: f32,
    ground_effect_strength: f32,
    ground_effect_span: f32,
) -> BiVector3 {
    let mut total = BiVector3::default();
    for child in children {
        let Ok((surface, local_tf)) = surface_q.get(*child) else { continue };
        // World position is relative to the transform origin; the moment arm and
        // the rotational airspeed term are relative to the center of mass.
        let surface_pos = origin + root_rot * (local_tf.translation * root_scale);
        let rel_pos = surface_pos - com;
        let world_air_vel = -lin_vel - ang_vel.cross(rel_pos);
        let world_rot = root_rot * local_tf.rotation;
        let ground_effect = ground_effect_factor(
            surface_pos.y - GROUND_Y, ground_effect_span, ground_effect_strength,
        );
        total += surface.calculate_forces(world_air_vel, air_density, rel_pos, world_rot, ground_effect);
    }
    total
}

/// Effective-aspect-ratio multiplier modelling proximity to the ground.
///
/// Returns `1.0` (free air) when the surface is high up or the effect is
/// disabled, rising toward `1.0 + strength` as it approaches the ground. The
/// aero model multiplies the surface's aspect ratio by this, which raises both
/// the lift-curve slope (more lift) and the effective span efficiency (less
/// induced drag) — the float you feel in the flare.
///
/// `proximity` falls off as a Gaussian in height: it's ~1 on the deck, ~0.37 at
/// half a span up, and effectively gone (~0.02) by one full span. `span` is the
/// reference wingspan and also sets how high the cushion reaches — a bigger span
/// makes the effect linger higher. (The textbook `(16·h/b)²` influence factor is
/// tuned for a low wing right at the surface; this gentler falloff is what lets
/// a high-wing aircraft — whose wing sits a couple of metres up — actually feel
/// it, and `strength` lets you exaggerate it past the physical ~1.3× ceiling.)
pub fn ground_effect_factor(height: f32, span: f32, strength: f32) -> f32 {
    if strength <= 0.0 || span <= 0.0 {
        return 1.0;
    }
    let proximity = (-(2.0 * height.max(0.0) / span).powi(2)).exp();
    1.0 + strength * proximity
}
