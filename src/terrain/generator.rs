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
pub const DEFAULT_HEIGHT_SCALE: f32 = 220.0;

/// Default horizontal frequency multiplier on every layer. Higher = tighter,
/// more rugged terrain (and smaller biomes) for the same world distance. 3.0
/// packs features ~3× tighter than the parent sim's authored scale.
pub const DEFAULT_HORIZONTAL_SCALE: f32 = 2.5;

/// Default humidity (0..1) past which climate blends toward open ocean. Lower =
/// more of the map is sea. See [`WorldGenerator::ocean_factor`].
pub const DEFAULT_OCEAN_HUMIDITY_THRESHOLD: f32 = 0.60;

/// Default width (in climate units) of the land→ocean blend. Wider = gentler,
/// broader coastlines; narrower = abrupt shorelines.
pub const DEFAULT_OCEAN_TRANSITION_WIDTH: f32 = 0.30;

/// Default depth (raw units, pre-`height_scale`) the ocean basin sinks below the
/// land baseline. Deeper basins sit further under the water plane's sea level.
pub const DEFAULT_OCEAN_DEPTH: f32 = 2.5;

/// Default biome-size multiplier. 1.0 keeps the original climate frequency;
/// larger = broader biomes, smaller = patchier. See [`WorldGenerator::from_config`].
pub const DEFAULT_BIOME_SIZE: f32 = 1.0;

/// Per-biome terrain shaping: `elevation` is the base offset (raw units, pre-
/// `height_scale`) the biome sits at, `relief` the amplitude multiplier on the
/// noise sum (taiga is mountainous, desert near-flat), and `abundance` a weight
/// on how strongly this biome dominates the blend (1.0 = neutral; higher spreads
/// its character over more of the map). The four land biomes sit at the corners
/// of the temperature/humidity climate square and are blended bilinearly — see
/// [`WorldGenerator::biome_weights`].
#[derive(Clone, Copy, PartialEq)]
pub struct BiomeShape {
    pub elevation: f32,
    pub relief: f32,
    pub abundance: f32,
}

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
    /// LOD bands as `(max_distance_in_chunks, subdivisions)`, ascending distance.
    /// A chunk uses the subdivisions of the first band it falls within; nearest
    /// is capped low so no single chunk's noise pass hitches the WASM main thread.
    /// Mirrored live into `ChunkManager` (no world rebuild — chunks just re-mesh).
    pub lod_levels: [(f32, u32); 5],
    /// Humidity past which climate turns to ocean; see
    /// [`DEFAULT_OCEAN_HUMIDITY_THRESHOLD`].
    pub ocean_humidity_threshold: f32,
    /// Land→ocean blend width; see [`DEFAULT_OCEAN_TRANSITION_WIDTH`].
    pub ocean_transition_width: f32,
    /// Ocean basin depth in raw units; see [`DEFAULT_OCEAN_DEPTH`].
    pub ocean_depth: f32,
    /// Biome size multiplier (scales the climate-field wavelength); see
    /// [`DEFAULT_BIOME_SIZE`].
    pub biome_size: f32,
    /// Per-biome shaping at the four corners of the climate square.
    pub desert: BiomeShape,
    pub grasslands: BiomeShape,
    pub forest: BiomeShape,
    pub taiga: BiomeShape,
    /// Climate-axis remap controlling biome distribution. `bias` shifts the whole
    /// map along the axis (temperature: colder↔hotter; humidity: drier↔wetter),
    /// `contrast` scales spread around the 0.5 midpoint (>1 = sharper, more
    /// distinct biomes; <1 = everything blends toward the middle). Defaults
    /// (bias 0, contrast 1) leave the raw Perlin climate untouched. Humidity also
    /// gates ocean coverage, so a wetter bias adds sea.
    pub temp_bias: f32,
    pub temp_contrast: f32,
    pub humidity_bias: f32,
    pub humidity_contrast: f32,
}

