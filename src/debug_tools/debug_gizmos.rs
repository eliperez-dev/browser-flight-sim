use avian3d::prelude::{AngularVelocity, CenterOfMass, LinearVelocity};
use bevy::prelude::*;

use crate::physics::aero_surface::AeroSurface;
use crate::physics::aircraft_physics::AircraftRoot;
use crate::physics::flight_config::FlightModelConfig;
use crate::physics::landing_gear::gear_legs;
use crate::plane::Airplane;

#[derive(Resource, Default)]
pub struct GizmosVisible(pub bool);

pub fn toggle_gizmos(keys: Res<ButtonInput<KeyCode>>, mut visible: ResMut<GizmosVisible>) {
    if keys.just_pressed(KeyCode::KeyG) {
        visible.0 = !visible.0;
    }
}

/// Draw the aero gizmos on top of the aircraft mesh instead of letting it
/// occlude them. `depth_bias = -1.0` renders gizmo lines in front of all
/// geometry (see Bevy's `GizmoConfig::depth_bias`), so the force/CoM arrows
/// always poke through the plane. Run once at startup.
pub fn setup_gizmo_config(mut config_store: ResMut<GizmoConfigStore>) {
    let (config, _) = config_store.config_mut::<DefaultGizmoConfigGroup>();
    config.depth_bias = -1.0;
}

