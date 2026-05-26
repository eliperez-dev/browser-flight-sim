//! The streaming engine: spawn chunks around the camera, displace their meshes
//! against the noise field off the main thread, swap in LOD levels as the camera
//! moves, and despawn chunks that fall out of range. Ported from the parent
//! `bevy_sim` with three adaptations for this project:
//!   1. tracks the [`TerrainCamera`]-marked 3D camera (no `MainCamera` here),
//!   2. tags every chunk with `PIXEL_LAYER` so it renders to the pixel canvas,
//!   3. no water child / tree LOD (out of v1 scope).
//!
//! Performance rests on two ideas carried over from the original: the grid is
//! only re-scanned when the camera crosses a chunk boundary, and all noise
//! sampling happens in [`AsyncComputeTaskPool`] tasks that are polled and
//! rate-limited rather than run inline.

use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::AsyncComputeTaskPool;

use crate::camera::PIXEL_LAYER;

use super::chunk::{
    lod_for_distance_sq, Chunk, ChunkManager, ChunkTask, SharedTerrainMaterial,
    WorldGenerationSettings,
};
use super::generator::{WorldGenConfig, WorldGenerator, CHUNK_SIZE};
use super::TerrainCamera;

/// Seconds the config must sit unchanged before a rebuild fires. Long enough
/// that dragging a slider doesn't thrash the (single-threaded, on WASM) chunk
/// generator every frame, short enough to feel immediate when you let go.
const REGEN_DEBOUNCE: f32 = 0.25;

/// Rebuilds the world when [`WorldGenConfig`] changes. Streaming params
/// (render distance, chunks/frame) are mirrored into their runtime resources
/// immediately; a seed/scale change additionally rebuilds the [`WorldGenerator`]
/// and clears every chunk so [`generate_chunks`] repopulates from scratch. The
/// rebuild is debounced so dragging a slider settles before the world rebuilds.
pub fn regenerate_terrain(
    config: Res<WorldGenConfig>,
    time: Res<Time>,
    mut generator: ResMut<WorldGenerator>,
    mut manager: ResMut<ChunkManager>,
    mut settings: ResMut<WorldGenerationSettings>,
    chunks: Query<Entity, With<Chunk>>,
    mut commands: Commands,
    mut pending: Local<Option<f32>>,
    mut initialized: Local<bool>,
) {
    // Cheap params can update live without a rebuild; mirror them every time the
    // config changes (and once on startup).
    if config.is_changed() {
        manager.render_distance = config.render_distance;
        settings.max_chunks_per_frame = config.max_chunks_per_frame;
    }

    // The resource is "changed" the frame it's inserted; treat that first sighting
    // as setup (generator already matches the config) rather than a rebuild.
    if !*initialized {
        *initialized = true;
        return;
    }

    if config.is_changed() {
        *pending = Some(REGEN_DEBOUNCE);
    }
    let Some(remaining) = *pending else { return };
    let remaining = remaining - time.delta_secs();
    if remaining > 0.0 {
        *pending = Some(remaining);
        return;
    }
    *pending = None;

    // Rebuild and wipe — generate_chunks (next in the chain) refills the disc.
    *generator = WorldGenerator::from_config(&config);
    for entity in &chunks {
        commands.entity(entity).despawn();
    }
    manager.spawned_chunks.clear();
    manager.to_spawn.clear();
    manager.lod_to_update.clear();
    manager.last_camera_chunk = None; // force a full rescan
}

/// Camera position rounded to chunk coordinates, or `None` if the camera isn't
/// available this frame.
fn camera_chunk(camera: &Query<&Transform, With<TerrainCamera>>) -> Option<(i32, i32)> {
    let t = camera.single().ok()?.translation;
    Some((
        (t.x / CHUNK_SIZE).round() as i32,
        (t.z / CHUNK_SIZE).round() as i32,
    ))
}

