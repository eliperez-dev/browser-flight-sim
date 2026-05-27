//! Components and resources for the streaming chunk grid. The streaming systems
//! that drive these live in [`super::streaming`].

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::tasks::Task;

/// A single terrain tile. `(x, z)` are integer chunk coordinates (world position
/// is `coord * CHUNK_SIZE`); `current_lod` is the subdivision count its mesh was
/// last built at, so the LOD pass can detect when it needs rebuilding.
#[derive(Component)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub current_lod: u32,
}

/// In-flight async mesh build for a chunk. `new_handle` is `Some` only for LOD
/// rebuilds, where a fresh (higher/lower-res) mesh replaces the old handle; for
/// the initial build the displaced mesh is written back into the existing handle.
#[derive(Component)]
pub struct ChunkTask {
    pub task: Task<Mesh>,
    pub new_handle: Option<Handle<Mesh>>,
}

/// Bookkeeping for which chunks exist and what still needs doing. The queues let
/// the streaming systems rate-limit work across frames instead of spawning the
/// whole render disc at once.
#[derive(Resource)]
pub struct ChunkManager {
    /// Chunk coords currently spawned, for O(1) "do we already have it?" checks.
    pub spawned_chunks: HashSet<(i32, i32)>,
    /// The camera chunk we last scanned from; re-scan only when this changes.
    pub last_camera_chunk: Option<(i32, i32)>,
    /// Pending spawns, sorted nearest-first.
    pub to_spawn: Vec<(i32, i32)>,
    /// Chunks whose LOD needs rebuilding, sorted nearest-first.
    pub lod_to_update: Vec<Entity>,
    /// View radius in chunks.
    pub render_distance: i32,
    /// `(max_distance_in_chunks, subdivisions)`, ascending distance. A chunk uses
    /// the subdivisions of the first band it falls within. Nearest is capped low
    /// so no single chunk's noise pass hitches the WASM main thread.
    pub lod_levels: [(f32, u32); 4],
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            spawned_chunks: HashSet::new(),
            last_camera_chunk: None,
            to_spawn: Vec::new(),
            lod_to_update: Vec::new(),
            render_distance: 28, // ~6 km at CHUNK_SIZE = 500 m
            lod_levels: [
                (2.0, 16),
                (6.0, 8),
                (20.0, 4),
                (30.0, 2),
            ],
        }
    }
}

/// How much chunk work may start per frame. **Kept low on purpose:** on WASM the
/// async task pool runs on the main thread, so this directly bounds peak
/// per-frame noise cost. Native could afford ~100; the browser wants ~2–4.
#[derive(Resource)]
pub struct WorldGenerationSettings {
    pub max_chunks_per_frame: usize,
}

impl Default for WorldGenerationSettings {
    fn default() -> Self {
        Self { max_chunks_per_frame: 3 }
    }
}

/// One shared material for every terrain chunk — vertex colours (from the height
/// ramp) carry the visual variety, so a single handle keeps batching tight.
#[derive(Resource)]
pub struct SharedTerrainMaterial {
    pub handle: Handle<StandardMaterial>,
}

/// Subdivision count for a chunk at the given squared chunk-distance from the
/// camera, looked up in [`ChunkManager::lod_levels`].
pub fn lod_for_distance_sq(distance_sq: f32, manager: &ChunkManager) -> u32 {
    let distance = distance_sq.sqrt();
    for (max_distance, subdivisions) in &manager.lod_levels {
        if distance <= *max_distance {
            return *subdivisions;
        }
    }
    manager.lod_levels.last().unwrap().1
}
