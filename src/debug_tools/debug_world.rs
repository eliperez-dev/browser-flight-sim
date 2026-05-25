use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;

pub fn spawn_debug_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    let scale = 100.0;

    // Low-level fill light so surfaces in shadow aren't pitch black.
    ambient.brightness = 400.0;
    // Large green ground plane
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(500.0 * scale, 500.0 * scale))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.55, 0.12),
            perceptual_roughness: 1.0,
            ..default()
        })),
        PIXEL_LAYER,
    ));

    // Cubes scattered on the ground — vary size and position for a rough landmark grid
    let cubes: &[(f32, f32, f32, f32, f32, f32)] = &[
        // x,    y_half, z,    w,   h,   d
        ( 8.0,  0.5,  5.0,  1.0, 1.0, 1.0),
        (-6.0,  1.0, -4.0,  1.0, 2.0, 1.0),
        (15.0,  0.75, -10.0, 1.5, 1.5, 1.5),
        (-12.0, 0.5,  8.0,  1.0, 1.0, 1.0),
        ( 3.0,  1.5,  18.0, 1.0, 3.0, 1.0),
        (-20.0, 0.5, -15.0, 2.0, 1.0, 2.0),
    ];

    let cube_color = materials.add(Color::srgb(0.75, 0.45, 0.2));

    for &(x, y_half, z, w, h, d) in cubes {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(w * scale, h * scale, d * scale))),
            MeshMaterial3d(cube_color.clone()),
            Transform::from_xyz(x * scale, y_half * scale, z * scale),
            PIXEL_LAYER,
        ));
    }

    // Directional light so the ground and cubes are actually lit
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.4, 0.0)),
        PIXEL_LAYER,
    ));
}
