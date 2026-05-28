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

/// Depth (metres) each chunk's perimeter skirt hangs below the terrain edge. The
/// skirt is a vertical wall ringing the tile that seals the hairline seam between
/// neighbours — T-junctions at LOD borders (finer edge follows the terrain, coarser
/// edge is a flat chord) and the sub-pixel rasterization gap with MSAA off — by
/// backing the gap with terrain-coloured geometry from below. Must exceed the
/// largest edge-height discontinuity; it's hidden inside the neighbouring hillside
/// (or below the water plane over ocean), so a generous value is safe.
pub const SKIRT_DEPTH: f32 = 50.0;

/// Default vertical exaggeration applied to the (biome-shaped) noise. With the
/// biome multipliers, taiga becomes tall mountains while desert/grass stay
/// near-flat; this one knob scales the whole world's relief.
pub const DEFAULT_HEIGHT_SCALE: f32 = 220.0;

/// Default horizontal frequency multiplier on every layer. Higher = tighter,
/// more rugged terrain (and smaller biomes) for the same world distance. 3.0
/// packs features ~3× tighter than the parent sim's authored scale.
pub const DEFAULT_HORIZONTAL_SCALE: f32 = 5.0;

/// Default sea level on the 0..1 *continentalness* scale: terrain below this
/// (minus the transition) is open ocean. Higher = more of the map is sea. See
/// [`WorldGenerator::ocean_factor`].
pub const DEFAULT_SEA_LEVEL_THRESHOLD: f32 = 0.45;

/// Default width (in continentalness units) of the land→ocean blend. Wider =
/// gentler, broader coastlines; narrower = abrupt shorelines.
pub const DEFAULT_OCEAN_TRANSITION_WIDTH: f32 = 0.30;

/// Default depth (raw units, pre-`height_scale`) the ocean basin sinks below the
/// land baseline. Deeper basins sit further under the water plane's sea level.
pub const DEFAULT_OCEAN_DEPTH: f32 = 2.5;

/// Default continent-size multiplier (scales the continentalness-field
/// wavelength). Larger = bigger landmasses and oceans. See
/// [`WorldGenerator::from_config`].
pub const DEFAULT_CONTINENT_SIZE: f32 = 1.0;

/// Default coastal-humidity strength: how much extra humidity is added near the
/// coast (decaying inland), giving wet coasts and dry interiors. 0 disables it.
pub const DEFAULT_COASTAL_HUMIDITY: f32 = 0.3;

/// Default biome-size multiplier. 1.0 keeps the original climate frequency;
/// larger = broader biomes, smaller = patchier. See [`WorldGenerator::from_config`].
pub const DEFAULT_BIOME_SIZE: f32 = 1.0;

/// Default lapse rate: normalised temperature drop per 1000 m of altitude above
/// sea level. Couples temperature to terrain height so peaks trend cold/snowy —
/// roughly the real ~6.5 °C/km mapped onto the 0..1 climate scale. See
/// [`WorldGenerator::natural_raw`].
pub const DEFAULT_TEMP_LAPSE: f32 = 0.25;

/// Default latitude banding strength: peak ± temperature swing between the warm
/// "equator" lines and the cold bands between them.
pub const DEFAULT_LATITUDE_STRENGTH: f32 = 0.3;

/// Default latitude band wavelength (m): world-Z distance from one warm equator
/// line to the next. The pattern is periodic so an endless world keeps cycling
/// through climate zones.
pub const DEFAULT_LATITUDE_BAND: f32 = 60_000.0;

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
    pub lod_levels: [(f32, u32); 4],
    /// Sea level on the continentalness scale; see [`DEFAULT_SEA_LEVEL_THRESHOLD`].
    pub sea_level_threshold: f32,
    /// Land→ocean blend width; see [`DEFAULT_OCEAN_TRANSITION_WIDTH`].
    pub ocean_transition_width: f32,
    /// Ocean basin depth in raw units; see [`DEFAULT_OCEAN_DEPTH`].
    pub ocean_depth: f32,
    /// Continent size multiplier; see [`DEFAULT_CONTINENT_SIZE`].
    pub continent_size: f32,
    /// Coastal-humidity strength; see [`DEFAULT_COASTAL_HUMIDITY`].
    pub coastal_humidity: f32,
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
    /// Altitude→temperature coupling (lapse rate); see [`DEFAULT_TEMP_LAPSE`].
    /// 0 disables it (climate is then height-independent, the old behaviour).
    pub temp_lapse: f32,
    /// Latitude banding strength; see [`DEFAULT_LATITUDE_STRENGTH`]. 0 disables.
    pub latitude_strength: f32,
    /// Latitude band wavelength (m); see [`DEFAULT_LATITUDE_BAND`].
    pub latitude_band: f32,
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
                (2.0, 16),
                (6.0, 10),
                (20.0, 4),
                (30.0, 2),
            ],
            sea_level_threshold: DEFAULT_SEA_LEVEL_THRESHOLD,
            ocean_transition_width: DEFAULT_OCEAN_TRANSITION_WIDTH,
            ocean_depth: DEFAULT_OCEAN_DEPTH,
            continent_size: DEFAULT_CONTINENT_SIZE,
            coastal_humidity: DEFAULT_COASTAL_HUMIDITY,
            biome_size: DEFAULT_BIOME_SIZE,
            // Corners of the climate square (matching the original constants):
            //   dry→wet at cold = grasslands→taiga, dry→wet at hot = desert→forest.
            desert:     BiomeShape { elevation: 0.3,  relief: 0.005, abundance: 0.3 },
            grasslands: BiomeShape { elevation: 0.04, relief: 0.02,  abundance: 1.0 },
            forest:     BiomeShape { elevation: 0.5,  relief: 0.05,  abundance: 1.0 },
            taiga:      BiomeShape { elevation: 6.5,  relief: 0.5,   abundance: 0.5 },
            temp_bias: -0.15,
            humidity_bias: -0.15,
            humidity_contrast: 1.0,
            temp_contrast: 1.0,
            temp_lapse: DEFAULT_TEMP_LAPSE,
            latitude_strength: DEFAULT_LATITUDE_STRENGTH,
            latitude_band: DEFAULT_LATITUDE_BAND,
        }
    }
}

