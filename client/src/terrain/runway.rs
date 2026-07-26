//! Seeded, streamed runway placement. Runways live on a 5 km grid: each cell
//! deterministically yields one airport (jittered position, seeded heading) from
//! the world seed, so the layout is infinite, matches the terrain, and is the
//! same for everyone on a seed. Nothing is stored globally — a cell's runways are
//! recomputed on demand wherever it's needed (terrain flattening, the gear,
//! visual spawning).
//!
//! Each cell produces one [`AirportLayout`], which may contain one or more
//! [`RunwayInstance`]s with their own dimensions and surface type.
//!
//! Terrain is levelled in an oriented *rectangle* around each strip (not a
//! circle), so the flat ground hugs the runway shape. The visible asphalt +
//! markings stream in around the camera and despawn behind it, like the chunks.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;

use super::chunk::ChunkManager;
use super::generator::{WorldGenerator, CHUNK_SIZE};
use super::TerrainCamera;

/// Default small-GA dimensions, kept for the map overlay / public API.
pub const RUNWAY_WIDTH: f32 = 45.0;
pub const RUNWAY_LENGTH: f32 = 2000.0;

// ---- Airport layout types ---------------------------------------------------

/// What kind of airport a grid cell contains. Determines strip count, dimensions,
/// and surface material. Chosen deterministically from the cell hash so the world
/// is stable across sessions.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AirportKind {
    /// Narrow dirt strip, shorter random length (400–900 m). Brown packed earth.
    DirtStrip,
    /// Standard light-GA paved runway (~45 × 2000 m).
    SmallGA,
    /// Single longer/wider commuter runway (~60 × 3200 m).
    LargeCommuter,
    /// Two parallel GA strips separated ~350 m laterally.
    Regional,
    /// Two parallel wide/long runways separated ~400 m (hub airport).
    Hub,
}

impl AirportKind {
    fn from_hash(h: u32) -> Self {
        match h % 100 {
            0..25  => AirportKind::DirtStrip,
            25..60 => AirportKind::SmallGA,
            60..75 => AirportKind::LargeCommuter,
            75..85 => AirportKind::Regional,
            _       => AirportKind::Hub,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AirportKind::DirtStrip     => "Dirt Strip",
            AirportKind::SmallGA       => "Small GA",
            AirportKind::LargeCommuter => "Large Commuter",
            AirportKind::Regional  => "Regional",
            AirportKind::Hub           => "Hub",
        }
    }
}

/// One airport: a grid cell with one or more runway strips sharing the same
/// ident and location. The first strip is the "primary" (used for the map pin
/// position and heading indicator); additional strips are parallel twins.
#[derive(Clone)]
pub struct Airport {
    pub cell: (i32, i32),
    pub kind: AirportKind,
    /// All strips. Never empty.
    pub strips: Vec<RunwayInstance>,
}

impl Airport {
    /// World position of the airport centre (average of all strip centres).
    pub fn pos(&self) -> (f32, f32) {
        let n = self.strips.len() as f32;
        let x = self.strips.iter().map(|r| r.x).sum::<f32>() / n;
        let z = self.strips.iter().map(|r| r.z).sum::<f32>() / n;
        (x, z)
    }

    /// Primary strip (used for heading line and runway numbers on the map).
    pub fn primary(&self) -> &RunwayInstance {
        &self.strips[0]
    }
}

/// Terrain is fully levelled within the runway rectangle expanded by this apron
/// (metres beyond each edge), then ramps back to natural height over
/// [`RUNWAY_BLEND_MARGIN`]. Keep the blend well under the spacing so terrain
/// survives between fields.
const RUNWAY_FLAT_APRON: f32 = 320.0;
const RUNWAY_BLEND_MARGIN: f32 = 600.0;

/// How far the asphalt surface sits above the graded ground (metres). Gives the
/// slab visible thickness and clean separation from the terrain (no coplanar
/// z-fighting). The landing gear rests on this raised surface over the pavement,
/// so wheels sit on the asphalt rather than sinking to the graded ground.
const RUNWAY_SURFACE_LIFT: f32 = 0.3;

/// How far the *graded terrain* under and around a runway is pushed below the
/// strip elevation (metres). The slab top sits at `elevation + RUNWAY_SURFACE_LIFT`;
/// terrain is graded to `elevation - RUNWAY_TERRAIN_DROP`, opening a gap of
/// `RUNWAY_SURFACE_LIFT + RUNWAY_TERRAIN_DROP` between the two so the coarse-LOD
/// terrain mesh can't poke through the flat slab when viewed top-down. The slab's
/// own thickness must cover this drop (see `THICKNESS`) so its skirt hides the gap.
const RUNWAY_TERRAIN_DROP: f32 = 2.0;

/// Hard cap on how far runway pavement/lights stream in, independent of the
/// terrain render-distance setting.
const RUNWAY_MAX_VIEW_M: f32 = 30_000.0;

/// Grid spacing between runways (one per cell).
const RUNWAY_SPACING: f32 = 6000.0;
/// Fraction of a cell a runway may wander from its grid point — keeps them off a
/// perfect lattice. Bounded < 0.5 so a runway stays within its own cell.
const RUNWAY_JITTER: f32 = 0.65;

/// Sea level (world-Y). A cell whose natural ground is below this (plus a small
/// margin) is water, so no runway is placed there — the strip would sit in the
/// sea. The origin runway is exempt (it's the fixed spawn point at y=0).
const WATER_LEVEL: f32 = 0.0;
const RUNWAY_MIN_ABOVE_WATER: f32 = 3.0;

/// Max natural-terrain elevation swing (metres) allowed across a runway's own
/// footprint before the site is rejected as too rugged. Grading always makes the
/// strip dead flat regardless of what's underneath, so without this check a
/// runway can land across a ridge or cliff face and carve an ugly gouge into the
/// mountainside. Comparable to a real siting requirement (max grade over the
/// touchdown zone); tuned loose enough to still allow gentle hills.
const RUNWAY_MAX_RELIEF: f32 = 100.0;

