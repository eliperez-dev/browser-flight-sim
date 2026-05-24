use avian3d::prelude::{CenterOfMass, LinearVelocity};
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
        (&Transform, &LinearVelocity, &CenterOfMass, &Children, &AircraftRoot),
        With<Airplane>,
    >,
    surface_q: Query<(&AeroSurface, &Transform), Without<Airplane>>,
    mut gizmos: Gizmos,
) {
    if !visible.0 {
        return;
    }

    let Ok((tf, lin_vel, com, children, root)) = aircraft_q.single() else {
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
    let thrust_n = cfg.thrust_max * root.throttle_percent;
    gizmos.arrow(
        com_world,
        com_world + nose * (thrust_n / 2000.0),
        Color::srgb(0.2, 0.4, 1.0),
    );

    gizmos.arrow(com_world, com_world + Vec3::NEG_Y * 3.0, Color::srgb(1.0, 0.2, 0.2));

    let speed = vel.length();
    let q = 0.5 * 1.2 * speed * speed;

    for child in children {
        let Ok((surface, local_tf)) = surface_q.get(*child) else {
            continue;
        };

        // Reconstruct world position from root's interpolated Transform + child local Transform.
        // child local_tf.translation is in the parent's local space (scale 0.1 already applies).
        let pos = tf.transform_point(local_tf.translation);
        let rot = tf.rotation * local_tf.rotation;

        gizmos.sphere(Isometry3d::from_translation(pos), 0.15, Color::srgb(1.0, 0.55, 0.0));

        let lift_dir = rot * Vec3::Y;
        let area = surface.config.chord * surface.config.span;
        let lift_scale = (q * area * 0.0002).clamp(0.05, 5.0);
        let lift_color = if surface.is_control_surface {
            Color::srgb(0.4, 1.0, 0.4)
        } else {
            Color::srgb(0.0, 0.8, 0.0)
        };
        gizmos.arrow(pos, pos + lift_dir * lift_scale, lift_color);

        gizmos.line(com_world, pos, Color::srgba(1.0, 1.0, 1.0, 0.25));
    }
}
