//! Streaming procedural terrain, ported from the parent `bevy_sim` and tuned for
//! the browser/WASM target. Add [`TerrainPlugin`] and tag the 3D camera with
//! [`TerrainCamera`]; chunks then stream in around it automatically.
//!
//! The terrain is purely visual — the aircraft interacts with it by sampling
//! [`WorldGenerator`] in the landing-gear physics, not via colliders.

mod chunk;
mod generator;
mod streaming;

pub use generator::WorldGenerator;

use bevy::prelude::*;

use chunk::{ChunkManager, SharedTerrainMaterial, WorldGenerationSettings};

/// Marks the 3D camera that terrain streams around. Exactly one entity should
/// carry this (the `PIXEL_LAYER` `Camera3d` in `main::setup`).
#[derive(Component)]
pub struct TerrainCamera;

/// Seed for the world. Fixed for now; expose later if we want regenerate-on-demand.
const WORLD_SEED: u32 = 3;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WorldGenerator::new(WORLD_SEED))
            .init_resource::<ChunkManager>()
            .init_resource::<WorldGenerationSettings>()
            .add_systems(Startup, setup_terrain_material)
            .add_systems(
                Update,
                (
                    streaming::generate_chunks,
                    streaming::displace_new_chunks,
                    streaming::apply_chunk_meshes,
                    streaming::update_chunk_lod,
                )
                    .chain(),
            )
            .add_systems(PostUpdate, streaming::despawn_out_of_bounds_chunks);
    }
}

/// Builds the one material shared by every chunk. Vertex colours carry the
/// terrain palette, so the base colour is white and roughness is high (matte).
fn setup_terrain_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(SharedTerrainMaterial {
        handle: materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            ..default()
        }),
    });
}
