//! Procedural terrain: multi-octave Perlin height shaped by a temperature/
//! humidity biome field, coloured from per-biome palettes. Ported in full from
//! the parent `bevy_sim` world generator, cleaned up and reorganised so a single
//! `field()` evaluation feeds both the mesh (height + colour) and the landing-gear
//! ground query (height only) from identical math.
//!
//! [`WorldGenerator`] is a [`Resource`] shared between the async chunk-mesh tasks
//! and the gear physics, and `Clone` because each task moves its own copy onto
//! the pool.

use bevy::color::Mix;
use bevy::prelude::*;
use noise::{NoiseFn, Perlin};

/// Metres per chunk edge. 500 m balances draw-call count (fewer, bigger chunks)
/// against per-chunk mesh-gen cost — the dominant constraint on WASM, where the
/// "async" task pool actually runs on the main thread (see [`super::streaming`]).
pub const CHUNK_SIZE: f32 = 500.0;

/// Default vertical exaggeration applied to the (biome-shaped) noise. With the
/// biome multipliers, taiga becomes tall mountains while desert/grass stay
/// near-flat; this one knob scales the whole world's relief.
pub const DEFAULT_HEIGHT_SCALE: f32 = 200.0;

/// Default horizontal frequency multiplier on every layer. Higher = tighter,
/// more rugged terrain (and smaller biomes) for the same world distance. 3.0
/// packs features ~3× tighter than the parent sim's authored scale.
pub const DEFAULT_HORIZONTAL_SCALE: f32 = 3.0;

/// Editable world-generation settings, surfaced as debug sliders (F3 panel).
/// Mutating this resource triggers a debounced world rebuild — see
/// [`super::streaming::regenerate_terrain`]. `PartialEq` lets that system detect
/// real changes; `Clone` lets it snapshot the config to rebuild the generator.
#[derive(Resource, Clone, PartialEq)]
pub struct WorldGenConfig {
    pub seed: u32,
    /// Horizontal feature frequency; see [`DEFAULT_HORIZONTAL_SCALE`].
    pub horizontal_scale: f32,
    /// Vertical relief; see [`DEFAULT_HEIGHT_SCALE`].
    pub height_scale: f32,
    /// View radius in chunks (also drives despawn distance).
    pub render_distance: i32,
    /// Chunk-builds started per frame; keep low on WASM (single-threaded tasks).
    pub max_chunks_per_frame: usize,
}

impl Default for WorldGenConfig {
    fn default() -> Self {
        Self {
            seed: 3,
            horizontal_scale: DEFAULT_HORIZONTAL_SCALE,
            height_scale: DEFAULT_HEIGHT_SCALE,
            render_distance: 12,
            max_chunks_per_frame: 3,
        }
    }
}

// --- Ocean shaping thresholds (in normalised 0..1 climate space) -------------
const OCEAN_HUMIDITY_THRESHOLD: f32 = 0.60;
const OCEAN_HOT_TEMP_THRESHOLD: f32 = 0.95;
const OCEAN_COLD_TEMP_THRESHOLD: f32 = 0.0;
const OCEAN_TRANSITION_WIDTH: f32 = 0.30;

/// Multi-octave Perlin terrain shaped by a coarse climate field. Built from a
/// [`WorldGenConfig`] snapshot; the horizontal scale is baked into the layers
/// here, so changing it means rebuilding the generator (which the regen system
/// does on config change).
#[derive(Resource, Clone)]
pub struct WorldGenerator {
    height_scale: f32,
    terrain_layers: Vec<PerlinLayer>,
    temperature_layer: PerlinLayer,
    humidity_layer: PerlinLayer,
}