/// One placed runway strip. `elevation` is the world-Y its surface sits at.
#[derive(Clone, Copy)]
pub struct RunwayInstance {
    pub x: f32,
    pub z: f32,
    pub heading: f32,
    pub elevation: f32,
    pub width: f32,
    pub length: f32,
    pub kind: AirportKind,
}


impl RunwayInstance {
    /// The two runway numbers (one per direction), each `round(heading° / 10)`
    /// mapped to 1..=36, with the reciprocal 180° opposite. e.g. heading 170° →
    /// `(17, 35)`. Returned low-number-first by convention.
    pub fn runway_numbers(&self) -> (u32, u32) {
        let deg = self.heading.to_degrees().rem_euclid(360.0);
        let mut a = (deg / 10.0).round() as i32 % 36;
        if a == 0 {
            a = 36;
        }
        let mut b = a + 18;
        if b > 36 {
            b -= 36;
        }
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        (lo as u32, hi as u32)
    }

    /// World (x, z) → the strip's own frame: origin at the strip center, +Z
    /// along its heading, +X across it. Shared by every method below that
    /// needs to know where a point falls relative to the runway rectangle.
    fn to_local(&self, x: f32, z: f32) -> (f32, f32) {
        let (s, c) = self.heading.sin_cos();
        let px = x - self.x;
        let pz = z - self.z;
        (px * c - pz * s, px * s + pz * c)
    }

    /// Flatten weight at world (x, z): 1.0 inside the runway rectangle (plus
    /// apron), ramping to 0.0 by [`RUNWAY_BLEND_MARGIN`] beyond it. Distance is
    /// measured in the runway's own frame, so the levelled zone is a rounded
    /// rectangle aligned to the strip rather than a circle.
    fn flatten_weight(&self, x: f32, z: f32) -> f32 {
        let (lx, lz) = self.to_local(x, z);

        let half_w = self.width * 0.5 + RUNWAY_FLAT_APRON;
        let half_l = self.length * 0.5 + RUNWAY_FLAT_APRON;
        let dx = (lx.abs() - half_w).max(0.0);
        let dz = (lz.abs() - half_l).max(0.0);
        let d = (dx * dx + dz * dz).sqrt();

        let t = (d / RUNWAY_BLEND_MARGIN).clamp(0.0, 1.0);
        1.0 - t * t * (3.0 - 2.0 * t)
    }

    /// Whether world (x, z) lies on the surface rectangle — where the gear rests.
    fn on_pavement(&self, x: f32, z: f32) -> bool {
        let (lx, lz) = self.to_local(x, z);
        lx.abs() <= self.width * 0.5 && lz.abs() <= self.length * 0.5
    }
}

/// Marks a spawned runway root entity (its meshes are children), tagged with its
/// grid cell and world position so the streaming system can despawn it by actual
/// distance when it leaves range. `width`/`length` are kept so
/// `stream_runway_lights` can rebuild this strip's light layout without
/// re-deriving it from `runways_for_cell`; `is_lit` is false for dirt strips,
/// which never get lights at all.
#[derive(Component)]
pub struct RunwaySlab {
    pub cell: (i32, i32),
    pub pos: Vec2,
    #[allow(dead_code)]
    pub elevation: f32,
    width: f32,
    length: f32,
    is_lit: bool,
}

/// Marks the light-group child entity a runway's `PointLight`s are spawned
/// under, so `stream_runway_lights` can find and despawn/respawn just the
/// lights — independent of the pavement (`RunwaySlab`) — as the camera
/// crosses `LIGHTS_MAX_VIEW_M`. The distance check itself reads the parent
/// `RunwaySlab::pos`, so this carries no data of its own.
#[derive(Component)]
pub(crate) struct RunwayLights;

/// Marks a REIL strobe — flashes at ~1 Hz.
#[derive(Component)]
pub struct ReilLight;

/// Marks one bar of an approach lighting sequence. `index` is the bar's distance
/// step from the threshold (1 = nearest, N = farthest); `end` is +1 or -1 for
/// which runway end this belongs to.
#[derive(Component)]
pub struct AlsLight {
    pub index: i32,
}

/// Global clock driving runway light animation.
#[derive(Resource, Default)]
pub struct RunwayLightClock {
    pub elapsed: f32,
}

/// User-facing toggle (Graphics menu) for runway lighting — every edge/
/// threshold/REIL/ALS `PointLight`. Off entirely skips spawning them (see
/// `stream_runway_lights`) rather than just hiding them, since the point is
/// cutting live-light count for the clustered renderer, not just visuals.
#[derive(Resource, Clone, Copy, PartialEq, Eq)]
pub struct RunwayLightsEnabled(pub bool);

impl Default for RunwayLightsEnabled {
    fn default() -> Self {
        Self(true)
    }
}

/// Grid cells whose runway is currently spawned (visually), plus a cache of
/// every evaluated cell's `runways_for_cell` result (including cells with no
/// runway at all). `runways_for_cell` samples multi-octave noise —
/// elevation, plus 6 more samples per candidate through
/// `site_is_flat_enough` — expensive enough that re-running it every frame
/// for the whole scanned ring (wider than the spawn radius, see
/// `stream_runways`) was a real, ongoing per-frame cost near any runway-dense
/// or otherwise-flat area, not just a one-time spawn cost. Caching the result
/// (not just whether it's currently spawned) means a cell is only ever
/// hashed/sampled once as long as it stays within the scan radius, not every
/// frame forever.
#[derive(Resource, Default)]
pub struct SpawnedRunways {
    cells: HashSet<(i32, i32)>,
    layout_cache: bevy::platform::collections::HashMap<(i32, i32), Vec<RunwayInstance>>,
}

/// Centreline dash size (metres). Every dash on every runway is the same
/// size, so `RunwayMaterials` builds one shared mesh instead of each dash
/// getting its own mesh asset.
const DASH_LEN: f32 = 30.0;
const DASH_W: f32 = 1.0;

