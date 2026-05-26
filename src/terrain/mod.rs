//! Streaming procedural terrain, ported from the parent `bevy_sim` and tuned for
//! the browser/WASM target. Add [`TerrainPlugin`] and tag the 3D camera with
//! [`TerrainCamera`]; chunks then stream in around it automatically.
//!
//! The terrain is purely visual — the aircraft interacts with it by sampling
//! [`WorldGenerator`] in the landing-gear physics, not via colliders.

mod chunk;
mod generator;
mod runway;
mod streaming;

pub use generator::{WorldGenConfig, WorldGenerator};

use bevy::prelude::*;

use chunk::{ChunkManager, SharedTerrainMaterial, WorldGenerationSettings};
use runway::RunwayMaterials;

/// Marks the 3D camera that terrain streams around. Exactly one entity should
/// carry this (the `PIXEL_LAYER` `Camera3d` in `main::setup`).
#[derive(Component)]
pub struct TerrainCamera;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        let config = WorldGenConfig::default();
        app.insert_resource(WorldGenerator::from_config(&config))
            .insert_resource(config)
            .init_resource::<ChunkManager>()
            .init_resource::<WorldGenerationSettings>()
            .add_systems(Startup, setup_terrain_material)
            .add_systems(
                Update,
                (
                    // Regen first so a config change clears stale chunks before
                    // generate_chunks repopulates from the new generator;
                    // sync_runways then rebuilds the strips for the new layout.
                    streaming::regenerate_terrain,
                    runway::sync_runways,
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
    commands.insert_resource(RunwayMaterials::new(&mut materials));
}
