//! Seeded, streamed runway placement. Runways live on a 5 km grid: each cell
//! deterministically yields one runway (jittered position, seeded heading) from
//! the world seed, so the layout is infinite, matches the terrain, and is the
//! same for everyone on a seed. Nothing is stored globally — a cell's runway is
//! recomputed on demand wherever it's needed (terrain flattening, the gear,
//! visual spawning).
//!
//! Terrain is levelled in an oriented *rectangle* around each strip (not a
//! circle), so the flat ground hugs the runway shape. The visible asphalt +
//! markings stream in around the camera and despawn behind it, like the chunks.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;

use super::chunk::ChunkManager;
use super::generator::{WorldGenerator, CHUNK_SIZE};
use super::TerrainCamera;

/// Realistic light-GA runway: ~45 m wide, 2000 m long. Same size for every
/// strip for now.
pub const RUNWAY_WIDTH: f32 = 45.0;
pub const RUNWAY_LENGTH: f32 = 2000.0;

/// Terrain is fully levelled within the runway rectangle expanded by this apron
/// (metres beyond each edge), then ramps back to natural height over
/// [`RUNWAY_BLEND_MARGIN`]. Keep the blend well under the spacing so terrain
/// survives between fields.
const RUNWAY_FLAT_APRON: f32 = 320.0;
const RUNWAY_BLEND_MARGIN: f32 = 600.0;

/// How far the asphalt surface sits above the graded ground (metres). Gives the
/// slab visible thickness and clean separation from the terrain (no coplanar
/// z-fighting). The landing gear rests on this raised surface over the pavement,
/// so wheels sit on the asphalt rather than sinking to the graded ground.
const RUNWAY_SURFACE_LIFT: f32 = 0.3;

/// Grid spacing between runways (one per cell).
const RUNWAY_SPACING: f32 = 10000.0;
/// Fraction of a cell a runway may wander from its grid point — keeps them off a
/// perfect lattice. Bounded < 0.5 so a runway stays within its own cell.
const RUNWAY_JITTER: f32 = 0.85;

/// Sea level (world-Y). A cell whose natural ground is below this (plus a small
/// margin) is water, so no runway is placed there — the strip would sit in the
/// sea. The origin runway is exempt (it's the fixed spawn point at y=0).
const WATER_LEVEL: f32 = 0.0;
const RUNWAY_MIN_ABOVE_WATER: f32 = 3.0;

/// One placed runway. `elevation` is the world-Y its asphalt sits at (and the
/// height terrain is flattened to around it).
#[derive(Clone, Copy)]
pub struct RunwayInstance {
    pub x: f32,
    pub z: f32,
    pub heading: f32,
    pub elevation: f32,
    /// The grid cell this strip belongs to — its stable identity, used to derive
    /// a deterministic ident for the map overlay (see [`runway_ident`]).
    pub cell: (i32, i32),
}

impl RunwayInstance {
    /// The two runway numbers (one per direction), each `round(heading° / 10)`
    /// mapped to 1..=36, with the reciprocal 180° opposite. e.g. heading 170° →
    /// `(17, 35)`. Returned low-number-first by convention.
    pub fn runway_numbers(&self) -> (u32, u32) {
        let deg = self.heading.to_degrees().rem_euclid(360.0);
        let mut a = (deg / 10.0).round() as i32 % 36;
        if a == 0 {
            a = 36;
        }
        let mut b = a + 18;
        if b > 36 {
            b -= 36;
        }
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        (lo as u32, hi as u32)
    }

    /// Flatten weight at world (x, z): 1.0 inside the runway rectangle (plus
    /// apron), ramping to 0.0 by [`RUNWAY_BLEND_MARGIN`] beyond it. Distance is
    /// measured in the runway's own frame, so the levelled zone is a rounded
    /// rectangle aligned to the strip rather than a circle.
    fn flatten_weight(&self, x: f32, z: f32) -> f32 {
        // World offset rotated into the runway's local frame (length along +Z).
        let (s, c) = self.heading.sin_cos();
        let px = x - self.x;
        let pz = z - self.z;
        let lx = px * c - pz * s;
        let lz = px * s + pz * c;

        let half_w = RUNWAY_WIDTH * 0.5 + RUNWAY_FLAT_APRON;
        let half_l = RUNWAY_LENGTH * 0.5 + RUNWAY_FLAT_APRON;
        let dx = (lx.abs() - half_w).max(0.0);
        let dz = (lz.abs() - half_l).max(0.0);
        let d = (dx * dx + dz * dz).sqrt();

        let t = (d / RUNWAY_BLEND_MARGIN).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t) // smoothstep falloff
    }

    /// Whether world (x, z) lies on the paved rectangle (the asphalt itself, no
    /// apron) — i.e. where the gear should rest on the raised asphalt surface.
    fn on_pavement(&self, x: f32, z: f32) -> bool {
        let (s, c) = self.heading.sin_cos();
        let px = x - self.x;
        let pz = z - self.z;
        let lx = px * c - pz * s;
        let lz = px * s + pz * c;
        lx.abs() <= RUNWAY_WIDTH * 0.5 && lz.abs() <= RUNWAY_LENGTH * 0.5
    }
}