/// Threshold "piano key" bar size (metres); same one-shared-mesh reasoning as
/// the centreline dash above.
const BAR_W: f32 = 2.5;
const BAR_LEN: f32 = 20.0;

/// Shared surface/paint materials and paint-marking meshes, built once so
/// every runway batches together instead of each spawning its own duplicate
/// mesh assets — every dash/bar across every runway is the same fixed size
/// (`DASH_W`×`DASH_LEN`, `BAR_W`×`BAR_LEN`), so one handle per shape covers
/// all of them; only the `Transform` differs per instance.
#[derive(Resource)]
pub struct RunwayMaterials {
    pub asphalt: Handle<StandardMaterial>,
    pub paint: Handle<StandardMaterial>,
    pub dirt: Handle<StandardMaterial>,
    pub stalk: Handle<StandardMaterial>,
    dash_mesh: Handle<Mesh>,
    bar_mesh: Handle<Mesh>,
}

impl RunwayMaterials {
    pub fn new(materials: &mut Assets<StandardMaterial>, meshes: &mut Assets<Mesh>) -> Self {
        Self {
            asphalt: materials.add(StandardMaterial {
                base_color: Color::srgb(0.12, 0.12, 0.13),
                perceptual_roughness: 1.0,
                ..default()
            }),
            paint: materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.9, 0.9),
                perceptual_roughness: 1.0,
                ..default()
            }),
            dirt: materials.add(StandardMaterial {
                base_color: Color::srgb(0.48, 0.35, 0.22),
                perceptual_roughness: 1.0,
                ..default()
            }),
            stalk: materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 1.0, 1.0),
                unlit: true,
                fog_enabled: false,
                ..default()
            }),
            dash_mesh: meshes.add(Plane3d::default().mesh().size(DASH_W, DASH_LEN)),
            bar_mesh: meshes.add(Plane3d::default().mesh().size(BAR_W, BAR_LEN)),
        }
    }
}

// --- Deterministic per-cell layout -------------------------------------------

/// Integer avalanche hash (lowbias32 by Chris Wellons).
fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Uniform `[0, 1)` from a hash state.
fn hash01(x: u32) -> f32 {
    (hash_u32(x) >> 8) as f32 / (1u32 << 24) as f32
}

/// A stable hash for grid cell `(gx, gz)` under `seed`.
fn cell_hash(seed: u32, gx: i32, gz: i32) -> u32 {
    let a = hash_u32(seed ^ 0x9e37_79b9);
    let b = hash_u32(a.wrapping_add(gx as u32).wrapping_mul(0x85eb_ca6b));
    hash_u32(b.wrapping_add(gz as u32).wrapping_mul(0xc2b2_ae35))
}

// Dirt strip length range (metres).
const DIRT_MIN_LEN: f32 = 400.0;
const DIRT_MAX_LEN: f32 = 900.0;
const DIRT_WIDTH: f32 = 20.0;

// Large commuter strip dimensions.
const LARGE_WIDTH: f32 = 80.0;
const LARGE_LENGTH: f32 = 3200.0;

// Parallel spacing for twin / hub layouts (lateral offset between centrelines).
const TWIN_OFFSET: f32 = 70.0;
const HUB_OFFSET: f32 = 180.0;

/// Samples natural (unflattened) terrain height at the corners and edge
/// midpoints of a runway's own footprint (in its heading-aligned frame) and
/// rejects the site if the spread exceeds [`RUNWAY_MAX_RELIEF`]. Cheap: a
/// handful of closed-form noise evaluations, same cost class as the existing
/// water-level check.
fn site_is_flat_enough(
    generator: &WorldGenerator,
    x: f32,
    z: f32,
    heading: f32,
    length: f32,
    width: f32,
    center_elevation: f32,
) -> bool {
    let (s, c) = heading.sin_cos();
    let half_l = length * 0.5;
    let half_w = width * 0.5;

    // Local-frame sample offsets: both ends, both side edges at both ends, and
    // the midpoint of each long edge — enough to catch a cross-slope, a
    // lengthwise grade, or a diagonal ridge cutting through the strip.
    let offsets: [(f32, f32); 6] = [
        (-half_l, -half_w),
        (-half_l, half_w),
        (half_l, -half_w),
        (half_l, half_w),
        (0.0, -half_w),
        (0.0, half_w),
    ];

    let mut min_h = center_elevation;
    let mut max_h = center_elevation;
    for (lx, lz) in offsets {
        let wx = x + lx * c + lz * s;
        let wz = z - lx * s + lz * c;
        let h = generator.natural_height(wx, wz);
        min_h = min_h.min(h);
        max_h = max_h.max(h);
    }

    (max_h - min_h) <= RUNWAY_MAX_RELIEF
}

