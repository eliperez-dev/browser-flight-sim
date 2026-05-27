//! CPU baking of the map's background layers into a texture, and the egui
//! overlay drawing (airports, plane, camera). Kept separate from the plugin
//! wiring in [`super`] so the "what the map looks like" logic stays in one place.
//!
//! The background is sampled from [`WorldGenerator`] once per *view change* (see
//! [`super::MapState`]'s dirty check) — never per frame — so the only steady-state
//! cost is the handful of painter calls for the overlays.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::terrain::{RunwayInstance, WorldGenerator};

use super::{MapLayer, MapState};

/// Background texture resolution (square). Modest on purpose: each texel is a
/// full multi-octave noise sample, and the bake only reruns when the view moves
/// far enough to matter, so 256² keeps even WASM rebuilds well under a frame.
pub const TEX: u32 = 256;

/// Half the world-space extent the texture currently covers, in metres: the map
/// spans `center ± half_span` on both axes.
pub fn half_span(state: &MapState) -> f32 {
    TEX as f32 * state.world_per_texel * 0.5
}

// --- Background baking --------------------------------------------------------

/// Refills the background `Image` from the generator for the current
/// `baked_center` / `world_per_texel` / `layer`. The caller decides *when* to
/// call this (only on a real view change); this just does the fill.
pub fn bake(image: &mut Image, generator: &WorldGenerator, state: &MapState) {
    let Some(data) = image.data.as_mut() else { return };
    let half = half_span(state);
    let origin = state.center - Vec2::splat(half);
    let wpt = state.world_per_texel;

    for ty in 0..TEX {
        for tx in 0..TEX {
            // Sample at the texel centre. World +X → texture +X (east right),
            // world +Z → texture +Y (south down) — the convention the overlay
            // mapping in `world_to_screen` also uses.
            let wx = origin.x + (tx as f32 + 0.5) * wpt;
            let wz = origin.y + (ty as f32 + 0.5) * wpt;
            let rgba = match state.layer {
                MapLayer::Biome => biome_texel(generator, wx, wz),
                MapLayer::Height => height_texel(generator, wx, wz),
            };
            let i = ((ty * TEX + tx) * 4) as usize;
            data[i..i + 4].copy_from_slice(&rgba);
        }
    }
}

/// One biome-layer texel: the terrain's own surface colour, so the map reads like
/// a shrunk-down view of what you fly over. `sample_natural` returns *linear*
/// RGBA (it feeds vertex colours), so convert to sRGB for the sRGB texture.
fn biome_texel(generator: &WorldGenerator, x: f32, z: f32) -> [u8; 4] {
    let (_h, lin) = generator.sample_natural(x, z);
    let srgb = Color::from(LinearRgba::new(lin[0], lin[1], lin[2], lin[3])).to_srgba();
    [
        to_u8(srgb.red),
        to_u8(srgb.green),
        to_u8(srgb.blue),
        255,
    ]
}

