use avian3d::prelude::{AngularVelocity, CenterOfMass, LinearVelocity};
use bevy::prelude::*;

use crate::physics::aero_surface::AeroSurface;
use crate::physics::aircraft_physics::AircraftRoot;
use crate::physics::flight_config::FlightModelConfig;
use crate::plane::Airplane;

#[derive(Resource, Default)]
pub struct GizmosVisible(pub bool);

pub fn toggle_gizmos(keys: Res<ButtonInput<KeyCode>>, mut visible: ResMut<GizmosVisible>) {
    if keys.just_pressed(KeyCode::KeyG) {
        visible.0 = !visible.0;
    }
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
    let com_world = tf.translation + tf.rotation * (tf.scale * com.0);

    gizmos.sphere(Isometry3d::from_translation(com_world), 0.3, Color::WHITE);

    let vel = lin_vel.0;
    if vel.length() > 0.1 {
        gizmos.arrow(com_world, com_world + vel * 0.1, Color::srgb(0.0, 1.0, 1.0));
    }

    let nose = tf.rotation * Vec3::Z;
    let thrust_n = cfg.thrust_max * root.throttle_percent * 3.0;
    gizmos.arrow(
        com_world,
        com_world + nose * (thrust_n / 2000.0),
        Color::srgb(0.2, 0.4, 1.0),
    );

    gizmos.arrow(com_world, com_world + Vec3::NEG_Y * 3.0, Color::srgb(1.0, 0.2, 0.2));

    // Fuselage drag box: a cuboid at the CoM whose extents are the per-axis
    // Cd·A (X=flank, Y=belly/top, Z=nose). The thin forward dimension vs. the
    // broad side/vertical faces visualises why pulls and skids bleed energy but
    // level cruise stays slippery. Oriented with the body, sized in metres.
    {
        let h = cfg.fuselage_drag * 0.5 * 0.2; // half-extents
        let drag_color = Color::srgb(1.0, 0.0, 0.8);
        // 8 corners in body frame, transformed to world via the body rotation.
        let corner = |sx: f32, sy: f32, sz: f32| {
            com_world + tf.rotation * Vec3::new(sx * h.x, sy * h.y, sz * h.z)
        };
        let signs = [-1.0_f32, 1.0];
        for &sy in &signs {
            for &sz in &signs {
                gizmos.line(corner(-1.0, sy, sz), corner(1.0, sy, sz), drag_color); // X edges
            }
        }
        for &sx in &signs {
            for &sz in &signs {
                gizmos.line(corner(sx, -1.0, sz), corner(sx, 1.0, sz), drag_color); // Y edges
            }
        }
        for &sx in &signs {
            for &sy in &signs {
                gizmos.line(corner(sx, sy, -1.0), corner(sx, sy, 1.0), drag_color); // Z edges
            }
        }
    }

    // Newtons → metres of arrow. Tuned so a wing at cruise reads ~1.5 m and a
    // hard pull grows it visibly, capped so high-g loads don't fill the screen.
    const FORCE_TO_M: f32 = 0.0004;

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
        let arrow_len = (force.length() * FORCE_TO_M).min(6.0);
        let lift_color = if surface.is_control_surface {
            Color::srgb(0.4, 1.0, 0.4)
        } else {
            Color::srgb(0.0, 0.8, 0.0)
        };
        gizmos.arrow(pos, pos + force.normalize_or_zero() * arrow_len, lift_color);

        gizmos.line(com_world, pos, Color::srgba(1.0, 1.0, 1.0, 0.25));
    }
}