/// Returns all runway strips for grid cell `(gx, gz)`. Water cells return empty.
/// Cell (0,0) is always a small-GA strip at heading 0 (the aircraft spawn point).
fn runways_for_cell(generator: &WorldGenerator, gx: i32, gz: i32) -> Vec<RunwayInstance> {
    if gx == 0 && gz == 0 {
        return vec![RunwayInstance {
            x: 0.0,
            z: 0.0,
            heading: 0.0,
            elevation: generator.natural_height(0.0, 0.0),
            width: RUNWAY_WIDTH,
            length: RUNWAY_LENGTH,
            kind: AirportKind::SmallGA,
        }];
    }

    let h = cell_hash(generator.seed(), gx, gz);

    if !hash_u32(h ^ 0xdeadbeef).is_multiple_of(2) {
        return vec![];
    }

    let jitter_x = (hash01(h) - 0.5) * RUNWAY_SPACING * RUNWAY_JITTER;
    let jitter_z = (hash01(h ^ 0x68bc_21eb) - 0.5) * RUNWAY_SPACING * RUNWAY_JITTER;
    let x = gx as f32 * RUNWAY_SPACING + jitter_x;
    let z = gz as f32 * RUNWAY_SPACING + jitter_z;
    let elevation = generator.natural_height(x, z);
    if elevation < WATER_LEVEL + RUNWAY_MIN_ABOVE_WATER {
        return vec![];
    }

    let heading = hash01(h ^ 0x2545_f491) * std::f32::consts::PI;
    let kind = AirportKind::from_hash(hash_u32(h ^ 0x1a2b_3c4d) % 100);

    // Reject sites where the ground under the strip's own footprint is too
    // uneven — the strip's *length* determines the footprint tested, so a long
    // commuter runway is held to the same absolute grade as a short dirt strip
    // scaled to its own extent (a short strip on a steep local slope covers
    // less rise than a long one at the same grade).
    let (test_len, test_width) = match kind {
        AirportKind::DirtStrip     => (DIRT_MAX_LEN, DIRT_WIDTH),
        AirportKind::SmallGA       => (RUNWAY_LENGTH, RUNWAY_WIDTH),
        AirportKind::LargeCommuter => (LARGE_LENGTH, LARGE_WIDTH),
        AirportKind::Regional      => (RUNWAY_LENGTH, RUNWAY_WIDTH + TWIN_OFFSET),
        AirportKind::Hub           => (LARGE_LENGTH, LARGE_WIDTH + HUB_OFFSET),
    };
    if !site_is_flat_enough(generator, x, z, heading, test_len, test_width, elevation) {
        return vec![];
    }

    match kind {
        AirportKind::DirtStrip => {
            let length = DIRT_MIN_LEN + hash01(h ^ 0xf00d_cafe) * (DIRT_MAX_LEN - DIRT_MIN_LEN);
            vec![RunwayInstance { x, z, heading, elevation, width: DIRT_WIDTH, length, kind }]
        }
        AirportKind::SmallGA => {
            vec![RunwayInstance { x, z, heading, elevation, width: RUNWAY_WIDTH, length: RUNWAY_LENGTH, kind }]
        }
        AirportKind::LargeCommuter => {
            vec![RunwayInstance { x, z, heading, elevation, width: LARGE_WIDTH, length: LARGE_LENGTH, kind }]
        }
        AirportKind::Regional => {
            // Two small-GA strips offset laterally (perpendicular to heading).
            let (s, c) = heading.sin_cos();
            let ox = c * TWIN_OFFSET;
            let oz = -s * TWIN_OFFSET;
            vec![
                RunwayInstance { x: x - ox * 0.5, z: z - oz * 0.5, heading, elevation, width: RUNWAY_WIDTH, length: RUNWAY_LENGTH, kind },
                RunwayInstance { x: x + ox * 0.5, z: z + oz * 0.5, heading, elevation, width: RUNWAY_WIDTH, length: RUNWAY_LENGTH, kind },
            ]
        }
        AirportKind::Hub => {
            let (s, c) = heading.sin_cos();
            let ox = c * HUB_OFFSET;
            let oz = -s * HUB_OFFSET;
            vec![
                RunwayInstance { x: x - ox * 0.5, z: z - oz * 0.5, heading, elevation, width: LARGE_WIDTH, length: LARGE_LENGTH, kind },
                RunwayInstance { x: x + ox * 0.5, z: z + oz * 0.5, heading, elevation, width: LARGE_WIDTH, length: LARGE_LENGTH, kind },
            ]
        }
    }
}

/// Full procedural name for an airport, e.g. `"Cedar Vance Regional Airport"`.
/// 75% of airports get an aviation suffix (Airfield, Airport, etc.); the other
/// 25% get a geographic place-name suffix (Falls, Creek, Junction, etc.).
pub fn airport_name(seed: u32, cell: (i32, i32), kind: AirportKind) -> String {
    use crate::airport_names::{NAMES, PREFIXES, SUFFIXES};
    let h = cell_hash(seed, cell.0, cell.1);
    let prefix = PREFIXES[(hash_u32(h ^ 0xaaaa_1111) as usize) % PREFIXES.len()];
    let name   = NAMES  [(hash_u32(h ^ 0xbbbb_2222) as usize) % NAMES.len()];
    let use_aviation_suffix = !hash_u32(h ^ 0xcccc_3333).is_multiple_of(4); // 75%
    let suffix = if use_aviation_suffix {
        match kind {
            AirportKind::DirtStrip     => "Strip",
            AirportKind::SmallGA       => "Airfield",
            AirportKind::LargeCommuter => "Airport",
            AirportKind::Regional      => "Regional Airport",
            AirportKind::Hub           => "International",
        }
    } else {
        SUFFIXES[(hash_u32(h ^ 0xdddd_4444) as usize) % SUFFIXES.len()]
    };
    format!("{prefix} {name} {suffix}")
}

/// ICAO-style callsign derived from the airport's generated name: `K` + first
/// letter of each word, uppercased, truncated/padded to 4 characters total.
/// Stable per cell — same name always yields the same callsign.
pub fn runway_ident(seed: u32, cell: (i32, i32)) -> String {
    // We need the kind to generate the name, but ident is called without kind
    // in some paths. Use a placeholder kind just for the word structure — the
    // actual words (prefix + name) are kind-independent, only the suffix differs,
    // and we only take initials from prefix+name anyway.
    use crate::airport_names::{NAMES, PREFIXES};
    let h = cell_hash(seed, cell.0, cell.1);
    let prefix = PREFIXES[(hash_u32(h ^ 0xaaaa_1111) as usize) % PREFIXES.len()];
    let name   = NAMES  [(hash_u32(h ^ 0xbbbb_2222) as usize) % NAMES.len()];

    // Collect first letters of each whitespace-separated word across prefix + name.
    let mut ident = String::with_capacity(4);
    ident.push('K');
    for word in prefix.split_whitespace().chain(name.split_whitespace()) {
        if ident.len() >= 4 { break; }
        if let Some(c) = word.chars().next() {
            ident.push(c.to_ascii_uppercase());
        }
    }
    // Pad to 4 with deterministic random letters (not just 'A').
    let mut pad_hash = hash_u32(h ^ 0xcccc_3333);
    while ident.len() < 4 {
        let c = (b'A' + (pad_hash % 26) as u8) as char;
        ident.push(c);
        pad_hash = hash_u32(pad_hash);
    }
    ident
}