/// Re-scans the render disc when the camera enters a new chunk, queues missing
/// chunks nearest-first, then spawns up to `max_chunks_per_frame` of them as
/// hidden, flat tiles. [`displace_new_chunks`] turns the flat tile into terrain.
pub fn generate_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<SharedTerrainMaterial>,
    mut manager: ResMut<ChunkManager>,
    camera: Query<&Transform, With<TerrainCamera>>,
    settings: Res<WorldGenerationSettings>,
) {
    let Some((cam_x, cam_z)) = camera_chunk(&camera) else { return };

    // Only re-scan when the camera moved to a new chunk — the common case
    // (camera within the same chunk) does no scanning work at all.
    if manager.last_camera_chunk != Some((cam_x, cam_z)) {
        manager.last_camera_chunk = Some((cam_x, cam_z));

        let render_distance = manager.render_distance;
        let render_distance_sq = (render_distance as f32).powi(2);
        manager.to_spawn.clear();

        for x in (cam_x - render_distance)..=(cam_x + render_distance) {
            for z in (cam_z - render_distance)..=(cam_z + render_distance) {
                let dx = (x - cam_x) as f32;
                let dz = (z - cam_z) as f32;
                if dx * dx + dz * dz <= render_distance_sq
                    && !manager.spawned_chunks.contains(&(x, z))
                {
                    manager.to_spawn.push((x, z));
                }
            }
        }

        manager.to_spawn.sort_by(|a, b| {
            let da = ((a.0 - cam_x).pow(2) + (a.1 - cam_z).pow(2)) as f32;
            let db = ((b.0 - cam_x).pow(2) + (b.1 - cam_z).pow(2)) as f32;
            // Spawn nearest-last so `pop()` (cheap) yields nearest-first.
            db.partial_cmp(&da).unwrap()
        });
    }

    let render_distance_sq = (manager.render_distance as f32).powi(2);
    let mut spawned = 0;
    while spawned < settings.max_chunks_per_frame {
        let Some((x, z)) = manager.to_spawn.pop() else { break };

        let dx = (x - cam_x) as f32;
        let dz = (z - cam_z) as f32;
        let distance_sq = dx * dx + dz * dz;

        // The camera may have moved since the scan; re-check range and dedupe.
        if distance_sq > render_distance_sq || manager.spawned_chunks.contains(&(x, z)) {
            continue;
        }
        manager.spawned_chunks.insert((x, z));

        let lod = lod_for_distance_sq(distance_sq, &manager);
        commands.spawn((
            Mesh3d(meshes.add(
                Plane3d::default()
                    .mesh()
                    .size(CHUNK_SIZE, CHUNK_SIZE)
                    .subdivisions(lod),
            )),
            MeshMaterial3d(material.handle.clone()),
            Transform::from_xyz(x as f32 * CHUNK_SIZE, 0.0, z as f32 * CHUNK_SIZE),
            Chunk { x, z, current_lod: lod },
            Visibility::Hidden,
            PIXEL_LAYER,
        ));
        spawned += 1;
    }
}

/// Displaces every freshly spawned chunk's vertices against the noise field and
/// paints per-vertex colours, off the main thread. The chunk stays hidden until
/// [`apply_chunk_meshes`] swaps the finished mesh in.
pub fn displace_new_chunks(
    mut commands: Commands,
    query: Query<(Entity, &Mesh3d, &Transform), Added<Chunk>>,
    world_gen: Res<WorldGenerator>,
    meshes: Res<Assets<Mesh>>,
) {
    let pool = AsyncComputeTaskPool::get();
    for (entity, mesh_handle, transform) in &query {
        let Some(mesh) = meshes.get(mesh_handle) else { continue };
        let mut mesh = mesh.clone();
        let world_gen = world_gen.clone();
        let origin = transform.translation;

        let task = pool.spawn(async move {
            displace_mesh(&mut mesh, &world_gen, origin);
            mesh
        });

        commands.queue(move |world: &mut World| {
            if let Ok(mut e) = world.get_entity_mut(entity) {
                e.insert(ChunkTask { task, new_handle: None });
            }
        });
    }
}