impl Default for WorldGenConfig {
    fn default() -> Self {
        Self {
            seed: 3,
            horizontal_scale: DEFAULT_HORIZONTAL_SCALE,
            height_scale: DEFAULT_HEIGHT_SCALE,
            render_distance: 35,
            max_chunks_per_frame: 4,
            lod_levels: [
                (2.0, 15),
                (6.0, 9),
                (13.0, 6),
                (15.0, 4),
                (25.0, 2),
            ],
            ocean_humidity_threshold: DEFAULT_OCEAN_HUMIDITY_THRESHOLD,
            ocean_transition_width: DEFAULT_OCEAN_TRANSITION_WIDTH,
            ocean_depth: DEFAULT_OCEAN_DEPTH,
            biome_size: DEFAULT_BIOME_SIZE,
            // Corners of the climate square (matching the original constants):
            //   dry→wet at cold = grasslands→taiga, dry→wet at hot = desert→forest.
            desert:     BiomeShape { elevation: 0.3,  relief: 0.005, abundance: 1.0 },
            grasslands: BiomeShape { elevation: 0.04, relief: 0.02,  abundance: 1.0 },
            forest:     BiomeShape { elevation: 0.5,  relief: 0.05,  abundance: 1.0 },
            taiga:      BiomeShape { elevation: 8.0,  relief: 1.5,   abundance: 1.0 },
            temp_bias: 0.0,
            temp_contrast: 1.0,
            humidity_bias: 0.0,
            humidity_contrast: 1.0,
        }
    }
}

// --- Ocean shaping thresholds (in normalised 0..1 climate space) -------------
// Humidity threshold and transition width are now per-world config (see
// `WorldGenConfig`); the temperature extremes stay fixed constants.
const OCEAN_HOT_TEMP_THRESHOLD: f32 = 0.95;
const OCEAN_COLD_TEMP_THRESHOLD: f32 = 0.0;

/// Multi-octave Perlin terrain shaped by a coarse climate field. Built from a
/// [`WorldGenConfig`] snapshot; the horizontal scale is baked into the layers
/// here, so changing it means rebuilding the generator (which the regen system
/// does on config change).
#[derive(Resource, Clone)]
pub struct WorldGenerator {
    seed: u32,
    height_scale: f32,
    ocean_humidity_threshold: f32,
    ocean_transition_width: f32,
    ocean_depth: f32,
    desert: BiomeShape,
    grasslands: BiomeShape,
    forest: BiomeShape,
    taiga: BiomeShape,
    temp_bias: f32,
    temp_contrast: f32,
    humidity_bias: f32,
    humidity_contrast: f32,
    terrain_layers: Vec<PerlinLayer>,
    temperature_layer: PerlinLayer,
    humidity_layer: PerlinLayer,
}

impl WorldGenerator {
    pub fn from_config(cfg: &WorldGenConfig) -> Self {
        let seed = cfg.seed;
        let hs = cfg.horizontal_scale;
        // Bigger biome_size = lower climate frequency = broader biomes. Clamped
        // away from zero so the wavelength can't blow up to a single flat biome.
        let climate_freq = 0.06 * hs / cfg.biome_size.max(0.05);
        Self {
            seed,
            height_scale: cfg.height_scale,
            ocean_humidity_threshold: cfg.ocean_humidity_threshold,
            ocean_transition_width: cfg.ocean_transition_width,
            ocean_depth: cfg.ocean_depth,
            desert: cfg.desert,
            grasslands: cfg.grasslands,
            forest: cfg.forest,
            taiga: cfg.taiga,
            temp_bias: cfg.temp_bias,
            temp_contrast: cfg.temp_contrast,
            humidity_bias: cfg.humidity_bias,
            humidity_contrast: cfg.humidity_contrast,
            // (horizontal_scale, vertical_amplitude). Low scale = broad features.
            terrain_layers: vec![
                PerlinLayer::new(seed,       0.08 * hs, 4.5),
                PerlinLayer::new(seed,       0.20 * hs, 3.5),
                PerlinLayer::new(seed + 100, 0.50 * hs, 1.75),
                PerlinLayer::new(seed + 200, 1.00 * hs, 0.50),
                PerlinLayer::new(seed + 300, 2.00 * hs, 0.40),
            ],
            // Temperature/humidity must vary slowly across the map — keep scales low.
            temperature_layer: PerlinLayer::new(seed + 400, climate_freq, 1.0),
            humidity_layer: PerlinLayer::new(seed + 500, climate_freq, 1.0),
        }
    }