/// Runways for every cell overlapping the world-space box, padded by one cell so
/// strips whose influence reaches in from a neighbour are included (water cells
/// yield nothing). Used to resolve a chunk's runways once, before the per-vertex
/// flatten.
pub fn runways_in_region(
    generator: &WorldGenerator,
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
) -> Vec<RunwayInstance> {
    let gx0 = (min_x / RUNWAY_SPACING).round() as i32 - 1;
    let gx1 = (max_x / RUNWAY_SPACING).round() as i32 + 1;
    let gz0 = (min_z / RUNWAY_SPACING).round() as i32 - 1;
    let gz1 = (max_z / RUNWAY_SPACING).round() as i32 + 1;
    let mut out = Vec::new();
    for gx in gx0..=gx1 {
        for gz in gz0..=gz1 {
            out.extend(runways_for_cell(generator, gx, gz));
        }
    }
    out
}

/// One [`Airport`] per non-water cell in the region — one pin per cell regardless
/// of how many parallel strips the airport has. Used by the map overlay so twin /
/// hub airports don't produce duplicate markers.
pub fn airports_in_region(
    generator: &WorldGenerator,
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
) -> Vec<Airport> {
    let gx0 = (min_x / RUNWAY_SPACING).round() as i32 - 1;
    let gx1 = (max_x / RUNWAY_SPACING).round() as i32 + 1;
    let gz0 = (min_z / RUNWAY_SPACING).round() as i32 - 1;
    let gz1 = (max_z / RUNWAY_SPACING).round() as i32 + 1;
    let mut out = Vec::new();
    for gx in gx0..=gx1 {
        for gz in gz0..=gz1 {
            let strips = runways_for_cell(generator, gx, gz);
            if strips.is_empty() { continue; }
            let kind = strips[0].kind;
            out.push(Airport { cell: (gx, gz), kind, strips });
        }
    }
    out
}

/// Blends `natural` height toward the nearest runway's elevation using the
/// rectangle weight. The strongest (nearest) runway wins where they overlap.
/// Cheap: pure rectangle math, no noise — the caller resolves `runways` once.
pub fn flatten_against(runways: &[RunwayInstance], x: f32, z: f32, natural: f32) -> f32 {
    let mut best_w = 0.0_f32;
    let mut best_e = 0.0_f32;
    for r in runways {
        let w = r.flatten_weight(x, z);
        if w > best_w {
            best_w = w;
            best_e = r.elevation;
        }
    }
    // Target sits a clear margin below the slab surface so the terrain mesh —
    // whose vertices are sparse at distance LODs and interpolate linearly between
    // samples — never rises through the flat slab when viewed top-down. The slab
    // top is at `elevation + RUNWAY_SURFACE_LIFT`; dropping the graded terrain a
    // further `RUNWAY_TERRAIN_DROP` below `elevation` opens a gap large enough to
    // survive depth-buffer precision at altitude and coarse-LOD vertex spacing.
    let target = best_e - RUNWAY_TERRAIN_DROP;
    natural + (target - natural) * best_w
}

/// The walkable ground height the landing gear should rest on at a single point:
/// the flattened terrain, raised to the asphalt surface where the point is over
/// pavement (so wheels sit on the runway, not the graded ground beneath it).
/// Resolves the nearby runways once; for one-off gear queries, not the per-vertex
/// mesh path (which uses [`flatten_against`] — no lift, since the slab mesh
/// provides the raised surface visually).
pub fn ground_height(generator: &WorldGenerator, x: f32, z: f32, natural: f32) -> f32 {
    let runways = runways_in_region(generator, x, z, x, z);
    let mut ground = flatten_against(&runways, x, z, natural);
    for r in &runways {
        if r.on_pavement(x, z) {
            ground = ground.max(r.elevation + RUNWAY_SURFACE_LIFT);
        }
    }
    ground
}

// --- Streaming the visible strips --------------------------------------------