/// Shared mesh-displacement kernel used by both the initial build and LOD
/// rebuilds: lift each vertex to its terrain height and colour it, split the mesh
/// into independent triangles, then give each face a single flat colour and flat
/// normal. The per-face colour is what makes terrain read as crisp facets rather
/// than a blurry gradient — without it the GPU smoothly interpolates the three
/// corner colours across every triangle.
fn displace_mesh(mesh: &mut Mesh, world_gen: &WorldGenerator, origin: Vec3) {
    // 1. Displace each vertex to its terrain height and record its colour.
    let mut colors: Vec<[f32; 4]> = Vec::new();
    if let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        colors.reserve(positions.len());
        for pos in positions.iter_mut() {
            let (h, color) = world_gen.surface(pos[0] + origin.x, pos[2] + origin.z);
            pos[1] = h;
            colors.push(color);
        }
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);

    // 2. Split shared vertices so every triangle owns its three corners. This is
    //    required for both flat normals and flat colours.
    mesh.duplicate_vertices();

    // 3. Collapse each triangle's three colours to their average, so the whole
    //    facet is one solid colour (crisp, low-poly look).
    if let Some(VertexAttributeValues::Float32x4(cols)) = mesh.attribute_mut(Mesh::ATTRIBUTE_COLOR)
    {
        for tri in cols.chunks_mut(3) {
            if let [a, b, c] = tri {
                let avg = [
                    (a[0] + b[0] + c[0]) / 3.0,
                    (a[1] + b[1] + c[1]) / 3.0,
                    (a[2] + b[2] + c[2]) / 3.0,
                    (a[3] + b[3] + c[3]) / 3.0,
                ];
                *a = avg;
                *b = avg;
                *c = avg;
            }
        }
    }

    // 4. One normal per face — flat (faceted) shading.
    mesh.compute_flat_normals();
}

/// Polls in-flight chunk tasks nearest-first, swaps finished meshes in, and
/// reveals the chunk. Rate-limited so a burst of completions can't spike a frame.
pub fn apply_chunk_meshes(
    mut commands: Commands,
    mut tasks: Query<(Entity, &Mesh3d, &mut ChunkTask, &Chunk)>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&Transform, With<TerrainCamera>>,
    settings: Res<WorldGenerationSettings>,
) {
    let (cam_x, cam_z) = camera_chunk(&camera).unwrap_or((0, 0));

    let mut ordered: Vec<(Entity, f32)> = tasks
        .iter()
        .map(|(e, _, _, c)| {
            let dx = (c.x - cam_x) as f32;
            let dz = (c.z - cam_z) as f32;
            (e, dx * dx + dz * dz)
        })
        .collect();
    ordered.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    let mut processed = 0;
    for (entity, _) in ordered {
        if processed >= settings.max_chunks_per_frame {
            break;
        }
        let Ok((entity, mesh_handle, mut task, _)) = tasks.get_mut(entity) else { continue };
        let Some(new_mesh) = future::block_on(future::poll_once(&mut task.task)) else { continue };

        if let Some(new_handle) = task.new_handle.take() {
            // LOD rebuild: write into the new handle, point the entity at it, drop the old.
            if let Some(slot) = meshes.get_mut(&new_handle) {
                *slot = new_mesh;
            }
            commands.entity(entity).try_insert(Mesh3d(new_handle));
            meshes.remove(mesh_handle);
        } else if let Some(slot) = meshes.get_mut(mesh_handle) {
            // Initial build: write back into the existing handle.
            *slot = new_mesh;
        }

        commands.entity(entity).try_remove::<ChunkTask>();
        commands.entity(entity).try_insert(Visibility::Visible);
        processed += 1;
    }
}