    /// World seed, used to derive the per-cell runway layout deterministically.
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Normalised climate at (x, z): `(temperature, humidity)`, each remapped by
    /// the axis bias/contrast and clamped 0..1. The remap is the single chokepoint
    /// for biome distribution, so it flows into elevation, relief, colour, and
    /// ocean coverage alike.
    pub fn get_climate(&self, x: f32, z: f32) -> (f32, f32) {
        let raw_temp = self.temperature_layer.get(x, z);
        let raw_hum = self.humidity_layer.get(x, z);
        let temp = ((raw_temp / self.temperature_layer.vertical_scale) + 1.0) * 0.5;
        let hum = ((raw_hum / self.humidity_layer.vertical_scale) + 1.0) * 0.5;
        (
            remap_axis(temp, self.temp_bias, self.temp_contrast),
            remap_axis(hum, self.humidity_bias, self.humidity_contrast),
        )
    }

    /// Biome at (x, z), from the climate field. Kept for upcoming features
    /// (vegetation, water, audio) that key off biome rather than raw height.
    #[allow(dead_code)]
    pub fn get_biome(&self, x: f32, z: f32) -> Biome {
        let (temp, hum) = self.get_climate(x, z);
        if hum > self.ocean_humidity_threshold + 0.1
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
        super::runway::ground_height(self, x, z, self.natural_height(x, z))
    }

    /// Natural terrain height (metres) ignoring runway flattening. Used to seed
    /// each runway's elevation; `get_terrain_height` flattens on top of it.
    pub fn natural_height(&self, x: f32, z: f32) -> f32 {
        self.natural_raw(x, z).0 * self.height_scale
    }

    /// Natural (un-flattened) height + colour at one point, for the chunk-mesh
    /// kernel. The mesh applies runway flattening separately, against runways it
    /// resolves once per chunk (see `runway::runways_in_region`), so this stays
    /// off the per-vertex runway path.
    pub fn sample_natural(&self, x: f32, z: f32) -> (f32, [f32; 4]) {
        let (raw, temp, hum) = self.natural_raw(x, z);
        (raw * self.height_scale, self.terrain_color(raw, temp, hum))
    }

    /// Everything the mesh needs at one point: world-space height (metres) and
    /// the linear-RGBA surface colour. One evaluation, no duplicated noise work.
    /// Core noise evaluation: the raw (pre-`height_scale`) natural height plus
    /// the climate that shaped it. Raw height is what the colour palettes are
    /// keyed to. No runway flattening — that's applied in world-height space.
    fn natural_raw(&self, x: f32, z: f32) -> (f32, f32, f32) {
        let (temp, hum) = self.get_climate(x, z);

        let mut base = 0.0;
        for layer in &self.terrain_layers {
            base += layer.get(x, z);
        }

        let raw =
            base * self.biome_height_multiplier(temp, hum) + self.biome_elevation_offset(temp, hum);
        (raw, temp, hum)
    }

