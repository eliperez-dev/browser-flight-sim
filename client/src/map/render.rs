//! CPU baking of the map's background layers into a texture, and the egui
//! overlay drawing (airports, breadcrumbs, waypoint, plane, camera, decorations).
//! Kept separate from the plugin wiring in [`super`] so the "what the map looks
//! like" logic stays in one place.
//!
//! The background is sampled from [`WorldGenerator`] once per *view change* (see
//! [`super::MapState`]'s dirty check) — never per frame — so the only steady-state
//! cost is the handful of painter calls for the overlays.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::terrain::{Airport, AirportKind, Biome, WorldGenerator, runway_ident};

use super::{Breadcrumb, MapIconSettings, MapLayer};

/// Background texture resolution (square). Modest on purpose: each texel is a
/// full multi-octave noise sample, and the bake only reruns when the view moves
/// far enough to matter, so 256² keeps even WASM rebuilds well under a frame.
pub const TEX: u32 = 256;

/// Below this world-metres-per-texel zoom, airport idents are drawn — at coarser
/// zooms there are too many strips on screen for labels to be readable. Raised
/// well past the old 160 so idents stay legible further into the zoomed-out
/// range, now that pins have a pixel-floor size and don't vanish out there.
const IDENT_ZOOM_LIMIT: f32 = 500.0;

/// Half the world-space extent the texture currently covers, in metres: the map
/// spans `center ± half_span` on both axes.
pub fn half_span(world_per_texel: f32) -> f32 {
    TEX as f32 * world_per_texel * 0.5
}

/// The current map viewport: the screen rect plus the world window it shows.
/// Bundles the world↔screen mapping so every overlay shares one source of truth.
#[derive(Clone, Copy)]
pub struct View {
    pub rect: egui::Rect,
    pub center: Vec2,
    pub half: f32,
}

impl View {
    /// World (x, z) → screen pixels. World +X is right, world +Z is down.
    pub fn to_screen(self, world: Vec2) -> egui::Pos2 {
        let nx = (world.x - (self.center.x - self.half)) / (2.0 * self.half);
        let nz = (world.y - (self.center.y - self.half)) / (2.0 * self.half);
        egui::pos2(
            self.rect.left() + nx * self.rect.width(),
            self.rect.top() + nz * self.rect.height(),
        )
    }

    /// Screen pixels → world (x, z). Inverse of [`Self::to_screen`], for the
    /// hover readout.
    pub fn to_world(self, screen: egui::Pos2) -> Vec2 {
        let nx = (screen.x - self.rect.left()) / self.rect.width();
        let nz = (screen.y - self.rect.top()) / self.rect.height();
        Vec2::new(
            self.center.x - self.half + nx * 2.0 * self.half,
            self.center.y - self.half + nz * 2.0 * self.half,
        )
    }

    /// World metres covered by one screen pixel (for the ruler / pan maths).
    pub fn world_per_px(&self) -> f32 {
        2.0 * self.half / self.rect.width()
    }
}

// --- Background baking --------------------------------------------------------

/// Bakes a horizontal slice of the background texture (`row_start..row_end`).
/// Splitting the full 256×256 bake into small row batches spreads the noise
/// cost over multiple frames so no single frame stalls.
#[allow(clippy::too_many_arguments)]
pub fn bake_rows(
    image: &mut Image,
    generator: &WorldGenerator,
    row_start: u32,
    row_end: u32,
    center: Vec2,
    world_per_texel: f32,
    layer: MapLayer,
    sea_level: f32,
) {
    let Some(data) = image.data.as_mut() else { return };
    let half = TEX as f32 * world_per_texel * 0.5;
    let origin = center - Vec2::splat(half);
    let wpt = world_per_texel;

    for ty in row_start..row_end {
        for tx in 0..TEX {
            let wx = origin.x + (tx as f32 + 0.5) * wpt;
            let wz = origin.y + (ty as f32 + 0.5) * wpt;
            let rgba = match layer {
                MapLayer::Biome => biome_texel(generator, wx, wz, sea_level),
                MapLayer::Height => height_texel(generator, wx, wz, sea_level),
                MapLayer::BiomeCategory => biome_category_texel(generator, wx, wz, sea_level),
            };
            let i = ((ty * TEX + tx) * 4) as usize;
            data[i..i + 4].copy_from_slice(&rgba);
        }
    }
}