/// Relief multiplier applied to open ocean — flat regardless of the surrounding
/// land's ruggedness.
const OCEAN_RELIEF: f32 = 0.01;

/// Multi-octave Perlin terrain shaped by a coarse climate field. Built from a
/// [`WorldGenConfig`] snapshot; the horizontal scale is baked into the layers
/// here, so changing it means rebuilding the generator (which the regen system
/// does on config change).
#[derive(Resource, Clone)]
pub struct WorldGenerator {
    seed: u32,
    height_scale: f32,
    sea_level_threshold: f32,
    ocean_transition_width: f32,
    ocean_depth: f32,
    coastal_humidity: f32,
    desert: BiomeShape,
    grasslands: BiomeShape,
    forest: BiomeShape,
    taiga: BiomeShape,
    temp_bias: f32,
    temp_contrast: f32,
    humidity_bias: f32,
    humidity_contrast: f32,
    temp_lapse: f32,
    latitude_strength: f32,
    latitude_band: f32,
    terrain_layers: Vec<PerlinLayer>,
    temperature_layer: PerlinLayer,
    humidity_layer: PerlinLayer,
    /// Low-frequency land/sea mask: high = continental interior, low = ocean.
    /// Independent of the climate field — geography decides where water is.
    continent_layer: PerlinLayer,
}

impl WorldGenerator {
    pub fn from_config(cfg: &WorldGenConfig) -> Self {
        let seed = cfg.seed;
        let hs = cfg.horizontal_scale;
        // Bigger biome_size = lower climate frequency = broader biomes. Clamped
        // away from zero so the wavelength can't blow up to a single flat biome.
        let climate_freq = 0.06 * hs / cfg.biome_size.max(0.05);
        // Continents are a large-scale geographic feature, so their frequency is
        // independent of horizontal_scale (terrain tightness). Bigger size = lower
        // frequency = broader landmasses/oceans (~33 km wavelength at size 1).
        let continent_freq = 0.03 / cfg.continent_size.max(0.05);
        Self {
            seed,
            height_scale: cfg.height_scale,
            sea_level_threshold: cfg.sea_level_threshold,
            ocean_transition_width: cfg.ocean_transition_width,
            ocean_depth: cfg.ocean_depth,
            coastal_humidity: cfg.coastal_humidity,
            desert: cfg.desert,
            grasslands: cfg.grasslands,
            forest: cfg.forest,
            taiga: cfg.taiga,
            temp_bias: cfg.temp_bias,
            temp_contrast: cfg.temp_contrast,
            humidity_bias: cfg.humidity_bias,
            humidity_contrast: cfg.humidity_contrast,
            temp_lapse: cfg.temp_lapse,
            latitude_strength: cfg.latitude_strength,
            latitude_band: cfg.latitude_band.max(1.0),
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
            continent_layer: PerlinLayer::new(seed + 600, continent_freq, 1.0),
        }
    }

    /// World seed, used to derive the per-cell runway layout deterministically.
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Sea-level climate at (x, z): `(temperature, humidity)`, 0..1. Altitude
    /// cooling is applied on top in [`Self::natural_raw`], once a height estimate
    /// exists. Thin wrapper over [`Self::climate_and_continent`] for callers that
    /// don't need the continentalness value (kept for upcoming features).
    #[allow(dead_code)]
    pub fn get_climate(&self, x: f32, z: f32) -> (f32, f32) {
        let (temp, hum, _) = self.climate_and_continent(x, z);
        (temp, hum)
    }