/// Spawns runways near the camera and despawns ones that fall out of range, so
/// only a handful exist at once regardless of how far you fly. The streamed
/// radius tracks the terrain render distance so strips don't appear beyond the
/// terrain edge. A seed/scale change (generator rebuilt) wipes the set so the
/// new layout repopulates next frame.
#[allow(clippy::too_many_arguments)]
pub fn stream_runways(
    mut commands: Commands,
    generator: Res<WorldGenerator>,
    manager: Res<ChunkManager>,
    materials: Res<RunwayMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&Transform, With<TerrainCamera>>,
    existing: Query<(Entity, &RunwaySlab)>,
    stalks: Query<(Entity, &WaypointStalk)>,
    mut spawned: ResMut<SpawnedRunways>,
) {
    if generator.is_changed() {
        for (entity, _) in &existing {
            commands.entity(entity).despawn();
        }
        for (entity, _) in &stalks {
            commands.entity(entity).despawn();
        }
        spawned.cells.clear();
        spawned.layout_cache.clear();
        return;
    }

    let Ok(cam) = camera.single() else { return };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let cam_cx = (cam.translation.x / RUNWAY_SPACING).round() as i32;
    let cam_cz = (cam.translation.z / RUNWAY_SPACING).round() as i32;

    // Cull by actual distance (not cell count) so strips disappear right around
    // the terrain edge. Spawn within the view radius, despawn a little past it —
    // the gap is hysteresis so a strip at the boundary doesn't flicker. Also capped
    // at RUNWAY_MAX_VIEW_M regardless of terrain render distance, so pavement/lights
    // don't stream in absurdly far when the player cranks up terrain draw distance.
    let view_m = (manager.render_distance as f32 * CHUNK_SIZE).min(RUNWAY_MAX_VIEW_M);
    let keep_sq = view_m * view_m;
    let drop_sq = (view_m * 1.15).powi(2);
    let cell_radius = (view_m * 1.15 / RUNWAY_SPACING).ceil() as i32 + 1;
    // Cache eviction uses a wider radius than drop_sq so a cell doesn't get
    // evicted and immediately re-scanned (and re-sampled) the very next
    // frame just from being right at the scan boundary.
    let cache_evict_sq = (view_m * 1.5).powi(2);

    for gx in (cam_cx - cell_radius)..=(cam_cx + cell_radius) {
        for gz in (cam_cz - cell_radius)..=(cam_cz + cell_radius) {
            let cell = (gx, gz);
            if spawned.cells.contains(&cell) {
                continue;
            }
            // Cached from an earlier frame (this cell's layout is a pure
            // function of the seed/cell, so a stale cache entry is always
            // still correct) — skip the noise sampling entirely.
            let strips = if let Some(cached) = spawned.layout_cache.get(&cell) {
                cached.clone()
            } else {
                let strips = runways_for_cell(&generator, gx, gz);
                spawned.layout_cache.insert(cell, strips.clone());
                strips
            };
            if strips.is_empty() {
                continue;
            }
            // Use the first strip's position for distance culling (all strips in a
            // cell share roughly the same centre).
            if Vec2::new(strips[0].x, strips[0].z).distance_squared(cam_xz) <= keep_sq {
                // Compute the airport centre for the stalk (average of all strip positions).
                let n = strips.len() as f32;
                let centre_x = strips.iter().map(|r| r.x).sum::<f32>() / n;
                let centre_z = strips.iter().map(|r| r.z).sum::<f32>() / n;
                let centre_elev = strips.iter().map(|r| r.elevation).sum::<f32>() / n;
                for (i, inst) in strips.iter().enumerate() {
                    spawn_runway(
                        &mut commands, &mut meshes, &materials, inst, (gx, gz),
                        if i == 0 { Some((centre_x, centre_elev, centre_z)) } else { None },
                        generator.seed(),
                    );
                }
                spawned.cells.insert(cell);
            }
        }
    }

    for (entity, slab) in &existing {
        if slab.pos.distance_squared(cam_xz) > drop_sq {
            commands.entity(entity).despawn();
            spawned.cells.remove(&slab.cell);
        }
    }
    for (entity, stalk) in &stalks {
        if !spawned.cells.contains(&stalk.cell) {
            commands.entity(entity).despawn();
        }
    }

    // Bound the cache's memory over a long flight: drop entries for cells far
    // enough behind that they're unlikely to be revisited soon. Uses the
    // cell's *grid* position (not a noise sample) so this is cheap.
    spawned.layout_cache.retain(|&(gx, gz), _| {
        let pos = Vec2::new(gx as f32 * RUNWAY_SPACING, gz as f32 * RUNWAY_SPACING);
        pos.distance_squared(cam_xz) <= cache_evict_sq
    });
}

/// Spawns/despawns each lit runway's light group as the camera crosses
/// `LIGHTS_MAX_VIEW_M`, independent of the pavement (`RunwaySlab`) itself —
/// see `LIGHTS_MAX_VIEW_M`'s doc comment for why lights need a much shorter
/// streaming radius than pavement. Also gated on `RunwayLightsEnabled` (the
/// Graphics-menu toggle) and daytime: real runway lights aren't lit in
/// daylight either, and skipping them then is the same live-PointLight-count
/// win as the distance cap, just time-gated instead of distance-gated — when
/// either is off, every currently-spawned light group is despawned outright
/// rather than merely hidden, so daytime flying costs nothing extra.
pub fn stream_runway_lights(
    mut commands: Commands,
    lights_enabled: Res<RunwayLightsEnabled>,
    day_night: Res<crate::sky::DayNightCycle>,
    slabs: Query<(Entity, &RunwaySlab, &Children)>,
    light_groups: Query<Entity, With<RunwayLights>>,
    camera: Query<&Transform, With<TerrainCamera>>,
) {
    let want_lights = lights_enabled.0 && day_night.is_night();
    if !want_lights {
        for (_, _, children) in &slabs {
            for &child in children {
                if light_groups.contains(child) {
                    commands.entity(child).despawn();
                }
            }
        }
        return;
    }

    let Ok(cam) = camera.single() else { return };
    let cam_xz = Vec2::new(cam.translation.x, cam.translation.z);
    let keep_sq = LIGHTS_MAX_VIEW_M * LIGHTS_MAX_VIEW_M;
    // Hysteresis so a strip right at the boundary doesn't spawn/despawn its
    // lights every frame — same pattern as stream_runways' keep_sq/drop_sq.
    let drop_sq = (LIGHTS_MAX_VIEW_M * 1.15).powi(2);

    for (root, slab, children) in &slabs {
        if !slab.is_lit {
            continue;
        }
        let dist_sq = slab.pos.distance_squared(cam_xz);
        let light_group = children.iter().find(|&c| light_groups.contains(c));

        match (light_group, dist_sq) {
            (None, d) if d <= keep_sq => {
                commands.entity(root).with_children(|parent| {
                    let half_len = slab.length * 0.5;
                    spawn_runway_lights(parent, slab.width, slab.length, half_len, RUNWAY_SURFACE_LIFT);
                });
            }
            (Some(light_entity), d) if d > drop_sq => {
                commands.entity(light_entity).despawn();
            }
            _ => {}
        }
    }
}

// --- Waypoint stalk constants ------------------------------------------------

const STALK_BASE_OFFSET: f32 = 2.0;
const STALK_TIP_OFFSET: f32 = 1000.0;
/// Authored radius at the reference distance (10 km). The scale system keeps
/// this apparent size constant; height is never scaled.
const STALK_RADIUS: f32 = 8.0;

// --- Runway lighting constants -----------------------------------------------

/// Height of light fixtures above the asphalt surface (metres).
const LIGHT_Y: f32 = 0.5;