/// The 3D world's water colour (linear RGB, from `WaterSettings::default`), used
/// to paint anything below sea level on the map instead of the dry seabed.
const WATER_LINEAR: [f32; 3] = [0.04, 0.18, 0.32];

/// One biome-layer texel: the terrain's own surface colour, so the map reads like
/// a shrunk-down view of what you fly over. `sample_natural` returns *linear*
/// RGBA (it feeds vertex colours), so convert to sRGB for the sRGB texture.
/// Anything below `sea_level` is painted water instead of the seabed the noise
/// would otherwise show, matching the 3D water plane.
fn biome_texel(generator: &WorldGenerator, x: f32, z: f32, sea_level: f32) -> [u8; 4] {
    let (h, lin) = generator.sample_natural(x, z);
    let lin = if h < sea_level {
        // Deeper water → darker, so coastlines and basins still read.
        let shade = 1.0 - (sea_level - h).clamp(0.0, 0.6);
        LinearRgba::new(
            WATER_LINEAR[0] * shade,
            WATER_LINEAR[1] * shade,
            WATER_LINEAR[2] * shade,
            1.0,
        )
    } else {
        LinearRgba::new(lin[0], lin[1], lin[2], lin[3])
    };
    let srgb = Color::from(lin).to_srgba();
    [to_u8(srgb.red), to_u8(srgb.green), to_u8(srgb.blue), 255]
}

/// One height-layer texel: a topographic ramp. Below `sea_level` fades to deep
/// blue; land ramps dark→bright with elevation above it. These are already
/// display (sRGB) values, written straight into the sRGB texture.
fn height_texel(generator: &WorldGenerator, x: f32, z: f32, sea_level: f32) -> [u8; 4] {
    let h = generator.natural_height(x, z);
    if h < sea_level {
        let d = ((sea_level - h) / 300.0).clamp(0.0, 1.0);
        [
            to_u8(0.05 + 0.10 * (1.0 - d)),
            to_u8(0.15 + 0.25 * (1.0 - d)),
            to_u8(0.30 + 0.35 * (1.0 - d)),
            255,
        ]
    } else {
        let t = ((h - sea_level) / 450.0).clamp(0.0, 1.0);
        let g = to_u8(0.15 + 0.80 * t);
        [g, g, g, 255]
    }
}

/// One biome-category texel: a flat colour per biome (no height shading), the
/// clearest read for "what kind of terrain is this". Below `sea_level`, or where
/// the generator calls it ocean, paints water.
fn biome_category_texel(generator: &WorldGenerator, x: f32, z: f32, sea_level: f32) -> [u8; 4] {
    let h = generator.natural_height(x, z);
    let biome = generator.get_biome(x, z);
    if h < sea_level || biome == Biome::Ocean {
        let srgb = Color::from(LinearRgba::new(WATER_LINEAR[0], WATER_LINEAR[1], WATER_LINEAR[2], 1.0))
            .to_srgba();
        return [to_u8(srgb.red), to_u8(srgb.green), to_u8(srgb.blue), 255];
    }
    biome_flat_color(biome)
}

/// Flat display (sRGB) colour for each land biome, written straight to the texture.
fn biome_flat_color(biome: Biome) -> [u8; 4] {
    let rgb = match biome {
        Biome::Desert => [0.85, 0.74, 0.45],
        Biome::Grasslands => [0.49, 0.70, 0.34],
        Biome::Forest => [0.16, 0.52, 0.20],
        Biome::Taiga => [0.36, 0.50, 0.44],
        Biome::Ocean => [WATER_LINEAR[0], WATER_LINEAR[1], WATER_LINEAR[2]],
    };
    [to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), 255]
}