/// Marks a spawned runway root entity (its meshes are children), tagged with its
/// grid cell and world position so the streaming system can despawn it by actual
/// distance when it leaves range.
#[derive(Component)]
pub struct RunwaySlab {
    cell: (i32, i32),
    pos: Vec2,
}

/// Marks a REIL strobe — flashes at ~1 Hz.
#[derive(Component)]
pub struct ReilLight;

/// Marks one bar of an approach lighting sequence. `index` is the bar's distance
/// step from the threshold (1 = nearest, N = farthest); `end` is +1 or -1 for
/// which runway end this belongs to.
#[derive(Component)]
pub struct AlsLight {
    pub index: i32,
    pub end: i32,
}

/// Global clock driving runway light animation.
#[derive(Resource, Default)]
pub struct RunwayLightClock {
    pub elapsed: f32,
}

/// Grid cells whose runway is currently spawned (visually).
#[derive(Resource, Default)]
pub struct SpawnedRunways {
    cells: HashSet<(i32, i32)>,
}

/// Shared asphalt/paint materials, built once so every runway batches together.
#[derive(Resource)]
pub struct RunwayMaterials {
    pub asphalt: Handle<StandardMaterial>,
    pub paint: Handle<StandardMaterial>,
}

impl RunwayMaterials {
    pub fn new(materials: &mut Assets<StandardMaterial>) -> Self {
        Self {
            // The slab now sits physically above the graded terrain
            // (RUNWAY_SURFACE_LIFT), so no depth bias is needed to avoid z-fighting.
            asphalt: materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.12, 0.13),
                perceptual_roughness: 1.0,
                ..default()
            }),
            paint: materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.9, 0.9),
                perceptual_roughness: 1.0,
                ..default()
            }),
        }
    }
}

// --- Deterministic per-cell layout -------------------------------------------

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

/// The runway for grid cell `(gx, gz)`, or `None` if the cell is water (its
/// natural ground is below sea level) so no strip is placed there. Cell (0,0) is
/// always present (the aircraft spawn point) and keeps heading 0; every runway,
/// origin included, sits level with the natural terrain at its centre. Others
/// are also jittered and seed-rotated. `generator` supplies that height (it
/// ignores runways, so no cycle).
fn runway_for_cell(generator: &WorldGenerator, gx: i32, gz: i32) -> Option<RunwayInstance> {
    if gx == 0 && gz == 0 {
        // The origin strip sits level with the terrain like any other runway, but
        // keeps heading 0 (the aircraft spawns aligned to it) and is always
        // present (it's the spawn point, exempt from the water check).
        return Some(RunwayInstance {
            x: 0.0,
            z: 0.0,
            heading: 0.0,
            elevation: generator.natural_height(0.0, 0.0),
            cell: (0, 0),
        });
    }
    let h = cell_hash(generator.seed(), gx, gz);
    let jitter_x = (hash01(h) - 0.5) * RUNWAY_SPACING * RUNWAY_JITTER;
    let jitter_z = (hash01(h ^ 0x68bc_21eb) - 0.5) * RUNWAY_SPACING * RUNWAY_JITTER;
    let x = gx as f32 * RUNWAY_SPACING + jitter_x;
    let z = gz as f32 * RUNWAY_SPACING + jitter_z;
    let elevation = generator.natural_height(x, z);
    // Skip water: don't put a runway where the sea is supposed to be.
    if elevation < WATER_LEVEL + RUNWAY_MIN_ABOVE_WATER {
        return None;
    }
    // A runway is bidirectional, so a half-turn covers every distinct heading.
    let heading = hash01(h ^ 0x2545_f491) * std::f32::consts::PI;
    Some(RunwayInstance { x, z, heading, elevation, cell: (gx, gz) })
}