impl WorldGenerator {
    pub fn from_config(cfg: &WorldGenConfig) -> Self {
        let seed = cfg.seed;
        let hs = cfg.horizontal_scale;
        Self {
            height_scale: cfg.height_scale,
            // (horizontal_scale, vertical_amplitude). Low scale = broad features.
            terrain_layers: vec![
                PerlinLayer::new(seed,       0.08 * hs, 4.5),
                PerlinLayer::new(seed,       0.20 * hs, 3.5),
                PerlinLayer::new(seed + 100, 0.50 * hs, 1.75),
                PerlinLayer::new(seed + 200, 1.00 * hs, 0.50),
                PerlinLayer::new(seed + 300, 2.00 * hs, 0.40),
            ],
            // Temperature/humidity must vary slowly across the map — keep scales low.
            temperature_layer: PerlinLayer::new(seed + 400, 0.06 * hs, 1.0),
            humidity_layer: PerlinLayer::new(seed + 500, 0.06 * hs, 1.0),
        }
    }

    /// Normalised climate at (x, z): `(temperature, humidity)`, each clamped 0..1.
    pub fn get_climate(&self, x: f32, z: f32) -> (f32, f32) {
        let raw_temp = self.temperature_layer.get(x, z);
        let raw_hum = self.humidity_layer.get(x, z);
        let temp = (((raw_temp / self.temperature_layer.vertical_scale) + 1.0) * 0.5).clamp(0.0, 1.0);
        let hum = (((raw_hum / self.humidity_layer.vertical_scale) + 1.0) * 0.5).clamp(0.0, 1.0);
        (temp, hum)
    }

    /// Biome at (x, z), from the climate field. Kept for upcoming features
    /// (vegetation, water, audio) that key off biome rather than raw height.
    #[allow(dead_code)]
    pub fn get_biome(&self, x: f32, z: f32) -> Biome {
        let (temp, hum) = self.get_climate(x, z);
        if hum > OCEAN_HUMIDITY_THRESHOLD + 0.1
            || temp > OCEAN_HOT_TEMP_THRESHOLD
            || temp < OCEAN_COLD_TEMP_THRESHOLD
        {
            return Biome::Ocean;
        }
        match (temp > 0.5, hum > 0.45) {
            (true, true) => Biome::Forest,    // hot & wet
            (true, false) => Biome::Desert,   // hot & dry
            (false, false) => Biome::Grasslands,
            (false, true) => Biome::Taiga,
        }
    }

    /// World-space terrain height (metres) at (x, z). Used by the landing gear
    /// to find the ground under each strut, so it must match the mesh exactly —
    /// both go through [`Self::field`].
    pub fn get_terrain_height(&self, x: f32, z: f32) -> f32 {
        self.field(x, z).0 * self.height_scale
    }

    /// Everything the mesh needs at one point: world-space height (metres) and
    /// the linear-RGBA surface colour. One evaluation, no duplicated noise work.
    pub fn surface(&self, x: f32, z: f32) -> (f32, [f32; 4]) {
        let (raw, temp, hum) = self.field(x, z);
        let height = raw * self.height_scale;
        (height, terrain_color(raw, temp, hum))
    }

    /// Core field evaluation: the raw (pre-`height_scale`) height plus the
    /// climate that shaped it. Raw height is what the colour palettes are keyed
    /// to. Flattened toward 0 near the origin so the runway airfield stays level.
    fn field(&self, x: f32, z: f32) -> (f32, f32, f32) {
        let (temp, hum) = self.get_climate(x, z);

        let mut base = 0.0;
        for layer in &self.terrain_layers {
            base += layer.get(x, z);
        }

        let raw = (base * biome_height_multiplier(temp, hum) + biome_elevation_offset(temp, hum))
            * origin_flatten(x, z);
        (raw, temp, hum)
    }
}

/// The four climate biomes plus open water.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum Biome {
    Desert,
    Grasslands,
    Taiga,
    Forest,
    Ocean,
}

#[derive(Clone)]
struct PerlinLayer {
    perlin: Perlin,
    horizontal_scale: f32,
    vertical_scale: f32,
    /// Per-layer domain offset so octaves seeded alike don't line their zero
    /// crossings into visible grid artifacts.
    offset: f64,
}

