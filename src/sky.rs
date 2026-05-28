//! Day-night cycle: a directional "sun" that orbits the world, driving sky
//! colour, sun illuminance, ambient fill, fog tint and a field of twinkling
//! stars. Ported from the parent `bevy_sim` (`day_cycle.rs`) and adapted to this
//! project's pixel-render camera ([`TerrainCamera`]) and [`FogSettings`]-driven
//! fog.
//!
//! Add [`SkyPlugin`]. Tune it live from the F3 debug menu ("Sky / Day-Night"):
//! `time_of_day` scrubs the clock (0 = midnight, 0.5 = noon), `speed` advances
//! it automatically and `inclination` tilts the sun's orbit off the vertical.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;
use crate::terrain::TerrainCamera;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Peak (noon) illuminance of the sun, in lux. Scaled down toward 0 at night.
const MAX_ILLUMINANCE: f32 = 11_000.0;
/// Ambient fill brightness at night / full day. Keeps shadowed faces readable.
const AMBIENT_NIGHT: f32 = 60.0;
const AMBIENT_DAY: f32 = 400.0;

/// How far from the camera the star sphere sits. Stars have fog disabled, so
/// this only needs to clear the terrain; the projection is infinite-far, so it
/// is never clipped.
const STAR_DISTANCE: f32 = 15_000.0;
/// Base size multiplier for star billboards at [`STAR_DISTANCE`].
const STAR_SCALE: f32 = 3.0;
/// Number of stars scattered over the celestial sphere. The whole field is a
/// single billboarded mesh (one draw call), so this can scale into the
/// thousands cheaply — see [`spawn_stars`].
const NUM_STARS: usize = 2500;

/// How far the visible sun disc sits from the camera. Slightly nearer than the
/// stars so it draws in front of them.
const SUN_DISTANCE: f32 = 13_000.0;
/// World-space radius of the sun disc at [`SUN_DISTANCE`] (≈1.3° across).
const SUN_RADIUS: f32 = 300.0;

// ---------------------------------------------------------------------------
// Resource & components
// ---------------------------------------------------------------------------

/// Global clock for the day-night cycle.
#[derive(Resource)]
pub struct DayNightCycle {
    /// 0.0 = midnight, 0.25 = sunrise, 0.5 = noon, 0.75 = sunset. Wraps at 1.0.
    pub time_of_day: f32,
    /// Fraction of the cycle advanced per second (0 freezes time).
    pub speed: f32,
    /// Tilt of the sun's orbit away from straight overhead, in radians.
    pub inclination: f32,
    /// When true, the cycle tints the camera fog to match the sky; when false,
    /// fog keeps whatever colour the Fog panel sets.
    pub tint_fog: bool,
    /// How far the fog tint is desaturated toward grey relative to the sky
    /// colour (0 = identical to the sky, 1 = fully grey). A small amount reads
    /// as real aerial haze; too much breaks the seamless terrain↔sky blend.
    pub fog_haze: f32,
}

impl Default for DayNightCycle {
    fn default() -> Self {
        Self {
            time_of_day: 0.35, // mid-morning so the world starts lit
            speed: 0.005,
            inclination: 0.3,
            tint_fog: true,
            fog_haze: 0.05,
        }
    }
}

/// Marker for the directional light acting as the sun (invisible; it only lights
/// the scene). The visible disc in the sky is a separate [`SunDisc`] entity.
#[derive(Component)]
pub struct Sun;

/// Marker for the emissive sphere drawn in the sky to represent the sun. Tracks
/// the camera and the sun direction each frame in [`update_daylight_cycle`].
#[derive(Component)]
pub struct SunDisc;

/// One star on the celestial sphere. `offset` is its fixed direction on the
/// sphere; the rest randomise its twinkle so they don't pulse in unison. This
/// is plain data held in [`StarField`] — stars are *not* separate entities.
struct Star {
    offset: Vec3,
    brightness: f32,
    phase: f32,
    twinkle_speed: f32,
}

