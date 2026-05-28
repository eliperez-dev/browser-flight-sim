//! A single low-poly water plane at sea level.
//!
//! The whole ocean is one flat quad — two triangles — that follows the terrain
//! camera horizontally while staying pinned to `sea_level` in Y. It's far cheaper
//! than meshed water and reads fine at this art style: the terrain's ocean biomes
//! already sink into deep basins (see [`crate::terrain`]'s `ocean_depth`), so the
//! flat plane fills them and laps at the low coastlines.
//!
//! Tuned live from the F3 debug menu via [`WaterSettings`]: colour / roughness /
//! metallic update the shared material in place; `sea_level` just moves the plane.

use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;
use crate::terrain::TerrainCamera;

/// Edge length (m) of the water quad. Comfortably past the fog horizon
/// so its edge is never visible as it snaps
/// along with the camera.
const WATER_PLANE_SIZE: f32 = 80_000.0;

/// Live-editable water look & level, surfaced as F3 sliders. `Clone`/`PartialEq`
/// let the apply system react only to real changes.
#[derive(Resource, Clone, PartialEq)]
pub struct WaterSettings {
    pub enabled: bool,
    /// World-space Y the surface sits at. The terrain's land baseline is ~0, ocean
    /// basins are well below, so 0 puts water at the coastline; nudge negative to
    /// keep water in basins, positive to flood the flats.
    pub sea_level: f32,
    /// Linear RGB base colour.
    pub color: [f32; 3],
    /// Lower = glossier, sharper sun glint.
    pub perceptual_roughness: f32,
    /// 0 = dielectric water, up to 1 = mirror-like.
    pub metallic: f32,
}

impl Default for WaterSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            sea_level: 0.0,
            color: [0.04, 0.18, 0.32],
            perceptual_roughness: 0.08,
            metallic: 0.8,
        }
    }
}

/// Marks the single water-surface entity.
#[derive(Component)]
struct Water;

/// Handle to the one material every (i.e. the only) water quad shares, so the
/// apply system can edit colour/roughness/metallic in place.
#[derive(Resource)]
struct WaterMaterial(Handle<StandardMaterial>);

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WaterSettings>()
            .add_systems(Startup, spawn_water)
            .add_systems(Update, (follow_camera, apply_water_settings));
    }
}

/// Builds the flat water material from a [`WaterSettings`] snapshot.
fn water_material(s: &WaterSettings) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::linear_rgb(s.color[0], s.color[1], s.color[2]),
        perceptual_roughness: s.perceptual_roughness,
        metallic: s.metallic,
        ..default()
    }
}

/// Spawns the single water quad on the pixel layer, pinned to `sea_level`.
fn spawn_water(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    settings: Res<WaterSettings>,
) {
    let handle = materials.add(water_material(&settings));
    commands.insert_resource(WaterMaterial(handle.clone()));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(WATER_PLANE_SIZE, WATER_PLANE_SIZE))),
        MeshMaterial3d(handle),
        Transform::from_xyz(0.0, settings.sea_level, 0.0),
        // Flat ocean shouldn't cast onto the seabed or self-darken from terrain.
        NotShadowCaster,
        NotShadowReceiver,
        
        Water,
        PIXEL_LAYER,
        if settings.enabled { Visibility::Visible } else { Visibility::Hidden },
    ));
}

/// Keeps the quad centred under the camera horizontally so its edge stays beyond
/// the fog, while holding Y at `sea_level`.
fn follow_camera(
    settings: Res<WaterSettings>,
    camera: Query<&Transform, (With<TerrainCamera>, Without<Water>)>,
    mut water: Query<&mut Transform, (With<Water>, Without<TerrainCamera>)>,
) {
    let Ok(cam) = camera.single() else { return };
    let Ok(mut tf) = water.single_mut() else { return };
    tf.translation.x = cam.translation.x;
    tf.translation.z = cam.translation.z;
    tf.translation.y = settings.sea_level;
}

/// Applies live look edits: rebuilds the shared material on change and toggles
/// the quad's visibility. Only touches assets when the settings actually changed.
fn apply_water_settings(
    settings: Res<WaterSettings>,
    material: Res<WaterMaterial>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut water: Query<&mut Visibility, With<Water>>,
) {
    if !settings.is_changed() {
        return;
    }
    if let Some(mat) = materials.get_mut(&material.0) {
        *mat = water_material(&settings);
    }
    if let Ok(mut vis) = water.single_mut() {
        *vis = if settings.enabled { Visibility::Visible } else { Visibility::Hidden };
    }
}