impl PerlinLayer {
    fn new(seed: u32, horizontal_scale: f32, vertical_scale: f32) -> Self {
        Self {
            perlin: Perlin::new(seed),
            horizontal_scale,
            vertical_scale,
            offset: (seed as f64 * 1337.42) % 100_000.0,
        }
    }

    fn get(&self, x: f32, z: f32) -> f32 {
        let n = self.perlin.get([
            (x * self.horizontal_scale / 1000.0) as f64 + self.offset,
            (z * self.horizontal_scale / 1000.0) as f64 + (self.offset.sqrt() + 202_994.0),
        ]) as f32;
        n * self.vertical_scale
    }
}

// --- Origin airfield flattening ----------------------------------------------

/// Fully flat (runway) out to this radius in metres. Must clear the 2000 m
/// runway's half-length so the whole strip stays level.
const FLAT_RADIUS: f32 = 1100.0;
/// Beyond this radius terrain is at full height; between the two it ramps up.
const BLEND_RADIUS: f32 = 2600.0;

/// 0.0 at the origin airfield, 1.0 past [`BLEND_RADIUS`], smoothstep between.
fn origin_flatten(x: f32, z: f32) -> f32 {
    let d = (x * x + z * z).sqrt();
    let t = ((d - FLAT_RADIUS) / (BLEND_RADIUS - FLAT_RADIUS)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) // smoothstep
}

// --- Biome shaping ------------------------------------------------------------

