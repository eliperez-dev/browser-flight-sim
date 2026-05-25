use bevy::camera::visibility::NoFrustumCulling;
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
    // Large green ground plane, dropped 1 m below the physics ground (y=0) so the
    // runway slab can sit at y=0 — where the wheels actually rest — without
    // z-fighting against this plane. A 1 m step is invisible from the air over a
    // 50 km plane, but gives the depth buffer plenty of separation up close.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(500.0 * scale, 500.0 * scale))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.22, 0.55, 0.12),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -1.0, 0.0),
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
    // Asphalt sits at the physics ground level (y=0) where the wheels rest, a
    // full 1 m above the lowered green ground plane. Marking paint sits a little
    // above the asphalt; that 0.1 m gap is plenty now that nothing competes with
    // it at the same depth.
    const SURFACE_Y: f32 = 0.0;
    const PAINT_Y: f32 = 0.1;

    let asphalt = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.12, 0.13),
        perceptual_roughness: 1.0,
        ..default()
    });
    let paint = materials.add(StandardMaterial {
        base_color: Color::srgb(0.9, 0.9, 0.9),
        perceptual_roughness: 1.0,
        ..default()
    });

    // Asphalt slab (a flat plane, so we don't need a tall box).
    // `NoFrustumCulling` on every runway piece: these meshes are zero-thickness
    // in Y, so their bounding box has no height and Bevy's frustum test wrongly
    // culls them at shallow camera angles / distances, making the runway flicker
    // out. Forcing them to always render is cheap here and fixes that.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(WIDTH, LENGTH))),
        MeshMaterial3d(asphalt.clone()),
        Transform::from_xyz(0.0, SURFACE_Y, 0.0),
        NoFrustumCulling,
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