    /// Climate plus the land/sea mask at (x, z): `(temperature, humidity,
    /// continentalness)`. Temperature gets the latitude band, humidity gets a
    /// coastal boost from low continentalness (wet coasts, dry interiors), then
    /// both pass through the axis bias/contrast remap. Computing continentalness
    /// here lets the one noise sample feed both the coastal humidity and the ocean
    /// shaping without re-evaluating it.
    fn climate_and_continent(&self, x: f32, z: f32) -> (f32, f32, f32) {
        let raw_temp = self.temperature_layer.get(x, z);
        let raw_hum = self.humidity_layer.get(x, z);
        let temp = ((raw_temp / self.temperature_layer.vertical_scale) + 1.0) * 0.5;
        let hum = ((raw_hum / self.humidity_layer.vertical_scale) + 1.0) * 0.5;
        let continentalness = self.continentalness(x, z);

        // Latitude banding: warm on the equator lines, cold between, periodic in
        // world-Z so an endless world keeps cycling. Mean-zero, so it shifts the
        // pattern rather than the average temperature.
        let latitude =
            self.latitude_strength * (z * std::f32::consts::TAU / self.latitude_band).cos();
        // Coastal humidity: oceans (low continentalness) raise nearby humidity,
        // decaying toward the dry continental interior — the realistic causality
        // where the ocean drives moisture, not the reverse.
        let coastal = self.coastal_humidity * (1.0 - continentalness);
        (
            remap_axis(temp + latitude, self.temp_bias, self.temp_contrast),
            remap_axis(hum + coastal, self.humidity_bias, self.humidity_contrast),
            continentalness,
        )
    }

    /// Land/sea mask at (x, z), 0..1: low = ocean, high = continental interior.
    /// A standalone low-frequency Perlin field, independent of the climate.
    fn continentalness(&self, x: f32, z: f32) -> f32 {
        let raw = self.continent_layer.get(x, z);
        (((raw / self.continent_layer.vertical_scale) + 1.0) * 0.5).clamp(0.0, 1.0)
    }

    /// Biome at (x, z). Kept for upcoming features (vegetation, audio) that key
    /// off biome rather than raw height.
    #[allow(dead_code)]
    pub fn get_biome(&self, x: f32, z: f32) -> Biome {
        let (temp, hum, continentalness) = self.climate_and_continent(x, z);
        if self.ocean_factor(continentalness) >= 0.5 {
            return Biome::Ocean;
        }
        match (temp > 0.5, hum > 0.5) {
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
        let (temp_base, hum, continentalness) = self.climate_and_continent(x, z);
        let ocean = self.ocean_factor(continentalness);

        let mut base = 0.0;
        for layer in &self.terrain_layers {
            base += layer.get(x, z);
        }

        // Altitude → temperature: estimate the elevation using the sea-level
        // climate, then cool by the lapse rate so high ground trends cold/snowy.
        // One fixed-point step breaks the climate↔height circular dependency
        // (height needs climate, cooled climate needs height) without iterating.
        let temp = if self.temp_lapse > 0.0 {
            let altitude = (self.shape_raw(base, temp_base, hum, ocean) * self.height_scale).max(0.0);
            (temp_base - self.temp_lapse * altitude / 1000.0).clamp(0.0, 1.0)
        } else {
            temp_base
        };

        (self.shape_raw(base, temp, hum, ocean), temp, hum)
    }

    /// How strongly a point is open ocean (0 = land, 1 = full sea), from the
    /// continentalness mask: anything below `sea_level_threshold` ramps to ocean
    /// across `ocean_transition_width` (the coastline). Geography decides where
    /// water is — climate no longer does.
    fn ocean_factor(&self, continentalness: f32) -> f32 {
        let width = self.ocean_transition_width.max(1e-3);
        ((self.sea_level_threshold - continentalness) / width).clamp(0.0, 1.0)
    }

    /// Raw (pre-`height_scale`) natural height from the noise `base`, the climate,
    /// and a precomputed `ocean` factor: abundance-weighted blends of biome
    /// elevation and relief, each pulled toward the ocean basin / flat sea floor.
    /// Taking `ocean` as an argument lets the altitude-cooling pass reuse it.
    fn shape_raw(&self, base: f32, temp: f32, humidity: f32, ocean: f32) -> f32 {
        let w = self.biome_weights(temp, humidity);
        let land_elev = w[0] * self.grasslands.elevation
            + w[1] * self.taiga.elevation
            + w[2] * self.desert.elevation
            + w[3] * self.forest.elevation;
        let land_relief = w[0] * self.grasslands.relief
            + w[1] * self.taiga.relief
            + w[2] * self.desert.relief
            + w[3] * self.forest.relief;
        let elevation = land_elev + (-self.ocean_depth - land_elev) * ocean;
        let relief = land_relief + (OCEAN_RELIEF - land_relief) * ocean;
        base * relief + elevation
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