/// Edge lights every this many metres along both sides of the runway. Real
/// runway edge lights are ~60m apart; this is deliberately much sparser —
/// each light is a live `PointLight` in Bevy's clustered forward renderer,
/// and a runway-dense area can have several airports streamed in at once
/// (see `LIGHTS_MAX_VIEW_M`), so light *count* is a real performance cost,
/// not just a visual one.
const EDGE_LIGHT_SPACING: f32 = 240.0;

/// Runway lights stream in/out at a much shorter radius than the pavement
/// itself (`RUNWAY_MAX_VIEW_M`) — strobes and edge lights aren't meaningfully
/// visible at range, and each one is a live `PointLight`, so capping how many
/// can be simultaneously active is the main lever on clustered-lighting cost
/// with several runway-dense airports in view. Independently toggled by
/// `stream_runway_lights`, not the initial `spawn_runway` distance check.
const LIGHTS_MAX_VIEW_M: f32 = 4_000.0;

/// Lateral inset from the runway edge for the edge light row.
const EDGE_LIGHT_INSET: f32 = 1.5;

/// PointLight `range` (metres) for each light type. Controls how far the light
/// reaches before it cuts off — tune these independently to balance look vs. cost.
const RANGE_EDGE: f32 = 200.0;
const RANGE_THRESHOLD: f32 = 80.0;
const RANGE_REIL: f32 = 3000.0;
const RANGE_ALS: f32 = 200.0;

/// Spawns one runway strip. `stalk_centre` is `Some((x, elev, z))` for the first
/// strip of each airport — that strip also spawns the vertical 3D stalk mesh so
/// the label can occlude behind terrain. Secondary strips pass `None`. Never
/// spawns lights itself — see `stream_runway_lights`.
#[allow(clippy::too_many_arguments)]
fn spawn_runway(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &RunwayMaterials,
    inst: &RunwayInstance,
    cell: (i32, i32),
    stalk_centre: Option<(f32, f32, f32)>,
    seed: u32,
) {
    const PAINT_Y: f32 = 0.1;
    const THICKNESS: f32 = 3.0;
    let surface_y = RUNWAY_SURFACE_LIFT;
    let w = inst.width;
    let l = inst.length;
    let half_len = l * 0.5;
    let is_dirt = inst.kind == AirportKind::DirtStrip;

    let root = Transform::from_xyz(inst.x, inst.elevation, inst.z)
        .with_rotation(Quat::from_rotation_y(inst.heading));
    let slab = RunwaySlab { cell, pos: Vec2::new(inst.x, inst.z), elevation: inst.elevation, width: w, length: l, is_lit: !is_dirt };

    commands
        .spawn((root, Visibility::default(), slab, PIXEL_LAYER))
        .with_children(|parent| {
            let surface_mat = if is_dirt { materials.dirt.clone() } else { materials.asphalt.clone() };

            parent.spawn((
                Mesh3d(meshes.add(Cuboid::new(w, THICKNESS, l))),
                MeshMaterial3d(surface_mat),
                Transform::from_xyz(0.0, surface_y - THICKNESS * 0.5, 0.0),
                PIXEL_LAYER,
            ));

            if is_dirt {
                // Dirt strips: no paint markings, no lights. Done.
                return;
            }

            // Dashed centreline. Every dash shares one mesh asset
            // (`materials.dash_mesh`) instead of each getting its own —
            // they're all the same DASH_W×DASH_LEN size, only Transform differs.
            const GAP_LEN: f32 = 20.0;
            let stride = DASH_LEN + GAP_LEN;
            let count = (l / stride) as i32;
            let start = -(count as f32 - 1.0) * stride * 0.5;
            for i in 0..count {
                let z = start + i as f32 * stride;
                parent.spawn((
                    Mesh3d(materials.dash_mesh.clone()),
                    MeshMaterial3d(materials.paint.clone()),
                    Transform::from_xyz(0.0, surface_y + PAINT_Y, z),
                    PIXEL_LAYER,
                ));
            }

            // Threshold "piano key" bars, sharing one mesh asset
            // (`materials.bar_mesh`) the same way. `NoFrustumCulling`:
            // zero-thickness paint planes have no AABB height so Bevy wrongly
            // culls them at shallow angles.
            const BAR_GAP: f32 = 2.0;
            for end in [-1.0_f32, 1.0] {
                let z = end * (half_len - BAR_LEN * 0.5 - 5.0);
                let bar_stride = BAR_W + BAR_GAP;
                for k in -4..=4_i32 {
                    if k == 0 { continue; }
                    parent.spawn((
                        Mesh3d(materials.bar_mesh.clone()),
                        MeshMaterial3d(materials.paint.clone()),
                        Transform::from_xyz(k as f32 * bar_stride, surface_y + PAINT_Y, z),
                        NoFrustumCulling,
                        PIXEL_LAYER,
                    ));
                }
            }

            // Lights are never spawned here — `stream_runway_lights` (chained
            // right after `stream_runways` in the same Update pass, so its
            // Commands see this same-frame spawn) is the single place that
            // decides whether a runway's lights should exist, based on
            // distance, RunwayLightsEnabled, and day/night. Keeping that
            // logic in one place means "should this runway be lit right now"
            // never has two different answers depending on which system last
            // touched it.
        });

    // One low-poly cylinder stalk per airport, spawned only for the primary strip.
    // Not a child of the runway root so it stays upright regardless of heading.
    if let Some((sx, se, sz)) = stalk_centre {
        let stalk_h = STALK_TIP_OFFSET - STALK_BASE_OFFSET;
        let mid_y = se + STALK_BASE_OFFSET + stalk_h * 0.5;
        commands.spawn((
            Mesh3d(meshes.add(Cylinder::new(STALK_RADIUS, stalk_h).mesh().resolution(6).build())),
            MeshMaterial3d(materials.stalk.clone()),
            Transform::from_xyz(sx, mid_y, sz),
            PIXEL_LAYER,
            WaypointStalk { cell, kind: inst.kind, name: airport_name(seed, cell, inst.kind) },
            NoFrustumCulling,
        ));
    }
}

