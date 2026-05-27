//! Spring-damper landing gear.
//!
//! Rather than giving the fuselage a rigid collider and letting the solver
//! resolve ground contacts (which makes a stiff body bounce and tumble on
//! touchdown), each wheel is modelled as a suspension strut: a downward ray
//! from a fixed mount point on the airframe. When the strut would intersect the
//! ground it produces a spring force (proportional to how far it is compressed)
//! plus a damper force (proportional to compression speed), so the aircraft
//! settles onto its wheels smoothly and absorbs bumps instead of rebounding.
//!
//! A simple tyre-friction model on top of that keeps the aircraft tracking
//! straight while taxiing: strong resistance to sliding sideways, light rolling
//! resistance fore-and-aft.
//!
//! All forces are applied through Avian's [`Forces`] API at the contact point,
//! so each strut also generates the correct pitch/roll moment about the centre
//! of mass — a hard nose-wheel-first touchdown pitches the tail down, a
//! one-wheel landing rolls the aircraft, and so on. The tunable feel constants
//! live in [`FlightModelConfig`] so they can be adjusted live from the F3 menu.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::flight_config::FlightModelConfig;
use crate::plane::{Airplane, PlaneState};
use crate::terrain::WorldGenerator;

/// Reference ground height, in metres. The streaming terrain is flattened to
/// y=0 around the runway, so this stays the datum for ground-effect lift
/// (aircraft_physics) and the gizmos. Actual wheel contact samples the real
/// terrain height per strut instead — see `apply_landing_gear`.
pub const GROUND_Y: f32 = 0.0;

/// One landing-gear strut: where it mounts to the airframe and how far it hangs
/// when uncompressed. The nose strut carries an independent rest length from the
/// two mains so the resting pitch attitude can be tuned.
pub struct GearLeg {
    /// Mount point in the body frame (metres; +Z nose, +Y up, +X right).
    pub mount: Vec3,
    /// Natural (uncompressed) strut length in metres — the strut extends this far
    /// straight down (body −Y) from the mount.
    pub rest_length: f32,
}

/// Builds the three gear struts from the tunable gear geometry in
/// [`FlightModelConfig`] so the layout can be adjusted live from the F3 menu.
///
/// Order is `[nose, main-left, main-right]`. The nose uses `gear_nose_rest_length`
/// while both mains share `gear_rest_length`. Shared with the debug gizmos so the
/// drawn struts always match where the physics looks for the ground.
pub fn gear_legs(flight_model: &FlightModelConfig) -> [GearLeg; 3] {
    let cfg = &flight_model.landing_gear;
    let nose_y = cfg.gear_nose_mount_height;
    let main_y = cfg.gear_main_mount_height;
    let half_track = cfg.gear_track * 0.5;
    [
        GearLeg {
            mount: Vec3::new(0.0, nose_y, cfg.gear_nose_z), // nose wheel, forward of the CoM
            rest_length: cfg.gear_nose_rest_length,
        },
        GearLeg {
            mount: Vec3::new(-half_track, main_y, cfg.gear_main_z), // main gear, left
            rest_length: cfg.gear_rest_length,
        },
        GearLeg {
            mount: Vec3::new(half_track, main_y, cfg.gear_main_z), // main gear, right
            rest_length: cfg.gear_rest_length,
        },
    ]
}

