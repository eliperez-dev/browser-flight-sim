//! Seeded runway placement. Runway *layout* (positions, headings, elevations) is
//! derived deterministically from the world seed so it matches the terrain and
//! is identical for everyone on the same seed. The [`WorldGenerator`] owns the
//! layout (it needs it to flatten terrain around each strip); this module also
//! spawns the visible asphalt + markings and keeps them in sync with the seed.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;

use super::generator::WorldGenerator;

/// Realistic light-GA runway: ~45 m wide, 2000 m long. Same size for every
/// strip for now.
pub const RUNWAY_WIDTH: f32 = 45.0;
pub const RUNWAY_LENGTH: f32 = 2000.0;

/// Terrain is fully levelled within this radius of a runway centre and ramps
/// back to natural height by [`RUNWAY_BLEND_RADIUS`]. The flat radius must clear
/// the 1000 m half-length so the whole strip sits on level ground; the blend
/// radius stays well under the 5 km spacing so terrain survives between fields.
pub const RUNWAY_FLAT_RADIUS: f32 = 1100.0;
pub const RUNWAY_BLEND_RADIUS: f32 = 1700.0;

/// Nominal spacing between runways, and how far the grid extends from the origin.
const RUNWAY_SPACING: f32 = 5000.0;
const RUNWAY_GRID_RADIUS: i32 = 2; // ±2 cells ⇒ a 5×5 grid out to ±10 km
/// Fraction of a cell a runway may wander from its grid point (keeps them off a
/// perfect lattice without letting neighbours collide).
const RUNWAY_JITTER: f32 = 0.5;

/// One placed runway. `elevation` is the world-Y its asphalt sits at (and the
/// height terrain is flattened to around it).
#[derive(Clone, Copy)]
pub struct RunwayInstance {
    pub x: f32,
    pub z: f32,
    pub heading: f32,
    pub elevation: f32,
}

/// Marks a spawned runway root entity (its meshes are children) so the sync
/// system can despawn the whole set when the seed changes.
#[derive(Component)]
pub struct RunwaySlab;

/// Shared asphalt/paint materials, built once so every runway batches together.
#[derive(Resource)]
pub struct RunwayMaterials {
    pub asphalt: Handle<StandardMaterial>,
    pub paint: Handle<StandardMaterial>,
}

impl RunwayMaterials {
    pub fn new(materials: &mut Assets<StandardMaterial>) -> Self {
        Self {
            asphalt: materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.12, 0.13),
                perceptual_roughness: 1.0,
                // The slab top is coplanar with the flattened terrain; a positive
                // depth bias renders asphalt in front so terrain can't z-fight up
                // through it. Paint gets a higher bias still.
                depth_bias: 4.0,
                ..default()
            }),
            paint: materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.9, 0.9),
                perceptual_roughness: 1.0,
                depth_bias: 8.0,
                ..default()
            }),
        }
    }
}

// --- Deterministic hashing ---------------------------------------------------

/// Integer avalanche hash (lowbias32 by Chris Wellons).
fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Uniform `[0, 1)` from a hash state.
fn hash01(x: u32) -> f32 {
    (hash_u32(x) >> 8) as f32 / (1u32 << 24) as f32
}

/// A stable hash for grid cell `(gx, gz)` under `seed`.
fn cell_hash(seed: u32, gx: i32, gz: i32) -> u32 {
    let a = hash_u32(seed ^ 0x9e37_79b9);
    let b = hash_u32(a.wrapping_add(gx as u32).wrapping_mul(0x85eb_ca6b));
    hash_u32(b.wrapping_add(gz as u32).wrapping_mul(0xc2b2_ae35))
}

/// Builds the runway layout for `seed`. Always includes one at the origin with
/// elevation 0 (the aircraft spawns there and the physics datum is y=0); the
/// rest sit on a jittered 5 km grid, each rotated to a seeded heading and placed
/// at the natural terrain height at its centre. `generator` supplies that height
/// — it must already have its noise layers built (runways don't affect it).
pub fn generate_runways(seed: u32, generator: &WorldGenerator) -> Vec<RunwayInstance> {
    let mut out = Vec::new();
    out.push(RunwayInstance { x: 0.0, z: 0.0, heading: 0.0, elevation: 0.0 });

    for gx in -RUNWAY_GRID_RADIUS..=RUNWAY_GRID_RADIUS {
        for gz in -RUNWAY_GRID_RADIUS..=RUNWAY_GRID_RADIUS {
            if gx == 0 && gz == 0 {
                continue; // origin handled above
            }
            let h = cell_hash(seed, gx, gz);
            let jitter_x = (hash01(h) - 0.5) * RUNWAY_SPACING * RUNWAY_JITTER;
            let jitter_z = (hash01(h ^ 0x68bc_21eb) - 0.5) * RUNWAY_SPACING * RUNWAY_JITTER;
            let x = gx as f32 * RUNWAY_SPACING + jitter_x;
            let z = gz as f32 * RUNWAY_SPACING + jitter_z;
            // A runway is bidirectional, so a half-turn of heading covers every
            // distinct orientation.
            let heading = hash01(h ^ 0x2545_f491) * std::f32::consts::PI;
            let elevation = generator.natural_height(x, z);
            out.push(RunwayInstance { x, z, heading, elevation });
        }
    }
    out
}