/// A deterministic 4-letter ident for a runway's grid cell under `seed`, e.g.
/// `"KQXR"`. Stable per world, distinct per seed — purely for the map overlay,
/// not tied to any real-world airport coding.
pub fn runway_ident(seed: u32, cell: (i32, i32)) -> String {
    let mut h = cell_hash(seed, cell.0, cell.1);
    // Lead with 'K' for a familiar look, then three letters peeled off the hash.
    let mut s = String::with_capacity(4);
    s.push('K');
    for _ in 0..3 {
        s.push((b'A' + (h % 26) as u8) as char);
        h /= 26;
    }
    s
}

/// Runways for every cell overlapping the world-space box, padded by one cell so
/// strips whose influence reaches in from a neighbour are included (water cells
/// yield nothing). Used to resolve a chunk's runways once, before the per-vertex
/// flatten.
pub fn runways_in_region(
    generator: &WorldGenerator,
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
) -> Vec<RunwayInstance> {
    let gx0 = (min_x / RUNWAY_SPACING).round() as i32 - 1;
    let gx1 = (max_x / RUNWAY_SPACING).round() as i32 + 1;
    let gz0 = (min_z / RUNWAY_SPACING).round() as i32 - 1;
    let gz1 = (max_z / RUNWAY_SPACING).round() as i32 + 1;
    let mut out = Vec::new();
    for gx in gx0..=gx1 {
        for gz in gz0..=gz1 {
            if let Some(inst) = runway_for_cell(generator, gx, gz) {
                out.push(inst);
            }
        }
    }
    out
}

/// Blends `natural` height toward the nearest runway's elevation using the
/// rectangle weight. The strongest (nearest) runway wins where they overlap.
/// Cheap: pure rectangle math, no noise — the caller resolves `runways` once.
pub fn flatten_against(runways: &[RunwayInstance], x: f32, z: f32, natural: f32) -> f32 {
    let mut best_w = 0.0_f32;
    let mut best_e = 0.0_f32;
    for r in runways {
        let w = r.flatten_weight(x, z);
        if w > best_w {
            best_w = w;
            best_e = r.elevation;
        }
    }
    natural + (best_e - natural) * best_w
}

/// The walkable ground height the landing gear should rest on at a single point:
/// the flattened terrain, raised to the asphalt surface where the point is over
/// pavement (so wheels sit on the runway, not the graded ground beneath it).
/// Resolves the nearby runways once; for one-off gear queries, not the per-vertex
/// mesh path (which uses [`flatten_against`] — no lift, since the slab mesh
/// provides the raised surface visually).
pub fn ground_height(generator: &WorldGenerator, x: f32, z: f32, natural: f32) -> f32 {
    let runways = runways_in_region(generator, x, z, x, z);
    let mut ground = flatten_against(&runways, x, z, natural);
    for r in &runways {
        if r.on_pavement(x, z) {
            ground = ground.max(r.elevation + RUNWAY_SURFACE_LIFT);
        }
    }
    ground
}

// --- Streaming the visible strips --------------------------------------------

/// Spawns runways near the camera and despawns ones that fall out of range, so
/// only a handful exist at once regardless of how far you fly. The streamed
/// radius tracks the terrain render distance so strips don't appear beyond the
/// terrain edge. A seed/scale change (generator rebuilt) wipes the set so the
/// new layout repopulates next frame.
pub fn stream_runways(
    mut commands: Commands,
    generator: Res<WorldGenerator>,
    manager: Res<ChunkManager>,
    materials: Res<RunwayMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&Transform, With<TerrainCamera>>,
    existing: Query<(Entity, &RunwaySlab)>,
    mut spawned: ResMut<SpawnedRunways>,
) {
    if generator.is_changed() {
        for (entity, _) in &existing {
            commands.entity(entity).despawn();
        }
        spawned.cells.clear();
        return; // repopulate around the camera next frame
    }

    let Ok(cam) = camera.single() else { return };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let cam_cx = (cam.translation.x / RUNWAY_SPACING).round() as i32;
    let cam_cz = (cam.translation.z / RUNWAY_SPACING).round() as i32;

    // Cull by actual distance (not cell count) so strips disappear right around
    // the terrain edge. Spawn within the view radius, despawn a little past it —
    // the gap is hysteresis so a strip at the boundary doesn't flicker.
    let view_m = manager.render_distance as f32 * CHUNK_SIZE;
    let keep_sq = view_m * view_m;
    let drop_sq = (view_m * 1.15).powi(2);
    let cell_radius = (view_m * 1.15 / RUNWAY_SPACING).ceil() as i32 + 1;

    for gx in (cam_cx - cell_radius)..=(cam_cx + cell_radius) {
        for gz in (cam_cz - cell_radius)..=(cam_cz + cell_radius) {
            if spawned.cells.contains(&(gx, gz)) {
                continue;
            }
            let Some(inst) = runway_for_cell(&generator, gx, gz) else { continue };
            if Vec2::new(inst.x, inst.z).distance_squared(cam_xz) <= keep_sq {
                spawn_runway(&mut commands, &mut meshes, &materials, &inst, (gx, gz));
                spawned.cells.insert((gx, gz));
            }
        }
    }

    for (entity, slab) in &existing {
        if slab.pos.distance_squared(cam_xz) > drop_sq {
            commands.entity(entity).despawn();
            spawned.cells.remove(&slab.cell);
        }
    }
}

