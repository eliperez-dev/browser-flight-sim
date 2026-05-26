use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;

pub fn spawn_debug_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    // Low-level fill light so surfaces in shadow aren't pitch black.
    ambient.brightness = 400.0;

    spawn_runway(&mut commands, &mut meshes, &mut materials);

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

/// A landable asphalt runway aligned with the +Z flight axis (the direction the
/// aircraft is launched in `spawn_aircraft`). Centered on the origin so the
/// plane can descend straight onto it. All dimensions are in world metres.
fn spawn_runway(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    // Realistic light-GA runway: ~45 m wide, 2000 m long.
    const WIDTH: f32 = 45.0;
    const LENGTH: f32 = 2000.0;
    // Asphalt surface sits at the physics ground level (y=0) where the wheels
    // rest — the terrain is flattened to y=0 across the airfield, so the slab and
    // the ground are coplanar. Marking paint sits a little above the asphalt.
    const SURFACE_Y: f32 = 0.0;
    const PAINT_Y: f32 = 0.1;
    // The slab is a real box `THICKNESS` deep (top face at SURFACE_Y, the rest
    // buried) so it reads as a volumetric pad rather than a paper-thin plane —
    // and so its sides hide the terrain edge wherever the ground isn't dead flat.
    const THICKNESS: f32 = 3.0;

    let asphalt = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.12, 0.13),
        perceptual_roughness: 1.0,
        // The slab top is coplanar with the flat y=0 terrain; a positive depth
        // bias renders the asphalt in front so the terrain can't z-fight through
        // it ("clipping up through the runway"). Paint gets a higher bias still.
        depth_bias: 4.0,
        ..default()
    });
    let paint = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.9, 0.9),
        perceptual_roughness: 1.0,
        depth_bias: 8.0,
        ..default()
    });

    // Volumetric asphalt slab: a box whose top face is at SURFACE_Y. Because it
    // has real height its bounding box is correct, so (unlike the old zero-Y
    // plane) it doesn't need NoFrustumCulling to stop flickering out.
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(WIDTH, THICKNESS, LENGTH))),
        MeshMaterial3d(asphalt.clone()),
        Transform::from_xyz(0.0, SURFACE_Y - THICKNESS * 0.5, 0.0),
        PIXEL_LAYER,
    ));

    // Dashed centerline running the length of the runway.
    const DASH_LEN: f32 = 30.0;
    const GAP_LEN: f32 = 20.0;
    const DASH_W: f32 = 1.0;
    let stride = DASH_LEN + GAP_LEN;
    let count = (LENGTH / stride) as i32;
    let start = -(count as f32 - 1.0) * stride * 0.5;
    for i in 0..count {
        let z = start + i as f32 * stride;
        commands.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(DASH_W, DASH_LEN))),
            MeshMaterial3d(paint.clone()),
            Transform::from_xyz(0.0, PAINT_Y, z),
            NoFrustumCulling,
            PIXEL_LAYER,
        ));
    }

    // Threshold "piano key" bars at each end of the runway.
    const BAR_W: f32 = 2.5;
    const BAR_LEN: f32 = 20.0;
    const BAR_GAP: f32 = 2.0;
    let half_len = LENGTH * 0.5;
    for end in [-1.0_f32, 1.0] {
        // Offset the bars just inboard of the runway end.
        let z = end * (half_len - BAR_LEN * 0.5 - 5.0);
        let bar_stride = BAR_W + BAR_GAP;
        // A row of bars centered across the width (leave the centerline clear).
        for k in -4..=4 {
            if k == 0 {
                continue;
            }
            let x = k as f32 * bar_stride;
            commands.spawn((
                Mesh3d(meshes.add(Plane3d::default().mesh().size(BAR_W, BAR_LEN))),
                MeshMaterial3d(paint.clone()),
                Transform::from_xyz(x, PAINT_Y, z),
                NoFrustumCulling,
                PIXEL_LAYER,
            ));
        }
    }
}