/// Spawns one runway's full light set (edge, threshold, REIL, ALS) under a
/// `RunwayLights`-tagged child group, so `stream_runway_lights` can despawn
/// the whole group in one shot when the camera moves out of
/// `LIGHTS_MAX_VIEW_M`.
fn spawn_runway_lights(parent: &mut ChildSpawnerCommands, w: f32, l: f32, half_len: f32, surface_y: f32) {
    let light_y = surface_y + LIGHT_Y;
    let half_w = w * 0.5 - EDGE_LIGHT_INSET;
    let n_edge = (l / EDGE_LIGHT_SPACING) as i32 + 1;
    let edge_start = -half_len;
    let threshold_z = half_len - 2.0;
    let white = Color::srgb(1.0, 0.97, 0.88);
    let yellow = Color::srgb(1.0, 0.85, 0.1);
    let caution_start = half_len - 610.0;

    parent.spawn((
        Transform::IDENTITY,
        Visibility::default(),
        RunwayLights,
        PIXEL_LAYER,
    )).with_children(|group| {
        for i in 0..=n_edge {
            let z = edge_start + i as f32 * EDGE_LIGHT_SPACING;
            if z > half_len { break; }
            let color = if z >= caution_start { yellow } else { white };
            for side in [-1.0_f32, 1.0] {
                group.spawn((
                    PointLight {
                        color,
                        intensity: 500_000.0,
                        range: RANGE_EDGE,
                        radius: 0.3,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(side * half_w, light_y, z),
                    PIXEL_LAYER,
                ));
            }
        }

        for end_sign in [-1.0_f32, 1.0] {
            let tz = end_sign * threshold_z;
            group.spawn((
                PointLight {
                    color: Color::srgb(0.1, 1.0, 0.2),
                    intensity: 3_200_000.0,
                    range: RANGE_THRESHOLD,
                    radius: w * 0.4,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, light_y, tz - end_sign * 1.0),
                PIXEL_LAYER,
            ));
            group.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.08, 0.05),
                    intensity: 3_200_000.0,
                    range: RANGE_THRESHOLD,
                    radius: w * 0.4,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.0, light_y, tz + end_sign * 1.0),
                PIXEL_LAYER,
            ));

            for side in [-1.0_f32, 1.0] {
                group.spawn((
                    PointLight {
                        color: Color::srgb(1.0, 0.15, 0.1),
                        intensity: 5_000_000.0,
                        range: RANGE_REIL,
                        radius: 1.5,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(side * (w * 0.5 + 5.0), light_y + 0.5, tz),
                    PIXEL_LAYER,
                    ReilLight,
                ));
            }

            const ALS_BARS: i32 = 3;
            const ALS_SPACING: f32 = 60.0;
            for j in 1..=ALS_BARS {
                let z = tz + end_sign * j as f32 * ALS_SPACING;
                group.spawn((
                    PointLight {
                        color: Color::srgb(1.0, 0.97, 0.88),
                        intensity: 2_000_000.0,
                        range: RANGE_ALS,
                        radius: 9.0,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::from_xyz(0.0, light_y + 1.5, z),
                    PIXEL_LAYER,
                    AlsLight { index: j },
                ));
            }
        }
    });
}

/// Marks the 3D stalk cylinder for a waypoint pin. `name` is precomputed at
/// spawn time (deterministic from seed/cell/kind) so the label UI doesn't
/// re-hash and re-format it every frame it's on screen.
#[derive(Component)]
pub struct WaypointStalk {
    pub cell: (i32, i32),
    pub kind: AirportKind,
    pub name: String,
}

/// Scales each stalk's X/Z to keep a constant apparent radius on screen.
/// Y (height) is never scaled so the stalk stays 1 km tall in world space.
pub fn scale_waypoint_stalks(
    camera: Query<&Transform, With<TerrainCamera>>,
    mut stalks: Query<&mut Transform, (With<WaypointStalk>, Without<TerrainCamera>)>,
) {
    let Ok(cam) = camera.single() else { return };
    let cam_pos = cam.translation;
    for mut tf in &mut stalks {
        let dist = tf.translation.distance(cam_pos).max(1.0);
        let s = dist / 10_000.0; // radius = STALK_RADIUS at 10 km
        tf.scale = Vec3::new(s, 1.0, s);
    }
}

/// Animates REIL strobes and ALS sequencing bars every frame.
///
/// - REIL: flashes at ~1 Hz (50 ms on, 950 ms off).
/// - ALS: one bar is lit at a time, stepping toward the threshold at `ALS_HZ`,
///   giving the classic "rabbit" effect pilots use on final approach.
pub fn animate_runway_lights(
    time: Res<Time>,
    mut clock: ResMut<RunwayLightClock>,
    mut reil: Query<&mut PointLight, (With<ReilLight>, Without<AlsLight>)>,
    mut als: Query<(&AlsLight, &mut PointLight), Without<ReilLight>>,
) {
    clock.elapsed += time.delta_secs();
    let t = clock.elapsed;

    // REIL: 1 Hz flash — on for 50 ms at the top of each second.
    let reil_on = (t % 1.0) < 0.05;
    for mut light in &mut reil {
        light.intensity = if reil_on { 2_000_000.0 } else { 0.0 };
    }

    // ALS rabbit: cycle through bars farthest→nearest at ALS_HZ.
    // `active` is which bar index (1=nearest threshold, 3=farthest) is lit.
    const ALS_BARS: i32 = 3;
    const ALS_HZ: f32 = 2.0;
    // Step 0 = bar 3 (farthest), step 2 = bar 1 (nearest threshold).
    let step = ((t * ALS_HZ) as i32).rem_euclid(ALS_BARS);
    let active_index = ALS_BARS - step; // counts down 3→2→1→3→…
    for (als, mut light) in &mut als {
        light.intensity = if als.index == active_index { 2_000_000.0 } else { 0.0 };
    }
}