/// The entire star field as a single mesh. Rather than one entity (and one
/// 20k-triangle sphere) per star, every star is a camera-facing quad packed
/// into one mesh drawn in a single call. [`update_daylight_cycle`] rewrites the
/// quad positions (for billboarding) and per-vertex alpha (for the fade and
/// twinkle) each frame — a few thousand cheap vertex writes instead of mutating
/// thousands of materials and drawing millions of triangles.
#[derive(Resource)]
pub struct StarField {
    stars: Vec<Star>,
    mesh: Handle<Mesh>,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DayNightCycle>()
            .add_systems(Startup, (spawn_sun, spawn_stars))
            // Run after the fog system so that on a frame where editing the Fog
            // panel rebuilds DistanceFog from FogSettings, our time-of-day tint
            // is applied last and isn't clobbered.
            .add_systems(Update, update_daylight_cycle.after(crate::fog::apply_fog));
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/// Spawns the directional "sun" light and its visible disc. Both are driven
/// every frame by [`update_daylight_cycle`] — the light's rotation sets the
/// lighting direction, the disc is positioned in the sky to match.
fn spawn_sun(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        DirectionalLight {
            illuminance: MAX_ILLUMINANCE,
            shadows_enabled: true,
            ..default()
        },
        Transform::default(),
        Sun,
        PIXEL_LAYER,
    ));

    // Visible disc: a unit sphere scaled to SUN_RADIUS. It is `unlit` so its
    // colour is rendered straight from `base_color`, bypassing scene exposure
    // and lighting (which otherwise desaturate the warm hue toward white), and
    // has fog disabled so the haze never tints it.
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.95, 0.8),
            unlit: true,
            fog_enabled: false,
            ..default()
        })),
        Transform::from_scale(Vec3::splat(SUN_RADIUS)),
        // The disc tracks the camera, so without this its shadow map blob
        // follows the camera as a square patch on the ground below.
        NotShadowCaster,
        SunDisc,
        PIXEL_LAYER,
    ));
}

/// Scatters [`NUM_STARS`] stars uniformly over a sphere and packs them into one
/// billboarded mesh (4 vertices + 2 triangles per star) drawn with a single
/// shared `unlit` material. The vertex buffers are placeholders here;
/// [`update_daylight_cycle`] fills in the billboard positions and fade/twinkle
/// alpha every frame.
fn spawn_stars(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Deterministic PRNG (no `rand` dependency): a 64-bit xorshift seeded once,
    // so the constellation is identical every run.
    let mut rng_state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rand_f32 = move || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        // Top 24 bits → [0, 1).
        (rng_state >> 40) as f32 / (1u32 << 24) as f32
    };

    let mut stars = Vec::with_capacity(NUM_STARS);
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(NUM_STARS * 4);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(NUM_STARS * 4);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(NUM_STARS * 4);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(NUM_STARS * 4);
    let mut indices: Vec<u32> = Vec::with_capacity(NUM_STARS * 6);

    for i in 0..NUM_STARS {
        let phi = rand_f32() * std::f32::consts::TAU;
        let theta = rand_f32() * std::f32::consts::PI;

        let x = theta.sin() * phi.cos();
        let y = theta.cos();
        let z = theta.sin() * phi.sin();

        let offset = Vec3::new(x, y, z).normalize();
        let brightness = 0.7 + rand_f32() * 0.5;
        let phase = rand_f32() * std::f32::consts::TAU;
        let twinkle_speed = 3.0 + rand_f32() * 4.0;
        stars.push(Star { offset, brightness, phase, twinkle_speed });

        // Four placeholder vertices per star (overwritten each frame). Normals
        // are unused (unlit) and UVs unused (untextured), but kept so the mesh
        // carries StandardMaterial's expected attribute set. Start fully
        // transparent so the field is invisible until the first night update.
        for _ in 0..4 {
            positions.push([0.0, 0.0, 0.0]);
            normals.push([0.0, 0.0, 1.0]);
            colors.push([1.0, 1.0, 1.0, 0.0]);
        }
        uvs.push([0.0, 0.0]);
        uvs.push([1.0, 0.0]);
        uvs.push([1.0, 1.0]);
        uvs.push([0.0, 1.0]);

        let b = (i * 4) as u32;
        indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    let mesh = meshes.add(mesh);

    // One shared material for the whole field: unlit white tinted by the
    // per-vertex colour, alpha-blended, fog disabled, and culling off so the
    // billboards show regardless of winding.
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        fog_enabled: false,
        ..default()
    });

    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(material),
        Transform::default(),
        NotShadowCaster,
        PIXEL_LAYER,
    ));

    commands.insert_resource(StarField { stars, mesh });
}

