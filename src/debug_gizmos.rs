use avian3d::prelude::{CenterOfMass, LinearVelocity};
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;
use crate::physics::aero_surface::AeroSurface;
use crate::physics::aircraft_physics::AircraftRoot;
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
    aircraft_q: Query<
        (&Transform, &LinearVelocity, &CenterOfMass, &Children, &AircraftRoot),
        With<Airplane>,
    >,
    surface_q: Query<(&AeroSurface, &Transform)>,
    mut gizmos: Gizmos,
) {
    if !visible.0 {
        return;
    }

    let Ok((tf, lin_vel, com, children, root)) = aircraft_q.single() else {
        return;
    };

    // CoM world position: entity origin + rotation * (scale * local_com)
    let com_world = tf.translation + tf.rotation * (tf.scale * com.0);

    // CoM marker — white sphere
    gizmos.sphere(Isometry3d::from_translation(com_world), 0.3, Color::WHITE);

    // Velocity arrow — cyan
    let vel = lin_vel.0;
    if vel.length() > 0.1 {
        gizmos.arrow(com_world, com_world + vel * 0.1, Color::srgb(0.0, 1.0, 1.0));
    }

    // Thrust arrow — blue, along nose (+Z rotated)
    let nose = tf.rotation * Vec3::Z;
    let thrust_n = root.thrust_max * root.throttle_percent;
    gizmos.arrow(
        com_world,
        com_world + nose * (thrust_n / 2000.0),
        Color::srgb(0.2, 0.4, 1.0),
    );

    // Gravity arrow — red downward reference
    gizmos.arrow(com_world, com_world + Vec3::NEG_Y * 3.0, Color::srgb(1.0, 0.2, 0.2));

    let speed = vel.length();
    let q = 0.5 * 1.2 * speed * speed; // dynamic pressure

    for child in children {
        let Ok((surface, local_tf)) = surface_q.get(*child) else {
            continue;
        };
        // Derive world pos/rot from the interpolated parent transform, not GlobalTransform.
        // GlobalTransform on children only syncs at the physics rate; tf is already interpolated.
        let pos = tf.transform_point(local_tf.translation);
        let rot = tf.rotation * local_tf.rotation;

        // Surface position — orange dot
        gizmos.sphere(Isometry3d::from_translation(pos), 0.15, Color::srgb(1.0, 0.55, 0.0));

        // Approximate lift direction: surface local +Y in world, scaled by dynamic pressure * area
        let lift_dir = rot * Vec3::Y;
        let area = surface.config.chord * surface.config.span;
        let lift_scale = (q * area * 0.002).clamp(0.1, 6.0);
        let lift_color = if surface.is_control_surface {
            Color::srgb(0.4, 1.0, 0.4) // lighter green for control surfaces
        } else {
            Color::srgb(0.0, 0.8, 0.0) // solid green for fixed surfaces
        };
        gizmos.arrow(pos, pos + lift_dir * lift_scale, lift_color);

        // Line from CoM to surface — dim white
        gizmos.line(com_world, pos, Color::srgba(1.0, 1.0, 1.0, 0.25));
    }
}