/// How strongly biome blends toward ocean (0 = land, 1 = full ocean) given the
/// climate — wet, or temperature past either extreme. Shared by the height
/// multiplier and elevation offset so they agree on where coastlines fall.
fn ocean_factor(temp: f32, humidity: f32) -> f32 {
    let wet = if humidity > OCEAN_HUMIDITY_THRESHOLD {
        ((humidity - OCEAN_HUMIDITY_THRESHOLD) / OCEAN_TRANSITION_WIDTH).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let hot = if temp > OCEAN_HOT_TEMP_THRESHOLD - OCEAN_TRANSITION_WIDTH {
        ((temp - (OCEAN_HOT_TEMP_THRESHOLD - OCEAN_TRANSITION_WIDTH)) / OCEAN_TRANSITION_WIDTH)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cold = if temp < OCEAN_COLD_TEMP_THRESHOLD + OCEAN_TRANSITION_WIDTH {
        ((OCEAN_COLD_TEMP_THRESHOLD + OCEAN_TRANSITION_WIDTH - temp) / OCEAN_TRANSITION_WIDTH)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };
    wet.max(hot).max(cold)
}

/// Vertical offset added to the noise sum, per biome, blended bilinearly across
/// the climate square and then toward a deep ocean basin.
fn biome_elevation_offset(temp: f32, humidity: f32) -> f32 {
    const DESERT: f32 = 0.0;
    const GRASS: f32 = 0.04;
    const FOREST: f32 = 0.5;
    const TAIGA: f32 = 8.0;
    const OCEAN: f32 = -2.5;

    let cold_blend = GRASS + (TAIGA - GRASS) * humidity;
    let hot_blend = DESERT + (FOREST - DESERT) * humidity;
    let land = cold_blend + (hot_blend - cold_blend) * temp;

    land + (OCEAN - land) * ocean_factor(temp, humidity)
}

/// Amplitude applied to the noise sum, per biome — taiga is mountainous, desert
/// and grass nearly flat, oceans flat. Bilinear across climate, then toward ocean.
fn biome_height_multiplier(temp: f32, humidity: f32) -> f32 {
    const DESERT: f32 = 0.01;
    const GRASS: f32 = 0.02;
    const FOREST: f32 = 0.05;
    const TAIGA: f32 = 1.5;
    const OCEAN: f32 = 0.01;

    let cold_blend = GRASS + (TAIGA - GRASS) * humidity;
    let hot_blend = DESERT + (FOREST - DESERT) * humidity;
    let land = cold_blend + (hot_blend - cold_blend) * temp;

    land + (OCEAN - land) * ocean_factor(temp, humidity)
}

// --- Colour -------------------------------------------------------------------

/// A height threshold and the colour terrain takes there. Heights are in raw
/// (pre-`height_scale`) units; stops ascend.
struct TerrainStop {
    height: f32,
    color: Color,
}

const GRASSLANDS_LEVELS: &[TerrainStop] = &[
    TerrainStop { height: -1.0, color: Color::srgb(0.3, 0.2, 0.1) },
    TerrainStop { height: -0.5, color: Color::srgb(0.8, 0.7, 0.5) },
    TerrainStop { height: 0.2,  color: Color::srgb(0.2, 0.5, 0.2) },
    TerrainStop { height: 2.5,  color: Color::srgb(0.5, 0.5, 0.5) },
];

const DESERT_LEVELS: &[TerrainStop] = &[
    TerrainStop { height: -1.0, color: Color::srgb(0.6, 0.4, 0.2) },
    TerrainStop { height: -0.5, color: Color::srgb(0.9, 0.8, 0.5) },
    TerrainStop { height: 0.7,  color: Color::srgb(0.8, 0.6, 0.3) },
    TerrainStop { height: 1.5,  color: Color::srgb(0.7, 0.4, 0.2) },
    TerrainStop { height: 2.5,  color: Color::srgb(0.6, 0.3, 0.1) },
];

const TAIGA_LEVELS: &[TerrainStop] = &[
    TerrainStop { height: -1.0, color: Color::srgb(0.2, 0.2, 0.2) },
    TerrainStop { height: -0.5, color: Color::srgb(0.4, 0.4, 0.4) },
    TerrainStop { height: 0.3,  color: Color::srgb(0.1, 0.3, 0.2) },
    TerrainStop { height: 0.8,  color: Color::srgb(0.5, 0.5, 0.5) },
    TerrainStop { height: 1.0,  color: Color::WHITE },
];

const FOREST_LEVELS: &[TerrainStop] = &[
    TerrainStop { height: -1.0, color: Color::srgb(0.3, 0.2, 0.1) },
    TerrainStop { height: -0.5, color: Color::srgb(0.2, 0.4, 0.1) },
    TerrainStop { height: 0.3,  color: Color::srgb(0.1, 0.8, 0.1) },
    TerrainStop { height: 2.7,  color: Color::srgb(0.4, 0.4, 0.4) },
    TerrainStop { height: 3.0,  color: Color::WHITE },
];

/// Linear colour for `height` within one biome palette, interpolated between the
/// two bracketing stops. (Per-face flat shading provides the crisp facets; the
/// palette itself stays continuous so neighbouring facets transition naturally.)
fn palette_color(height: f32, palette: &[TerrainStop]) -> LinearRgba {
    if height <= palette[0].height {
        return palette[0].color.to_linear();
    }
    let last = palette.len() - 1;
    if height >= palette[last].height {
        return palette[last].color.to_linear();
    }
    for i in 1..palette.len() {
        if height <= palette[i].height {
            let lower = &palette[i - 1];
            let upper = &palette[i];
            let t = (height - lower.height) / (upper.height - lower.height);
            return lower.color.to_linear().mix(&upper.color.to_linear(), t);
        }
    }
    palette[last].color.to_linear()
}

/// Surface colour at raw `height` for the given climate: each biome palette is
/// sampled, then blended bilinearly across the temperature/humidity square.
/// Returned as linear RGBA for `Mesh::ATTRIBUTE_COLOR`.
fn terrain_color(height: f32, temp: f32, humidity: f32) -> [f32; 4] {
    let forest = palette_color(height, FOREST_LEVELS);
    let desert = palette_color(height, DESERT_LEVELS);
    let taiga = palette_color(height, TAIGA_LEVELS);
    let grass = palette_color(height, GRASSLANDS_LEVELS);

    let cold = grass.mix(&taiga, humidity); // dry→wet at cold
    let hot = desert.mix(&forest, humidity); // dry→wet at hot
    cold.mix(&hot, temp).to_f32_array() // cold→hot
}