// --- Runway lighting constants -----------------------------------------------

/// Height of light fixtures above the asphalt surface (metres).
const LIGHT_Y: f32 = 0.5;

/// Edge lights every this many metres along both sides of the runway.
const EDGE_LIGHT_SPACING: f32 = 120.0;

/// Lateral inset from the runway edge for the edge light row.
const EDGE_LIGHT_INSET: f32 = 1.5;

/// PointLight `range` (metres) for each light type. Controls how far the light
/// reaches before it cuts off — tune these independently to balance look vs. cost.
const RANGE_EDGE: f32 = 200.0;
const RANGE_THRESHOLD: f32 = 80.0;
const RANGE_REIL: f32 = 3000.0;
const RANGE_ALS: f32 = 200.0;

/// Spawns one runway: a volumetric asphalt slab plus centreline dashes and
/// threshold bars, all as children of a root placed at the runway's position,
/// elevation and heading. Geometry is authored in local space (centre at the
/// origin, length along local +Z) so the root transform handles placement.
fn spawn_runway(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &RunwayMaterials,
    inst: &RunwayInstance,
    cell: (i32, i32),
) {
    // Local slab-top sits RUNWAY_SURFACE_LIFT above the root (which is at the
    // graded ground elevation), so the asphalt is a raised pad with the rest of
    // its THICKNESS buried — visible thickness, no coplanar z-fight with terrain.
    const PAINT_Y: f32 = 0.1; // markings above the asphalt top
    const THICKNESS: f32 = 3.0;
    let surface_y = RUNWAY_SURFACE_LIFT;

    let root = Transform::from_xyz(inst.x, inst.elevation, inst.z)
        .with_rotation(Quat::from_rotation_y(inst.heading));

    let slab = RunwaySlab { cell, pos: Vec2::new(inst.x, inst.z) };
    commands
        .spawn((root, Visibility::default(), slab, PIXEL_LAYER))
        .with_children(|parent| {
            // Volumetric asphalt slab (top face at local surface_y).
            parent.spawn((
                Mesh3d(meshes.add(Cuboid::new(RUNWAY_WIDTH, THICKNESS, RUNWAY_LENGTH))),
                MeshMaterial3d(materials.asphalt.clone()),
                Transform::from_xyz(0.0, surface_y - THICKNESS * 0.5, 0.0),
                PIXEL_LAYER,
            ));

            // Dashed centreline.
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
                    Transform::from_xyz(0.0, surface_y + PAINT_Y, z),
                    PIXEL_LAYER,
                ));
            }

            // Threshold "piano key" bars at each end. `NoFrustumCulling` on the
            // zero-thickness paint planes: their bounding box has no height, so
            // Bevy's frustum test wrongly culls them at shallow angles.
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
                        Transform::from_xyz(k as f32 * bar_stride, surface_y + PAINT_Y, z),
                        NoFrustumCulling,
                        PIXEL_LAYER,
                    ));
                }
            }

            // --- Runway lights ---
            let light_y = surface_y + LIGHT_Y;
            let half_w = RUNWAY_WIDTH * 0.5 - EDGE_LIGHT_INSET;
            let n_edge = (RUNWAY_LENGTH / EDGE_LIGHT_SPACING) as i32 + 1;
            let edge_start = -half_len;
            let threshold_z = half_len - 2.0;
            let white = Color::srgb(1.0, 0.97, 0.88);
            let yellow = Color::srgb(1.0, 0.85, 0.1);
            let caution_start = half_len - 610.0;

            // Edge lights — white, last 610 m turns yellow (FAA caution zone).
            // Spaced widely with high intensity + range so fewer entities cover the strip.
            for i in 0..=n_edge {
                let z = edge_start + i as f32 * EDGE_LIGHT_SPACING;
                if z > half_len { break; }
                let color = if z >= caution_start { yellow } else { white };
                for side in [-1.0_f32, 1.0] {
                    parent.spawn((
                        PointLight {
                            color,
                            intensity: 500_000.0,
                            range: RANGE_EDGE,
                            radius: 0.3,
                            shadows_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(side * half_w, light_y, z),
                        PIXEL_LAYER,
                    ));
                }
            }

            // Threshold lights: one wide green + one wide red per end (centred).
            // A large radius fakes the spread of the full bar row with 2 lights instead of 8.
            for end_sign in [-1.0_f32, 1.0] {
                let tz = end_sign * threshold_z;
                parent.spawn((
                    PointLight {
                        color: Color::srgb(0.1, 1.0, 0.2),
                        intensity: 3_200_000.0,
                        range: RANGE_THRESHOLD,
                        radius: RUNWAY_WIDTH * 0.4,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(0.0, light_y, tz - end_sign * 1.0),
                    PIXEL_LAYER,
                ));
                parent.spawn((
                    PointLight {
                        color: Color::srgb(1.0, 0.08, 0.05),
                        intensity: 3_200_000.0,
                        range: RANGE_THRESHOLD,
                        radius: RUNWAY_WIDTH * 0.4,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(0.0, light_y, tz + end_sign * 1.0),
                    PIXEL_LAYER,
                ));

                // REIL: one bright strobe each side of the threshold.
                for side in [-1.0_f32, 1.0] {
                    parent.spawn((
                        PointLight {
                            color: Color::srgb(1.0, 0.15, 0.1),
                            intensity: 5_000_000.0,
                            range: RANGE_REIL,
                            radius: 1.5,
                            shadows_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(side * (RUNWAY_WIDTH * 0.5 + 5.0), light_y + 0.5, tz),
                        PIXEL_LAYER,
                        ReilLight,
                    ));
                }

                // Approach lighting (ALS): one wide light per bar, radius fakes the
                // 18 m cross-bar spread. 3 bars × 2 ends = 6 lights total.
                // Bars sequence toward the threshold ("the rabbit").
                const ALS_BARS: i32 = 3;
                const ALS_SPACING: f32 = 60.0;
                let end_i = if end_sign > 0.0 { 1_i32 } else { -1_i32 };
                for j in 1..=ALS_BARS {
                    let z = tz + end_sign * j as f32 * ALS_SPACING;
                    parent.spawn((
                        PointLight {
                            color: Color::srgb(1.0, 0.97, 0.88),
                            intensity: 2_000_000.0,
                            range: RANGE_ALS,
                            radius: 9.0,
                            shadows_enabled: false,
                            ..default()
                        },
                        Transform::from_xyz(0.0, light_y + 1.5, z),
                        PIXEL_LAYER,
                        AlsLight { index: j, end: end_i },
                    ));
                }
            }
        });
}

/// Animates REIL strobes and ALS sequencing bars every frame.
///
/// - REIL: flashes at ~1 Hz (50 ms on, 950 ms off).
/// - ALS: one bar is lit at a time, stepping toward the threshold at ~10 Hz,
///   giving the classic "rabbit" effect pilots use on final approach.
pub fn animate_runway_lights(
    time: Res<Time>,
    mut clock: ResMut<RunwayLightClock>,
    mut reil: Query<&mut PointLight, (With<ReilLight>, Without<AlsLight>)>,
    mut als: Query<(&AlsLight, &mut PointLight), Without<ReilLight>>,
) {
    clock.elapsed += time.delta_secs();
    let t = clock.elapsed;

    // REIL: 1 Hz flash — on for 50 ms at the top of each second.
    let reil_on = (t % 1.0) < 0.05;
    for mut light in &mut reil {
        light.intensity = if reil_on { 2_000_000.0 } else { 0.0 };
    }

    // ALS rabbit: cycle through bars farthest→nearest at 10 Hz (100 ms per step).
    // `active` is which bar index (1=nearest threshold, 3=farthest) is lit.
    const ALS_BARS: i32 = 3;
    const ALS_HZ: f32 = 2.0;
    // Step 0 = bar 3 (farthest), step 2 = bar 1 (nearest threshold).
    let step = ((t * ALS_HZ) as i32).rem_euclid(ALS_BARS);
    let active_index = ALS_BARS - step; // counts down 3→2→1→3→…
    for (als, mut light) in &mut als {
        light.intensity = if als.index == active_index { 2_000_000.0 } else { 0.0 };
    }
}