/// One height-layer texel: a topographic ramp. Below sea level fades to deep
/// blue; land ramps dark→bright with elevation. These are already display
/// (sRGB) values, written straight into the sRGB texture.
fn height_texel(generator: &WorldGenerator, x: f32, z: f32) -> [u8; 4] {
    let h = generator.natural_height(x, z);
    if h < 0.0 {
        // Deeper water → darker. 300 m maps roughly the visible basin depth.
        let d = (-h / 300.0).clamp(0.0, 1.0);
        [
            to_u8(0.05 + 0.10 * (1.0 - d)),
            to_u8(0.15 + 0.25 * (1.0 - d)),
            to_u8(0.30 + 0.35 * (1.0 - d)),
            255,
        ]
    } else {
        // Land: greyscale from valley floor to peak over ~450 m of relief.
        let t = (h / 450.0).clamp(0.0, 1.0);
        let g = to_u8(0.15 + 0.80 * t);
        [g, g, g, 255]
    }
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

// --- Overlay drawing ----------------------------------------------------------

/// Maps a world-space (x, z) point into screen pixels inside the map widget
/// `rect`, for the current view. World +X is right, world +Z is down.
pub fn world_to_screen(world: Vec2, rect: egui::Rect, center: Vec2, half: f32) -> egui::Pos2 {
    let nx = (world.x - (center.x - half)) / (2.0 * half);
    let nz = (world.y - (center.y - half)) / (2.0 * half);
    egui::pos2(
        rect.left() + nx * rect.width(),
        rect.top() + nz * rect.height(),
    )
}

/// Draws every nearby runway as a short line oriented like the strip, plus a dot
/// at its centre. `runways` is already filtered to the visible region.
pub fn draw_airports(
    painter: &egui::Painter,
    runways: &[RunwayInstance],
    rect: egui::Rect,
    center: Vec2,
    half: f32,
) {
    let color = egui::Color32::from_rgb(255, 90, 220);
    let stroke = egui::Stroke::new(2.0, color);
    for r in runways {
        // Strip length runs along the runway's local +Z, rotated by `heading`
        // about Y — matching how `spawn_runway` orients the slab. That sends the
        // local +Z axis to world (sin θ, cos θ) in the (x, z) plane.
        let (s, c) = r.heading.sin_cos();
        let dir = Vec2::new(s, c) * (crate::terrain::RUNWAY_LENGTH * 0.5);
        let pos = Vec2::new(r.x, r.z);
        let a = world_to_screen(pos - dir, rect, center, half);
        let b = world_to_screen(pos + dir, rect, center, half);
        painter.line_segment([a, b], stroke);
        painter.circle_filled(world_to_screen(pos, rect, center, half), 2.0, color);
    }
}

/// Draws the player aircraft as an oriented rectangle (a debug placeholder) with
/// a short nose line showing heading. `forward` is the plane's world-space facing.
pub fn draw_plane(
    painter: &egui::Painter,
    pos: Vec2,
    forward: Vec2,
    rect: egui::Rect,
    center: Vec2,
    half: f32,
) {
    let c = world_to_screen(pos, rect, center, half);
    // Screen-space facing: world +X→right, +Z→down, so the world (x, z) heading
    // maps directly onto the screen axes.
    let f = to_egui(forward.normalize_or_zero());
    let side = egui::vec2(-f.y, f.x);
    let (hl, hw) = (7.0, 4.0); // half length / half width in pixels
    let corners = vec![
        c + f * hl + side * hw,
        c + f * hl - side * hw,
        c - f * hl - side * hw,
        c - f * hl + side * hw,
    ];
    let fill = egui::Color32::from_rgb(255, 230, 60);
    painter.add(egui::Shape::convex_polygon(
        corners,
        fill,
        egui::Stroke::new(1.0, egui::Color32::BLACK),
    ));
    // Nose whisker so the heading is unambiguous even at small sizes.
    painter.line_segment([c, c + f * (hl + 6.0)], egui::Stroke::new(2.0, fill));
}

/// Draws the terrain camera as a small triangle pointing along its facing, with a
/// thin field-of-view wedge.
pub fn draw_camera(
    painter: &egui::Painter,
    pos: Vec2,
    forward: Vec2,
    rect: egui::Rect,
    center: Vec2,
    half: f32,
) {
    let c = world_to_screen(pos, rect, center, half);
    let f = to_egui(forward.normalize_or_zero());
    let side = egui::vec2(-f.y, f.x);
    let color = egui::Color32::from_rgb(120, 220, 255);
    // Body triangle.
    let tip = c + f * 8.0;
    let tri = vec![tip, c - f * 4.0 + side * 5.0, c - f * 4.0 - side * 5.0];
    painter.add(egui::Shape::convex_polygon(
        tri,
        color.gamma_multiply(0.6),
        egui::Stroke::new(1.0, color),
    ));
    // FOV wedge.
    let wedge = egui::Stroke::new(1.0, color.gamma_multiply(0.8));
    let l = 22.0;
    let spread = 0.5;
    let left = f * l + side * (l * spread);
    let right = f * l - side * (l * spread);
    painter.line_segment([c, c + left], wedge);
    painter.line_segment([c, c + right], wedge);
}

fn to_egui(v: Vec2) -> egui::Vec2 {
    egui::vec2(v.x, v.y)
}