    /// How strongly biome blends toward ocean (0 = land, 1 = full ocean) given the
    /// climate — wet, or temperature past either extreme. Shared by the height
    /// multiplier and elevation offset so they agree on where coastlines fall.
    /// Uses the per-world humidity threshold and transition width.
    fn ocean_factor(&self, temp: f32, humidity: f32) -> f32 {
        let width = self.ocean_transition_width.max(1e-3);
        let wet = if humidity > self.ocean_humidity_threshold {
            ((humidity - self.ocean_humidity_threshold) / width).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let hot = if temp > OCEAN_HOT_TEMP_THRESHOLD - width {
            ((temp - (OCEAN_HOT_TEMP_THRESHOLD - width)) / width).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let cold = if temp < OCEAN_COLD_TEMP_THRESHOLD + width {
            ((OCEAN_COLD_TEMP_THRESHOLD + width - temp) / width).clamp(0.0, 1.0)
        } else {
            0.0
        };
        wet.max(hot).max(cold)
    }

    /// Normalised per-biome blend weights at the given (already-remapped) climate,
    /// in the order `[grasslands, taiga, desert, forest]`. These are the bilinear
    /// corner weights scaled by each biome's `abundance` and renormalised, so all
    /// abundances at 1.0 reproduce the plain bilinear blend exactly. Shared by
    /// elevation, relief, and colour so they always agree on the mix.
    fn biome_weights(&self, temp: f32, humidity: f32) -> [f32; 4] {
        let grass = (1.0 - temp) * (1.0 - humidity) * self.grasslands.abundance;
        let taiga = (1.0 - temp) * humidity * self.taiga.abundance;
        let desert = temp * (1.0 - humidity) * self.desert.abundance;
        let forest = temp * humidity * self.forest.abundance;
        let sum = grass + taiga + desert + forest;
        if sum <= 1e-6 {
            // Degenerate (all abundances ~0) — fall back to an even split.
            return [0.25; 4];
        }
        [grass / sum, taiga / sum, desert / sum, forest / sum]
    }

    /// Vertical offset added to the noise sum: the abundance-weighted blend of the
    /// four biome base elevations, then pulled toward a deep ocean basin
    /// (`ocean_depth`).
    fn biome_elevation_offset(&self, temp: f32, humidity: f32) -> f32 {
        let w = self.biome_weights(temp, humidity);
        let land = w[0] * self.grasslands.elevation
            + w[1] * self.taiga.elevation
            + w[2] * self.desert.elevation
            + w[3] * self.forest.elevation;
        let ocean = -self.ocean_depth;
        land + (ocean - land) * self.ocean_factor(temp, humidity)
    }

    /// Amplitude applied to the noise sum — the abundance-weighted blend of the
    /// four biome reliefs (taiga mountainous, desert near-flat), then flattened
    /// toward ocean. Open water stays flat regardless of the surrounding land.
    fn biome_height_multiplier(&self, temp: f32, humidity: f32) -> f32 {
        const OCEAN: f32 = 0.01;
        let w = self.biome_weights(temp, humidity);
        let land = w[0] * self.grasslands.relief
            + w[1] * self.taiga.relief
            + w[2] * self.desert.relief
            + w[3] * self.forest.relief;
        land + (OCEAN - land) * self.ocean_factor(temp, humidity)
    }

    /// Surface colour at raw `height` for the given climate: each biome palette is
    /// sampled at this height, then combined with the same abundance-weighted blend
    /// as elevation/relief. Returned as linear RGBA for `Mesh::ATTRIBUTE_COLOR`.
    fn terrain_color(&self, height: f32, temp: f32, humidity: f32) -> [f32; 4] {
        let grass = palette_color(height, GRASSLANDS_LEVELS);
        let taiga = palette_color(height, TAIGA_LEVELS);
        let desert = palette_color(height, DESERT_LEVELS);
        let forest = palette_color(height, FOREST_LEVELS);
        let w = self.biome_weights(temp, humidity);
        let mix = |a: f32, b: f32, c: f32, d: f32| w[0] * a + w[1] * b + w[2] * c + w[3] * d;
        [
            mix(grass.red, taiga.red, desert.red, forest.red),
            mix(grass.green, taiga.green, desert.green, forest.green),
            mix(grass.blue, taiga.blue, desert.blue, forest.blue),
            mix(grass.alpha, taiga.alpha, desert.alpha, forest.alpha),
        ]
    }
}

/// Remaps a normalised 0..1 climate value: scale spread around the 0.5 midpoint
/// by `contrast`, shift by `bias`, then clamp back to 0..1. Identity at
/// (bias 0, contrast 1).
fn remap_axis(v: f32, bias: f32, contrast: f32) -> f32 {
    (0.5 + (v - 0.5) * contrast + bias).clamp(0.0, 1.0)
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