// ---------------------------------------------------------------------------
// Per-frame update
// ---------------------------------------------------------------------------

/// Advances the clock, swings the sun, and recolours the world to match the
/// time of day: sun illuminance, ambient fill, the camera's sky (clear) colour,
/// an optional fog tint, and the visibility / twinkle of every star.
fn update_daylight_cycle(
    time: Res<Time>,
    mut cycle: ResMut<DayNightCycle>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut sun_q: Query<
        (&mut Transform, &mut DirectionalLight),
        (With<Sun>, Without<SunDisc>, Without<TerrainCamera>),
    >,
    mut disc_q: Query<
        (&mut Transform, &MeshMaterial3d<StandardMaterial>),
        (With<SunDisc>, Without<Sun>, Without<TerrainCamera>),
    >,
    mut cam_q: Query<
        (&Transform, &mut Camera, Option<&mut DistanceFog>),
        (With<TerrainCamera>, Without<Sun>, Without<SunDisc>),
    >,
    star_field: Res<StarField>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    cycle.time_of_day = (cycle.time_of_day + cycle.speed * time.delta_secs()) % 1.0;

    // Orbit the sun around the world's X axis, then tilt the whole orbit by the
    // inclination around Z so it doesn't pass straight through the zenith.
    let angle = cycle.time_of_day * std::f32::consts::TAU;
    let final_rotation = Quat::from_rotation_z(cycle.inclination) * Quat::from_rotation_x(angle);
    let sun_dir = final_rotation * Vec3::NEG_Z;
    // How far above the horizon the sun is (1 = overhead, <0 = below horizon).
    let up_dot = sun_dir.dot(Vec3::NEG_Y);
    let daylight = ((up_dot + 0.1) * 5.0).clamp(0.0, 1.0);
    // Peaks while the sun is near the horizon → warm sunrise/sunset band.
    let sunset_factor = (1.0 - (up_dot.abs() / 0.34)).clamp(0.0, 1.0);

    // Blend the sky colour: night → day, warmed toward orange near the horizon.
    let night_sky = Vec3::new(0.02, 0.02, 0.06);
    let day_sky = Vec3::new(0.45, 0.65, 0.9);
    let sunset_sky = Vec3::new(0.90, 0.45, 0.2);
    let sky = night_sky.lerp(day_sky, daylight).lerp(sunset_sky, sunset_factor);
    let sky_color = Color::srgb(sky.x, sky.y, sky.z);

    if let Ok((mut transform, mut light)) = sun_q.single_mut() {
        transform.rotation = final_rotation;
        light.illuminance = daylight * MAX_ILLUMINANCE;
    }

    ambient.brightness = AMBIENT_NIGHT + (AMBIENT_DAY - AMBIENT_NIGHT) * daylight;

    // Fog base tint: a desaturated version of the sky reads as real aerial
    // haze (distant terrain inscatters toward this colour). Desaturating toward
    // luminance-grey keeps night fog dark while taking the edge off bright days.
    let lum = sky.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    let fog_vec = sky.lerp(Vec3::splat(lum), cycle.fog_haze.clamp(0.0, 1.0));
    let fog_color = Color::srgb(fog_vec.x, fog_vec.y, fog_vec.z);

    // Sun-glow tint of the fog: warm white by day, deep orange at the horizon.
    // (Its strength is naturally gated by the sun's illuminance, so it fades out
    // on its own at night.)
    let day_glow = Vec3::new(1.0, 0.95, 0.85);
    let sunset_glow = Vec3::new(1.0, 0.45, 0.15);
    let glow = day_glow.lerp(sunset_glow, sunset_factor);

    let mut camera_pos = Vec3::ZERO;
    let mut camera_right = Vec3::X;
    let mut camera_up = Vec3::Y;
    if let Ok((cam_tf, mut camera, fog)) = cam_q.single_mut() {
        camera_pos = cam_tf.translation;
        camera_right = cam_tf.right().as_vec3();
        camera_up = cam_tf.up().as_vec3();
        camera.clear_color = ClearColorConfig::Custom(sky_color);
        if cycle.tint_fog {
            if let Some(mut fog) = fog {
                fog.color = fog_color;
                // Keep the Fog panel's glow strength (alpha) and tightness
                // (exponent); only swing the hue with the time of day.
                let strength = fog.directional_light_color.alpha();
                fog.directional_light_color =
                    Color::srgba(glow.x, glow.y, glow.z, strength);
            }
        }
    }

    // Visible sun disc: sits opposite the light-travel direction (so it's where
    // the light comes *from*), tracking the camera. It warms toward orange near
    // the horizon and fades out as it dips below it.
    if let Ok((mut disc_tf, material_handle)) = disc_q.single_mut() {
        let show = ((up_dot + 0.06) / 0.12).clamp(0.0, 1.0);
        if show <= 0.001 {
            disc_tf.scale = Vec3::ZERO;
        } else {
            disc_tf.translation = camera_pos - sun_dir * SUN_DISTANCE;
            disc_tf.scale = Vec3::splat(SUN_RADIUS);
            if let Some(material) = materials.get_mut(material_handle) {
                let day_sun = Vec3::new(1.0, 0.95, 0.8);
                let sunset_sun = Vec3::new(1.0, 0.45, 0.15);
                let c = day_sun.lerp(sunset_sun, sunset_factor);
                material.base_color = Color::srgb(c.x, c.y, c.z);
            }
        }
    }

    // Stars: only visible once the sun is well below the horizon, then each
    // fades in as it rises above its own local horizon. The whole field is one
    // mesh, so we rewrite its vertex positions (billboarding each quad toward
    // the camera) and per-vertex alpha (fade × twinkle) in a single pass.
    let global_visibility = ((-up_dot - 0.2) / 0.6).clamp(0.0, 1.0);

    // The star field rotates rigidly with the sun's orbit (offset by a quarter
    // turn so stars trail the night side rather than tracking the sun).
    let star_rotation = final_rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);

    if let Some(mesh) = meshes.get_mut(&star_field.mesh) {
        let now = time.elapsed_secs();
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(star_field.stars.len() * 4);
        let mut colors: Vec<[f32; 4]> = Vec::with_capacity(star_field.stars.len() * 4);

        for star in &star_field.stars {
            let dir = (star_rotation * -star.offset).normalize();
            let elevation = dir.dot(Vec3::Y);
            let horizon_fade = ((elevation + 0.05) / 0.35).clamp(0.0, 1.0);

            // Two out-of-phase sines give an irregular twinkle.
            let t = now * star.twinkle_speed;
            let noise = (t + star.phase).sin() * 0.7 + (t * 2.7 + star.phase * 1.5).sin() * 0.3;
            let twinkle = noise * 0.4 + 1.0;

            // Brightness flicker folded into the fade alpha; size also twinkles.
            let alpha = (global_visibility * horizon_fade * (twinkle * 0.3 + 0.7)).clamp(0.0, 1.0);
            let half = (4.0 + star.brightness * 2.0)
                * STAR_SCALE
                * star.brightness
                * (twinkle * 0.2 + 0.8);

            // Build a camera-facing quad around the star's point on the sphere.
            let center = camera_pos + dir * STAR_DISTANCE;
            let rx = camera_right * half;
            let uy = camera_up * half;
            positions.push((center - rx - uy).to_array());
            positions.push((center + rx - uy).to_array());
            positions.push((center + rx + uy).to_array());
            positions.push((center - rx + uy).to_array());

            let c = [1.0, 1.0, 1.0, alpha];
            colors.extend_from_slice(&[c, c, c, c]);
        }

        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
}