pub fn draw_aero_gizmos(
    visible: Res<GizmosVisible>,
    cfg: Res<FlightModelConfig>,
    aircraft_q: Query<
        (&Transform, &LinearVelocity, &AngularVelocity, &CenterOfMass, &Children, &AircraftRoot),
        With<Airplane>,
    >,
    surface_q: Query<(&AeroSurface, &Transform), Without<Airplane>>,
    mut gizmos: Gizmos,
) {
    if !visible.0 {
        return;
    }

    let Ok((tf, lin_vel, ang_vel, com, children, root)) = aircraft_q.single() else {
        return;
    };

    // CoM world position derived from the root's interpolated Transform (updated every frame
    // by TransformInterpolation), not GlobalTransform (only updated on physics steps).
    // CenterOfMass is in metres (unscaled), matching Avian's
    // global_center_of_mass = position + rotation * center_of_mass — so do NOT
    // apply tf.scale here, or the offset (and its shift under load) shrinks 10×.
    let com_world = tf.translation + tf.rotation * com.0;

    gizmos.sphere(Isometry3d::from_translation(com_world), 0.3, Color::WHITE);

    let vel = lin_vel.0;
    if vel.length() > 0.1 {
        gizmos.arrow(com_world, com_world + vel * 0.2, Color::srgb(0.0, 1.0, 1.0));
    }

    let nose = tf.rotation * Vec3::Z;
    let thrust_n = cfg.thrust_max * root.throttle_percent * 5.0;
    gizmos.arrow(
        com_world,
        com_world + nose * (thrust_n / 2000.0),
        Color::srgb(0.2, 0.4, 1.0),
    );

    gizmos.arrow(com_world, com_world + Vec3::NEG_Y * 3.0, Color::srgb(1.0, 0.2, 0.2));

    // Landing-gear struts: a line from each mount down to the wheel at full
    // extension, with a sphere at the contact point. Mounts are in metres in the
    // body frame (like the CoM), so reconstruct world position the same way —
    // tf.translation + tf.rotation * mount — *without* tf.scale. The wheel hangs
    // `gear_rest_length` along body-down (−Y); this is exactly where the strut in
    // landing_gear.rs starts looking for the ground, so the spheres mark where
    // the aircraft will touch down.
    {
        let strut_color = Color::srgb(0.8, 0.4, 1.0); // violet
        let down = tf.rotation * Vec3::NEG_Y;
        for leg in gear_legs(&cfg) {
            let mount_world = tf.translation + tf.rotation * leg.mount;
            let wheel_world = mount_world + down * leg.rest_length;
            gizmos.line(mount_world, wheel_world, strut_color);
            gizmos.sphere(Isometry3d::from_translation(wheel_world), 0.2, strut_color);
        }
    }

    // Fuselage drag box: a cuboid at the CoM whose extents are the per-axis
    // Cd·A (X=flank, Y=belly/top, Z=nose). The thin forward dimension vs. the
    // broad side/vertical faces visualises why pulls and skids bleed energy but
    // level cruise stays slippery. Oriented with the body, sized in metres.
    // {
    //     let h = cfg.fuselage_drag * 0.5 * 0.2; // half-extents
    //     let drag_color = Color::srgb(1.0, 0.0, 0.8);
    //     // 8 corners in body frame, transformed to world via the body rotation.
    //     let corner = |sx: f32, sy: f32, sz: f32| {
    //         com_world + tf.rotation * Vec3::new(sx * h.x, sy * h.y, sz * h.z)
    //     };
    //     let signs = [-1.0_f32, 1.0];
    //     for &sy in &signs {
    //         for &sz in &signs {
    //             gizmos.line(corner(-1.0, sy, sz), corner(1.0, sy, sz), drag_color); // X edges
    //         }
    //     }
    //     for &sx in &signs {
    //         for &sz in &signs {
    //             gizmos.line(corner(sx, -1.0, sz), corner(sx, 1.0, sz), drag_color); // Y edges
    //         }
    //     }
    //     for &sx in &signs {
    //         for &sy in &signs {
    //             gizmos.line(corner(sx, sy, -1.0), corner(sx, sy, 1.0), drag_color); // Z edges
    //         }
    //     }
    // }

    // Newtons → metres of arrow. Tuned so a wing at cruise reads ~1.5 m and a
    // hard pull grows it visibly, capped so high-g loads don't fill the screen.
    const FORCE_TO_M: f32 = 0.0004;

    // Aerodynamic center (the whole-aircraft neutral point): the point about
    // which the pitching moment does NOT change with angle of attack. It's where
    // the *increment* of aero force with AoA acts, so we find it by finite
    // difference — sampling every surface at the current freestream and at a
    // freestream nudged by `D_ALPHA`, then locating where ΔF acts:
    //   AC = CoM + (ΔF × ΔM) / |ΔF|².
    // Unlike the center of pressure, ΔF (the lift-curve-slope force) stays large
    // and steady in trim, so the AC sits still; it only shifts near stall where
    // the lift slope collapses. The rotational airflow term is excluded on
    // purpose — the AC is a pure AoA response, not a rate/damping effect.
    let pitch_axis = tf.rotation * Vec3::X; // body X = pitch axis
    let d_alpha = 2.0_f32.to_radians();
    let base_wind = -vel; // freestream relative to the aircraft
    let pert_wind = Quat::from_axis_angle(pitch_axis, d_alpha) * base_wind;
    let mut f_base = Vec3::ZERO;
    let mut m_base = Vec3::ZERO;
    let mut f_pert = Vec3::ZERO;
    let mut m_pert = Vec3::ZERO;

    for child in children {
        let Ok((surface, local_tf)) = surface_q.get(*child) else {
            continue;
        };

        // Reconstruct world position from root's interpolated Transform + child local Transform.
        // child local_tf.translation is in the parent's local space (scale 0.1 already applies).
        let pos = tf.transform_point(local_tf.translation);
        let rot = tf.rotation * local_tf.rotation;

        gizmos.sphere(Isometry3d::from_translation(pos), 0.15, Color::srgb(1.0, 0.55, 0.0));

        // Actual aerodynamic force this surface produces right now, using the
        // same calculation as the physics step (so it tracks AoA, control
        // deflection and stall). world_air_vel includes the rotation-induced
        // flow, matching sum_aero_forces in aircraft_physics.rs.
        let rel_pos = pos - com_world;
        let world_air_vel = -vel - ang_vel.0.cross(rel_pos);
        let force = surface
            .calculate_forces(world_air_vel, cfg.air_density, rel_pos, rot)
            .force;

        // AC sampling: uniform freestream (no rotation term), base vs perturbed AoA.
        let fb = surface.calculate_forces(base_wind, cfg.air_density, rel_pos, rot).force;
        let fp = surface.calculate_forces(pert_wind, cfg.air_density, rel_pos, rot).force;
        f_base += fb;
        m_base += rel_pos.cross(fb);
        f_pert += fp;
        m_pert += rel_pos.cross(fp);

        let arrow_len = (force.length() * FORCE_TO_M).min(6.0);
        let lift_color = if surface.is_control_surface {
            Color::srgb(0.4, 1.0, 0.4)
        } else {
            Color::srgb(0.0, 0.8, 0.0)
        };
        gizmos.arrow(pos, pos + force.normalize_or_zero() * arrow_len, lift_color);

        gizmos.line(com_world, pos, Color::srgba(1.0, 1.0, 1.0, 0.25));
    }

    // Draw the aerodynamic center once there's a meaningful lift-curve response
    // (skip on the ground / near zero airspeed / deep stall). The yellow↔white
    // gap is the static margin: AC aft of the CoM (toward the tail) is
    // pitch-stable, AC ahead of it is unstable.
    let d_force = f_pert - f_base;
    let d_moment = m_pert - m_base;
    if vel.length() > 5.0 && d_force.length() > 1.0 {
        let ac_rel = (d_force.cross(d_moment) / d_force.length_squared()).clamp_length_max(10.0);
        let ac_world = com_world + ac_rel;
        let ac_color = Color::srgb(1.0, 0.9, 0.0); // yellow
        gizmos.sphere(Isometry3d::from_translation(ac_world), 0.25, ac_color);
        gizmos.line(com_world, ac_world, ac_color);
    }
}