/// Display name for a biome, for the hover readout.
pub fn biome_name(biome: Biome) -> &'static str {
    match biome {
        Biome::Desert => "Desert",
        Biome::Grasslands => "Grasslands",
        Biome::Forest => "Forest",
        Biome::Taiga => "Taiga",
        Biome::Ocean => "Ocean",
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

// --- Overlay drawing ----------------------------------------------------------

const PLANE: egui::Color32 = egui::Color32::from_rgb(255, 230, 60);
const CAMERA: egui::Color32 = egui::Color32::from_rgb(120, 220, 255);
const WAYPOINT: egui::Color32 = egui::Color32::from_rgb(255, 140, 40);

/// Fill colour per [`AirportKind`], so airport class reads at a glance without
/// relying on dot size (which stays uniform — see [`draw_airports`]).
pub fn airport_kind_color(kind: AirportKind) -> egui::Color32 {
    match kind {
        AirportKind::DirtStrip => egui::Color32::from_rgb(150, 110, 70),
        AirportKind::SmallGA => egui::Color32::from_rgb(255, 90, 220),
        AirportKind::LargeCommuter => egui::Color32::from_rgb(255, 150, 60),
        AirportKind::Regional => egui::Color32::from_rgb(140, 170, 255),
        AirportKind::Hub => egui::Color32::from_rgb(90, 255, 140),
    }
}

/// On-screen radius (px) of an airport pin at the current zoom: scales with
/// world-space size like the runway lines do when zoomed in, floored so it
/// never vanishes zoomed out, and capped in *screen* space so a close-in zoom
/// can't balloon the dot into covering its own runway strips. Same for every
/// airport (colour carries the class, not size) — shared by [`draw_airports`]
/// and the map's click hit-test so "what you see" and "what you can click"
/// never drift apart.
pub fn airport_dot_radius(icons: &MapIconSettings, view: &View) -> f32 {
    const DOT_WORLD_DIAMETER: f32 = 1000.0;
    const DOT_SCREEN_RADIUS_MAX: f32 = 16.0;
    let world_radius = DOT_WORLD_DIAMETER * 0.5 / view.world_per_px();
    world_radius.max(icons.airport_circle).min(DOT_SCREEN_RADIUS_MAX)
}

/// Draws one map pin per airport (one per cell), with every strip drawn at its
/// own heading and to-scale length — so non-parallel runways render correctly.
/// The pin's colour encodes [`AirportKind`] (dirt strip → hub); every dot is
/// the same size so colour alone carries the class, not size.
pub fn draw_airports(
    painter: &egui::Painter,
    airports: &[Airport],
    seed: u32,
    selected: Option<(i32, i32)>,
    show_idents: bool,
    icons: &MapIconSettings,
    view: &View,
) {
    let dot_radius = airport_dot_radius(icons, view);

    for ap in airports {
        let (ax, az) = ap.pos();
        let center = view.to_screen(Vec2::new(ax, az));
        let color = airport_kind_color(ap.kind);

        // Outline makes small/dim kinds (dirt strips) still read against
        // similarly-coloured terrain, and gives every pin a crisp edge.
        painter.circle(
            center,
            dot_radius,
            color,
            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(200)),
        );

        // Draw each strip's own line on top, each using its own heading/length,
        // once they're big enough on screen to be worth drawing.
        for strip in &ap.strips {
            let sc = view.to_screen(Vec2::new(strip.x, strip.z));
            let (s, c) = strip.heading.sin_cos();
            let hdir = egui::vec2(s, c);
            let strip_screen_len = strip.length / view.world_per_px();
            let half = hdir * (strip_screen_len * 0.5);
            let stroke_width = (strip.width / view.world_per_px()).max(icons.runway_width * 0.4);
            let stroke = egui::Stroke::new(stroke_width, egui::Color32::WHITE);
            painter.line_segment([sc - half, sc + half], stroke);
        }

        if selected == Some(ap.cell) {
            painter.circle_stroke(
                center,
                icons.selected_ring,
                egui::Stroke::new(2.0, egui::Color32::WHITE),
            );
        }
        if show_idents {
            // Fixed clearance beyond the dot, not just a few px past its edge —
            // at coarse zoom the dot shrinks to its pixel floor but the label
            // should still sit clearly outside it, not crowd the marker.
            painter.text(
                center + egui::vec2(dot_radius + 8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{}  {}", runway_ident(seed, ap.cell), ap.kind.display_name()),
                egui::FontId::proportional(icons.label_font),
                egui::Color32::WHITE,
            );
        }
    }
}

/// Draws the breadcrumb trail: a short heading-aligned dash per logged sample,
/// fading from faint (oldest, front of the deque) to brighter (newest).
pub fn draw_breadcrumbs(
    painter: &egui::Painter,
    crumbs: &VecDeque<Breadcrumb>,
    icons: &MapIconSettings,
    view: &View,
) {
    let n = crumbs.len().max(1) as f32;
    let len = icons.breadcrumb_len;
    for (i, c) in crumbs.iter().enumerate() {
        let p = view.to_screen(c.pos);
        let d = to_egui(c.heading.normalize_or_zero());
        // Older → more transparent so the trail reads as a direction over time.
        let alpha = (40.0 + 150.0 * (i as f32 / n)) as u8;
        let color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
        painter.line_segment([p - d * len, p + d * len], egui::Stroke::new(2.0, color));
    }
}

/// Draws the active "direct-to" course: a dashed line from the plane to the
/// destination, a ring + ident at the destination, and the great-circle-free
/// straight-line distance at the midpoint.
pub fn draw_waypoint(
    painter: &egui::Painter,
    plane: Vec2,
    dest: Vec2,
    ident: &str,
    icons: &MapIconSettings,
    view: &View,
) {
    let a = view.to_screen(plane);
    let b = view.to_screen(dest);
    for shape in egui::Shape::dashed_line(&[a, b], egui::Stroke::new(2.0, WAYPOINT), 8.0, 5.0) {
        painter.add(shape);
    }
    painter.circle_stroke(b, icons.waypoint_marker, egui::Stroke::new(2.0, WAYPOINT));
    let km = plane.distance(dest) / 1000.0;
    let mid = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    painter.text(
        mid + egui::vec2(0.0, -8.0),
        egui::Align2::CENTER_BOTTOM,
        format!("→ {ident}  {km:.1} km"),
        egui::FontId::proportional(icons.label_font),
        WAYPOINT,
    );
}

/// Draws the player aircraft as an oriented rectangle (debug placeholder) with a
/// short nose line showing heading. `forward` is the plane's world-space facing.
pub fn draw_plane(painter: &egui::Painter, pos: Vec2, forward: Vec2, icons: &MapIconSettings, view: &View) {
    let c = view.to_screen(pos);
    let f = to_egui(forward.normalize_or_zero());
    let side = egui::vec2(-f.y, f.x);
    let (hl, hw) = (icons.plane_len, icons.plane_width); // half length / half width (px)
    let corners = vec![
        c + f * hl + side * hw,
        c + f * hl - side * hw,
        c - f * hl - side * hw,
        c - f * hl + side * hw,
    ];
    painter.add(egui::Shape::convex_polygon(
        corners,
        PLANE,
        egui::Stroke::new(1.0, egui::Color32::BLACK),
    ));
    painter.line_segment([c, c + f * (hl + 6.0)], egui::Stroke::new(2.0, PLANE));
}

/// Draws the terrain camera as a small triangle pointing along its facing, with a
/// thin field-of-view wedge.
pub fn draw_camera(painter: &egui::Painter, pos: Vec2, forward: Vec2, icons: &MapIconSettings, view: &View) {
    let c = view.to_screen(pos);
    let f = to_egui(forward.normalize_or_zero());
    let side = egui::vec2(-f.y, f.x);
    // Everything scales from one size knob (defaults reproduce the original
    // tip 8 / base 4 / side 5 / wedge 22 proportions).
    let s = icons.camera_size;
    let tip = c + f * s;
    let tri = vec![tip, c - f * (s * 0.5) + side * (s * 0.625), c - f * (s * 0.5) - side * (s * 0.625)];
    painter.add(egui::Shape::convex_polygon(
        tri,
        CAMERA.gamma_multiply(0.6),
        egui::Stroke::new(1.0, CAMERA),
    ));
    let wedge = egui::Stroke::new(1.0, CAMERA.gamma_multiply(0.8));
    let (l, spread) = (s * 2.75, 0.5);
    painter.line_segment([c, c + f * l + side * (l * spread)], wedge);
    painter.line_segment([c, c + f * l - side * (l * spread)], wedge);
}

/// Scale bar in the bottom-left: a "nice" round world length (1/2/5 × 10ⁿ) drawn
/// as a bar with its length labelled, recomputed from the current zoom.
pub fn draw_ruler(painter: &egui::Painter, view: &View) {
    let (world_len, label) = nice_length(view.world_per_px() * 80.0);
    let px = world_len / view.world_per_px();
    let y = view.rect.bottom() - 12.0;
    let x0 = view.rect.left() + 10.0;
    let x1 = x0 + px;
    let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
    painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke);
    painter.line_segment([egui::pos2(x0, y - 10.0), egui::pos2(x0, y + 10.0)], stroke);
    painter.line_segment([egui::pos2(x1, y - 10.0), egui::pos2(x1, y + 10.0)], stroke);
    painter.text(
        egui::pos2(x0, y - 4.0),
        egui::Align2::LEFT_BOTTOM,
        label,
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
}

/// Rounds `raw` metres down to the nearest 1/2/5 × 10ⁿ and labels it (m or km).
fn nice_length(raw: f32) -> (f32, String) {
    let mag = 10f32.powf(raw.max(1.0).log10().floor());
    let n = raw / mag;
    let nice = if n < 1.5 {
        1.0
    } else if n < 3.5 {
        2.0
    } else if n < 7.5 {
        5.0
    } else {
        10.0
    };
    let len = nice * mag;
    let label = if len >= 1000.0 {
        format!("{:.0} km", len / 1000.0)
    } else {
        format!("{len:.0} m")
    };
    (len, label)
}

/// Draws a small readout box near the cursor: world coords, ground elevation,
/// and biome under the pointer. `anchor` is the cursor position.
pub fn draw_hover(
    painter: &egui::Painter,
    anchor: egui::Pos2,
    world: Vec2,
    elevation: f32,
    biome: &str,
    rect: egui::Rect,
) {
    let text = format!(
        "{:.0}, {:.0}\n{elevation:.0} m · {biome}",
        world.x, world.y
    );
    let font = egui::FontId::proportional(11.0);
    // Lay the text out so we can size the background and keep it on-map.
    let galley = painter.layout_no_wrap(text, font, egui::Color32::WHITE);
    let pad = egui::vec2(5.0, 3.0);
    let size = galley.size() + pad * 2.0;
    // Offset up-right of the cursor, then clamp to stay inside the map rect.
    let mut min = anchor + egui::vec2(12.0, -size.y - 6.0);
    min.x = min.x.clamp(rect.left(), rect.right() - size.x);
    min.y = min.y.clamp(rect.top(), rect.bottom() - size.y);
    let bg = egui::Rect::from_min_size(min, size);
    painter.rect_filled(bg, 3.0, egui::Color32::from_black_alpha(180));
    painter.galley(min + pad, galley, egui::Color32::WHITE);
}

/// Whether idents should be drawn at the given zoom (avoids label clutter when
/// zoomed out).
pub fn idents_visible(world_per_texel: f32) -> bool {
    world_per_texel <= IDENT_ZOOM_LIMIT
}

fn to_egui(v: Vec2) -> egui::Vec2 {
    egui::vec2(v.x, v.y)
}