/// Applies suspension and tyre-friction forces for every gear leg in contact
/// with the ground, and records the on-ground / braking status in
/// [`PlaneState`] for the HUD and other systems to read.
///
/// Holding **B** applies the wheel brakes, which add `gear_brake_strength` to
/// the rolling-resistance coefficient so the tyres bite and decelerate the
/// rollout (and, like rolling resistance, the force scales with wheel load).
///
/// Runs chained after `apply_aero_forces` so it shares the same physics step;
/// both accumulate onto Avian's per-step force buffer, which is cleared after
/// the step.
pub fn apply_landing_gear(
    mut aircraft_q: Query<(Forces, &CenterOfMass, &mut PlaneState), With<Airplane>>,
    flight_model: Res<FlightModelConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    world_gen: Res<WorldGenerator>,
) {
    let Ok((mut forces, center_of_mass, mut state)) = aircraft_q.single_mut() else {
        return;
    };

    let cfg = &flight_model.landing_gear;

    // Brakes add to the rolling-resistance coefficient while B is held.
    let braking = keys.pressed(KeyCode::KeyB);
    let rolling_crr = cfg.gear_rolling_resistance + if braking { cfg.gear_brake_strength } else { 0.0 };

    let origin: Vec3 = forces.position().0;
    let rot: Quat = forces.rotation().0;
    let lin_vel: Vec3 = forces.linear_velocity();
    let ang_vel: Vec3 = forces.angular_velocity();
    // The body rotates about its CoM and Avian's velocities are CoM-relative, so
    // every strut moment arm is measured from the world CoM (see aircraft_physics).
    let com: Vec3 = origin + rot * center_of_mass.0;

    // Strut axis: "up" is body +Y in world space; the strut extends along -up.
    let up = rot * Vec3::Y;
    let down = -up;

    let mut any_contact = false;

    for leg in gear_legs(&flight_model) {
        // If the strut is not pointing meaningfully downward (aircraft on its
        // side or inverted) the flat-ground intersection is undefined — skip it.
        if down.y > -1.0e-3 {
            continue;
        }

        let mount_world = origin + rot * leg.mount;

        // Ground height under this strut, sampled from the terrain field at the
        // mount's horizontal position (the strut is near-vertical at touchdown,
        // so sampling at the mount x/z rather than the exact contact point is a
        // negligible approximation).
        let ground_y = world_gen.get_terrain_height(mount_world.x, mount_world.z);

        // Distance along the strut from the mount to the local ground plane:
        //   (mount_world + down * t).y == ground_y
        let t = (ground_y - mount_world.y) / down.y;

        // No contact while the wheel hangs above the ground.
        if t >= leg.rest_length {
            continue;
        }
        any_contact = true;

        // Compression is how far the strut is pushed up from its rest length.
        // `t <= 0` means the mount itself is at/below ground (deep penetration);
        // clamp so the strut bottoms out rather than producing absurd forces.
        let compression = (leg.rest_length - t.max(0.0)).clamp(0.0, leg.rest_length);

        // Contact point on the ground, used as the force application point.
        let contact = mount_world + down * t.clamp(0.0, leg.rest_length);

        // Velocity of the airframe at the contact point.
        let v_point = lin_vel + ang_vel.cross(contact - com);

        // Spring pushes up by compression; damper opposes the compression rate.
        // `-v_point.dot(up)` is positive while the strut is compressing.
        let compression_speed = -v_point.dot(up);
        let normal_force = (cfg.gear_spring * compression + cfg.gear_damping * compression_speed)
            .max(0.0); // a strut can push but never pull the wheel down

        // --- Tyre friction (scales with the load on the strut) -------------
        // Horizontal velocity in the ground-tangent plane (strip the strut-axis
        // component), split into rolling (along the nose heading) and lateral.
        let v_tangent = v_point - up * v_point.dot(up);
        let heading = (rot * Vec3::Z - up * (rot * Vec3::Z).dot(up)).normalize_or_zero();
        let rolling_speed = v_tangent.dot(heading);
        let lateral_v = v_tangent - heading * rolling_speed;

        // Lateral grip: resists sliding sideways, capped at the normal load — a
        // tyre can't grip harder than the weight pressing it down (~1 g side).
        let lateral_force = (-cfg.gear_grip * lateral_v).clamp_length_max(normal_force);

        // Rolling resistance: a small fraction (Crr) of the *normal load*
        // opposing the rolling direction, like a real tyre. Because it scales
        // with the load rather than with speed, it stays tiny next to thrust and
        // — crucially — fades to zero as the wings take weight off the wheels
        // during the takeoff roll, so the aircraft can accelerate and rotate
        // instead of being pinned. `tanh` smooths the sign through zero so a
        // parked aircraft doesn't jitter.
        let rolling_force =
            heading * (-rolling_crr * normal_force * (rolling_speed * 2.0).tanh());

        forces.apply_force_at_point(up * normal_force + lateral_force + rolling_force, contact);
    }

    state.on_ground = any_contact;
    state.braking = braking;
}
