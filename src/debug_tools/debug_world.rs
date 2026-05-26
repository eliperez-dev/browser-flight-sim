use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;

pub fn spawn_debug_world(
    mut commands: Commands,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    // Low-level fill light so surfaces in shadow aren't pitch black.
    ambient.brightness = 400.0;

    // Runways are spawned by the terrain plugin (seeded placement + terrain
    // flattening); this system just sets up the scene lighting.

    // Directional light so the terrain and runways are actually lit.
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
