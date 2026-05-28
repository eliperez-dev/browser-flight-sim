//! Day-night cycle: a directional "sun" that orbits the world, driving sky
//! colour, sun illuminance, ambient fill, fog tint and a field of twinkling
//! stars. Ported from the parent `bevy_sim` (`day_cycle.rs`) and adapted to this
//! project's pixel-render camera ([`TerrainCamera`]) and [`FogSettings`]-driven
//! fog.
//!
//! Add [`SkyPlugin`]. Tune it live from the F3 debug menu ("Sky / Day-Night"):
//! `time_of_day` scrubs the clock (0 = midnight, 0.5 = noon), `speed` advances
//! it automatically and `inclination` tilts the sun's orbit off the vertical.

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
/// Number of stars scattered over the celestial sphere.
const NUM_STARS: usize = 500;

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
}

impl Default for DayNightCycle {
    fn default() -> Self {
        Self {
            time_of_day: 0.35, // mid-morning so the world starts lit
            speed: 0.005,
            inclination: 0.3,
            tint_fog: true,
        }
    }
}

/// Marker for the directional light acting as the sun.
#[derive(Component)]
pub struct Sun;

/// One star on the celestial sphere. `offset` is its fixed direction on the
/// sphere; the rest randomise its twinkle so they don't pulse in unison.
#[derive(Component)]
pub struct Star {
    offset: Vec3,
    brightness: f32,
    phase: f32,
    twinkle_speed: f32,
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DayNightCycle>()
            .add_systems(Startup, (spawn_sun, spawn_stars))
            .add_systems(Update, update_daylight_cycle);
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

/// Spawns the single directional "sun". Its rotation (and therefore light
/// direction) is driven every frame by [`update_daylight_cycle`].
fn spawn_sun(mut commands: Commands) {
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
}

/// Scatters [`NUM_STARS`] emissive spheres uniformly over a sphere. Each gets a
/// shared-look material with fog disabled so the cycle can fade them in at dusk
/// without the fog eating them at distance.
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

    for _ in 0..NUM_STARS {
        let phi = rand_f32() * std::f32::consts::TAU;
        let theta = rand_f32() * std::f32::consts::PI;

        let x = theta.sin() * phi.cos();
        let y = theta.cos();
        let z = theta.sin() * phi.sin();

        let offset = Vec3::new(x, y, z).normalize();
        let brightness = 0.7 + rand_f32() * 0.5;
        let phase = rand_f32() * std::f32::consts::TAU;
        let twinkle_speed = 3.0 + rand_f32() * 4.0;

        let star_mesh = meshes.add(Sphere::new(4.0 + brightness * 2.0));
        let star_material = materials.add(StandardMaterial {
            base_color: Color::srgba(1.0, 1.0, 1.0, 0.0),
            emissive: LinearRgba::rgb(10.0, 10.0, 10.0),
            alpha_mode: AlphaMode::Blend,
            fog_enabled: false,
            ..default()
        });

        commands.spawn((
            Mesh3d(star_mesh),
            MeshMaterial3d(star_material),
            Transform::default(),
            Star { offset, brightness, phase, twinkle_speed },
            PIXEL_LAYER,
        ));
    }
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
        (With<Sun>, Without<Star>, Without<TerrainCamera>),
    >,
    mut cam_q: Query<
        (&Transform, &mut Camera, Option<&mut DistanceFog>),
        (With<TerrainCamera>, Without<Sun>, Without<Star>),
    >,
    mut star_q: Query<
        (&Star, &mut Transform, &MeshMaterial3d<StandardMaterial>),
        (Without<Sun>, Without<TerrainCamera>),
    >,
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

    let mut camera_pos = Vec3::ZERO;
    if let Ok((cam_tf, mut camera, fog)) = cam_q.single_mut() {
        camera_pos = cam_tf.translation;
        camera.clear_color = ClearColorConfig::Custom(sky_color);
        if cycle.tint_fog {
            if let Some(mut fog) = fog {
                fog.color = sky_color;
            }
        }
    }

    // Stars: only visible once the sun is well below the horizon, then each
    // fades in as it rises above its own local horizon.
    let global_visibility = ((-up_dot - 0.2) / 0.6).clamp(0.0, 1.0);
    if global_visibility <= 0.0 {
        for (_, mut star_tf, _) in &mut star_q {
            star_tf.scale = Vec3::ZERO;
        }
        return;
    }

    // The star field rotates rigidly with the sun's orbit (offset by a quarter
    // turn so stars trail the night side rather than tracking the sun).
    let star_rotation = final_rotation * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    for (star, mut star_tf, material_handle) in &mut star_q {
        let dir = (star_rotation * -star.offset).normalize();
        let elevation = dir.dot(Vec3::Y);
        let horizon_fade = ((elevation + 0.05) / 0.35).clamp(0.0, 1.0);
        let alpha = global_visibility * horizon_fade;

        if alpha <= 0.001 {
            star_tf.scale = Vec3::ZERO;
            continue;
        }

        star_tf.translation = camera_pos + dir * STAR_DISTANCE;

        // Two out-of-phase sines give an irregular twinkle.
        let t = time.elapsed_secs() * star.twinkle_speed;
        let noise = (t + star.phase).sin() * 0.7 + (t * 2.7 + star.phase * 1.5).sin() * 0.3;
        let twinkle = noise * 0.4 + 1.0;

        star_tf.scale = Vec3::splat(STAR_SCALE * star.brightness * (twinkle * 0.2 + 0.8));

        if let Some(material) = materials.get_mut(material_handle) {
            let glow = 10.0 * star.brightness * twinkle * alpha;
            material.base_color = Color::srgba(1.0, 1.0, 1.0, alpha);
            material.emissive = LinearRgba::rgb(glow, glow, glow);
        }
    }
}