/// When the camera crosses a chunk boundary, finds chunks whose distance band
/// now calls for a different subdivision level and rebuilds their meshes (async,
/// rate-limited). Chunks already mid-build are skipped via the `Without` filter.
pub fn update_chunk_lod(
    mut commands: Commands,
    camera: Query<&Transform, With<TerrainCamera>>,
    mut chunks: Query<(Entity, &mut Chunk, &Transform), Without<ChunkTask>>,
    mut meshes: ResMut<Assets<Mesh>>,
    world_gen: Res<WorldGenerator>,
    mut manager: ResMut<ChunkManager>,
    settings: Res<WorldGenerationSettings>,
    mut last_cam: Local<Option<(i32, i32)>>,
) {
    let Some((cam_x, cam_z)) = camera_chunk(&camera) else { return };

    if *last_cam != Some((cam_x, cam_z)) {
        *last_cam = Some((cam_x, cam_z));

        let mut candidates: Vec<(Entity, f32)> = Vec::new();
        for (entity, chunk, _) in &chunks {
            let dx = (chunk.x - cam_x) as f32;
            let dz = (chunk.z - cam_z) as f32;
            let distance_sq = dx * dx + dz * dz;
            if lod_for_distance_sq(distance_sq, &manager) != chunk.current_lod {
                candidates.push((entity, distance_sq));
            }
        }
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        manager.lod_to_update = candidates.into_iter().map(|(e, _)| e).collect();
    }

    let pool = AsyncComputeTaskPool::get();
    let mut processed = 0;
    while processed < settings.max_chunks_per_frame {
        let Some(entity) = manager.lod_to_update.pop() else { break };
        let Ok((entity, mut chunk, transform)) = chunks.get_mut(entity) else { continue };

        let dx = (chunk.x - cam_x) as f32;
        let dz = (chunk.z - cam_z) as f32;
        let desired = lod_for_distance_sq(dx * dx + dz * dz, &manager);
        if desired == chunk.current_lod {
            continue;
        }

        let new_handle = meshes.add(
            Plane3d::default()
                .mesh()
                .size(CHUNK_SIZE, CHUNK_SIZE)
                .subdivisions(desired),
        );
        let Some(base) = meshes.get(&new_handle) else { continue };
        let mut mesh = base.clone();
        let world_gen = world_gen.clone();
        let origin = transform.translation;

        let task = pool.spawn(async move {
            displace_mesh(&mut mesh, &world_gen, origin);
            mesh
        });

        commands.queue(move |world: &mut World| {
            if let Ok(mut e) = world.get_entity_mut(entity) {
                e.insert(ChunkTask { task, new_handle: Some(new_handle) });
            }
        });
        chunk.current_lod = desired;
        processed += 1;
    }
}

/// Despawns chunks past `render_distance + 1`, farthest-first and rate-limited,
/// and frees their mesh handles so memory doesn't grow on a long flight.
pub fn despawn_out_of_bounds_chunks(
    mut commands: Commands,
    camera: Query<&Transform, With<TerrainCamera>>,
    chunks: Query<(Entity, &Chunk, &Mesh3d)>,
    mut manager: ResMut<ChunkManager>,
    mut meshes: ResMut<Assets<Mesh>>,
    settings: Res<WorldGenerationSettings>,
) {
    let Some((cam_x, cam_z)) = camera_chunk(&camera) else { return };
    let despawn_sq = ((manager.render_distance + 1) as f32).powi(2);

    let mut out: Vec<(Entity, i32, i32, f32, Handle<Mesh>)> = Vec::new();
    for (entity, chunk, mesh) in &chunks {
        let dx = (chunk.x - cam_x) as f32;
        let dz = (chunk.z - cam_z) as f32;
        let distance_sq = dx * dx + dz * dz;
        if distance_sq > despawn_sq {
            out.push((entity, chunk.x, chunk.z, distance_sq, mesh.0.clone()));
        }
    }
    out.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());

    for (entity, x, z, _, mesh) in out.into_iter().take(settings.max_chunks_per_frame * 2) {
        manager.spawned_chunks.remove(&(x, z));
        meshes.remove(&mesh);
        commands.entity(entity).despawn();
    }
}
