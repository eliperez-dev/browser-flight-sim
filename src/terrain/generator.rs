//! Procedural terrain height + colour, ported and trimmed from the parent
//! `bevy_sim` world generator. v1 keeps the multi-octave Perlin height stack and
//! a single height-based colour ramp; biomes (temperature/humidity) and oceans
//! from the original are intentionally dropped for now.
//!
//! [`WorldGenerator`] is a [`Resource`] so two very different consumers can share
//! one source of truth: the async chunk-mesh tasks (which displace vertices) and
//! the landing-gear physics (which samples ground height under each strut). It is
//! `Clone` because each async task moves its own copy onto the task pool.

use bevy::prelude::*;
use noise::{NoiseFn, Perlin};

/// Metres per chunk edge. 500 m balances draw-call count (fewer, bigger chunks)
/// against per-chunk mesh-gen cost — the dominant constraint on WASM, where the
/// "async" task pool actually runs on the main thread (see [`super::streaming`]).
pub const CHUNK_SIZE: f32 = 500.0;

/// Vertical exaggeration applied to the summed noise. Gentler than the parent
/// sim's 500 — this is a light-GA sim, not a mountain flyover.
pub const MAP_HEIGHT_SCALE: f32 = 80.0;

/// Multi-octave Perlin terrain. Each [`PerlinLayer`] adds detail at a finer
/// horizontal scale and smaller vertical amplitude; summed they give natural
/// rolling terrain.
#[derive(Resource, Clone)]
pub struct WorldGenerator {
    pub seed: u32,
    layers: Vec<PerlinLayer>,
}

impl WorldGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            // (horizontal_scale, vertical_amplitude). Low scale = broad features.
            layers: vec![
                PerlinLayer::new(seed,       0.08, 4.5),
                PerlinLayer::new(seed,       0.20, 3.5),
                PerlinLayer::new(seed + 100, 0.50, 1.75),
                PerlinLayer::new(seed + 200, 1.00, 0.50),
                PerlinLayer::new(seed + 300, 2.00, 0.40),
            ],
        }
    }

    /// World-space terrain height (metres) at horizontal position (x, z).
    /// Terrain is a height field, so there is no `y` input.
    ///
    /// Terrain is flattened to y=0 within [`FLAT_RADIUS`] of the origin and
    /// smoothly blended to full height by [`BLEND_RADIUS`], giving a flat airfield
    /// around the runway (which sits at y=0) so the aircraft rests flush on it.
    pub fn get_terrain_height(&self, x: f32, z: f32) -> f32 {
        let mut h = 0.0;
        for layer in &self.layers {
            h += layer.get(x, z);
        }
        h * MAP_HEIGHT_SCALE * origin_flatten(x, z)
    }
}

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

#[derive(Clone)]
struct PerlinLayer {
    perlin: Perlin,
    horizontal_scale: f32,
    vertical_scale: f32,
    /// Per-layer domain offset so octaves seeded from the same `seed` don't line
    /// up their zero-crossings into visible grid artifacts.
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

/// A height threshold and the colour terrain takes at or above it. Stops must be
/// listed in ascending height order.
struct ColorStop {
    height: f32,
    color: Color,
}

/// Single height ramp standing in for the parent sim's per-biome palettes.
/// Heights are in world metres (post [`MAP_HEIGHT_SCALE`]).
const TERRAIN_RAMP: &[ColorStop] = &[
    ColorStop { height: -40.0, color: Color::srgb(0.15, 0.35, 0.55) }, // deep water-ish
    ColorStop { height: -2.0,  color: Color::srgb(0.80, 0.72, 0.50) }, // sand
    ColorStop { height: 10.0,  color: Color::srgb(0.22, 0.52, 0.16) }, // grass
    ColorStop { height: 60.0,  color: Color::srgb(0.45, 0.40, 0.32) }, // rock
    ColorStop { height: 110.0, color: Color::srgb(0.95, 0.95, 0.97) }, // snow
];

/// Linear-RGBA colour for a vertex at world height `h`, smoothly blended between
/// the two bracketing [`TERRAIN_RAMP`] stops. Returned as a raw array ready for
/// `Mesh::ATTRIBUTE_COLOR`.
pub fn terrain_color(h: f32) -> [f32; 4] {
    if h <= TERRAIN_RAMP[0].height {
        return TERRAIN_RAMP[0].color.to_linear().to_f32_array();
    }
    let last = TERRAIN_RAMP.len() - 1;
    if h >= TERRAIN_RAMP[last].height {
        return TERRAIN_RAMP[last].color.to_linear().to_f32_array();
    }
    for i in 1..TERRAIN_RAMP.len() {
        let upper = &TERRAIN_RAMP[i];
        if h <= upper.height {
            let lower = &TERRAIN_RAMP[i - 1];
            let t = (h - lower.height) / (upper.height - lower.height);
            let a = lower.color.to_linear();
            let b = upper.color.to_linear();
            return a.mix(&b, t).to_f32_array();
        }
    }
    TERRAIN_RAMP[last].color.to_linear().to_f32_array()
}