/// Respawns all runway meshes whenever the [`WorldGenerator`] changes (startup
/// and every seed/scale edit), so the visible strips always match the layout the
/// terrain was flattened for.
pub fn sync_runways(
    mut commands: Commands,
    generator: Res<WorldGenerator>,
    materials: Res<RunwayMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    existing: Query<Entity, With<RunwaySlab>>,
    mut spawned: Local<bool>,
) {
    if *spawned && !generator.is_changed() {
        return;
    }
    *spawned = true;

    for entity in &existing {
        commands.entity(entity).despawn();
    }
    for inst in generator.runways() {
        spawn_runway(&mut commands, &mut meshes, &materials, inst);
    }
}

/// Spawns one runway: a volumetric asphalt slab plus centreline dashes and
/// threshold bars, all as children of a root placed at the runway's position,
/// elevation and heading. Geometry is authored in local space (centre at the
/// origin, length along local +Z) so the root transform handles placement.
fn spawn_runway(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &RunwayMaterials,
    inst: &RunwayInstance,
) {
    const SURFACE_Y: f32 = 0.0; // local: slab top
    const PAINT_Y: f32 = 0.1;
    const THICKNESS: f32 = 3.0; // slab depth; top at SURFACE_Y, rest buried

    let root = Transform::from_xyz(inst.x, inst.elevation, inst.z)
        .with_rotation(Quat::from_rotation_y(inst.heading));

    commands
        .spawn((root, Visibility::default(), RunwaySlab, PIXEL_LAYER))
        .with_children(|parent| {
            // Volumetric asphalt slab (top face at local SURFACE_Y).
            parent.spawn((
                Mesh3d(meshes.add(Cuboid::new(RUNWAY_WIDTH, THICKNESS, RUNWAY_LENGTH))),
                MeshMaterial3d(materials.asphalt.clone()),
                Transform::from_xyz(0.0, SURFACE_Y - THICKNESS * 0.5, 0.0),
                PIXEL_LAYER,
            ));

            // Dashed centreline. `NoFrustumCulling` on the zero-thickness paint
            // planes: their bounding box has no height, so Bevy's frustum test
            // wrongly culls them at shallow angles.
            const DASH_LEN: f32 = 30.0;
            const GAP_LEN: f32 = 20.0;
            const DASH_W: f32 = 1.0;
            let stride = DASH_LEN + GAP_LEN;
            let count = (RUNWAY_LENGTH / stride) as i32;
            let start = -(count as f32 - 1.0) * stride * 0.5;
            for i in 0..count {
                let z = start + i as f32 * stride;
                parent.spawn((
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(DASH_W, DASH_LEN))),
                    MeshMaterial3d(materials.paint.clone()),
                    Transform::from_xyz(0.0, PAINT_Y, z),
                    NoFrustumCulling,
                    PIXEL_LAYER,
                ));
            }

            // Threshold "piano key" bars at each end.
            const BAR_W: f32 = 2.5;
            const BAR_LEN: f32 = 20.0;
            const BAR_GAP: f32 = 2.0;
            let half_len = RUNWAY_LENGTH * 0.5;
            for end in [-1.0_f32, 1.0] {
                let z = end * (half_len - BAR_LEN * 0.5 - 5.0);
                let bar_stride = BAR_W + BAR_GAP;
                for k in -4..=4 {
                    if k == 0 {
                        continue;
                    }
                    parent.spawn((
                        Mesh3d(meshes.add(Plane3d::default().mesh().size(BAR_W, BAR_LEN))),
                        MeshMaterial3d(materials.paint.clone()),
                        Transform::from_xyz(k as f32 * bar_stride, PAINT_Y, z),
                        NoFrustumCulling,
                        PIXEL_LAYER,
                    ));
                }
            }
        });
}
