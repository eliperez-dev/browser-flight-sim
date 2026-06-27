//! In-world waypoint labels for loaded runways.
//!
//! Each runway gets a vertical stalk (ground → 400 m above) projected into
//! screen space, with bare text at the top — no background box. Labels are
//! hidden when closer than 2 km (never plastered over your own aircraft) and
//! fade out beyond 30 km.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::{OuterCamera, PIXEL_HEIGHT, PIXEL_WIDTH};
use crate::plane::Airplane;
use crate::terrain::{RunwaySlab, WorldGenerator, runway_ident};
use crate::terrain::TerrainCamera;

/// Height above the runway elevation for the stalk base (sits just above the asphalt).
const STALK_BASE_OFFSET: f32 = 2.0;
/// Height above the runway elevation for the stalk tip and label (1 km up).
const STALK_TIP_OFFSET: f32 = 1000.0;

/// Hide labels closer than this (they'd overlap the cockpit view).
const MIN_DIST_KM: f32 = 2.0;
/// Begin fading at this distance; invisible by FAR_DIST_KM.
const FADE_START_KM: f32 = 30.0;
const FAR_DIST_KM: f32 = 120.0;

pub struct WaypointsPlugin;

impl Plugin for WaypointsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_waypoint_labels);
    }
}

pub fn draw_waypoint_labels(
    mut contexts: EguiContexts,
    generator: Res<WorldGenerator>,
    slabs: Query<&RunwaySlab>,
    plane_q: Query<&Transform, With<Airplane>>,
    inner_cam_q: Query<(&Camera, &GlobalTransform), With<TerrainCamera>>,
    outer_proj_q: Query<&Projection, With<OuterCamera>>,
    windows: Query<&Window>,
) -> Result {
    let Ok(plane_tf) = plane_q.single() else { return Ok(()) };
    let plane_pos = plane_tf.translation;
    let Ok((inner_cam, inner_gtf)) = inner_cam_q.single() else { return Ok(()) };

    let canvas_scale = outer_proj_q.single().ok().and_then(|p| {
        if let Projection::Orthographic(o) = p { Some(1.0 / o.scale) } else { None }
    }).unwrap_or(1.0);

    let window = windows.single().ok();
    let win_w = window.map(|w| w.width()).unwrap_or(640.0);
    let win_h = window.map(|w| w.height()).unwrap_or(360.0);
    let canvas_w = PIXEL_WIDTH as f32 * canvas_scale;
    let canvas_h = PIXEL_HEIGHT as f32 * canvas_scale;
    let canvas_offset_x = (win_w - canvas_w) * 0.5;
    let canvas_offset_y = (win_h - canvas_h) * 0.5;

    let seed = generator.seed();
    let ctx = contexts.ctx_mut()?;

    // Use a single layered painter so all waypoints share one draw layer and
    // don't interfere with egui's widget layout.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("waypoint_layer"),
    ));

    // Deduplicate by cell: twin/hub airports spawn two RunwaySlabs, but we want
    // one stalk centred between them. Average pos/elevation per cell.
    let mut cell_map: std::collections::HashMap<(i32, i32), (Vec2, f32, u32)> =
        std::collections::HashMap::new();
    for slab in &slabs {
        let e = cell_map.entry(slab.cell).or_insert((Vec2::ZERO, 0.0, 0));
        e.0 += slab.pos;
        e.1 += slab.elevation;
        e.2 += 1;
    }

    for (cell, (pos_sum, elev_sum, count)) in &cell_map {
        let n = *count as f32;
        let pos = *pos_sum / n;
        let elevation = elev_sum / n;

        let dist_km = Vec2::new(pos.x - plane_pos.x, pos.y - plane_pos.z).length() / 1000.0;

        if dist_km < MIN_DIST_KM { continue; }

        let alpha = (1.0 - (dist_km - FADE_START_KM).max(0.0) / (FAR_DIST_KM - FADE_START_KM))
            .clamp(0.0, 1.0);
        if alpha <= 0.01 { continue; }

        let base_world = Vec3::new(pos.x, elevation + STALK_BASE_OFFSET, pos.y);
        let tip_world  = Vec3::new(pos.x, elevation + STALK_TIP_OFFSET,  pos.y);
        let cell = *cell;

        let Ok(base_canvas) = inner_cam.world_to_viewport(inner_gtf, base_world) else { continue };
        let Ok(tip_canvas)  = inner_cam.world_to_viewport(inner_gtf, tip_world)  else { continue };

        let to_win = |cp: Vec2| egui::pos2(
            cp.x * canvas_scale + canvas_offset_x,
            cp.y * canvas_scale + canvas_offset_y,
        );
        let base_win = to_win(base_canvas);
        let tip_win  = to_win(tip_canvas);

        if tip_win.x < -200.0 || tip_win.x > win_w + 200.0
        || tip_win.y < -100.0 || tip_win.y > win_h + 100.0 {
            continue;
        }

        let a = (alpha * 255.0) as u8;
        let stalk_color = egui::Color32::from_rgba_unmultiplied(220, 235, 255, (alpha * 180.0) as u8);
        let dot_color   = egui::Color32::from_rgba_unmultiplied(255, 255, 255, a);
        let ident_color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, a);
        let dist_color  = egui::Color32::from_rgba_unmultiplied(160, 200, 255, a);

        painter.line_segment([base_win, tip_win], egui::Stroke::new(1.0, stalk_color));
        painter.circle_filled(tip_win, 2.5, dot_color);

        let ident = runway_ident(seed, cell);
        let dist_text = format!("{dist_km:.1} km");

        let font_ident = egui::FontId::proportional(14.0);
        let font_dist  = egui::FontId::proportional(12.0);

        // Measure so we can centre both lines horizontally over the tip.
        let ident_galley = painter.layout_no_wrap(ident.clone(), font_ident.clone(), ident_color);
        let dist_galley  = painter.layout_no_wrap(dist_text.clone(), font_dist.clone(), dist_color);

        let line_gap = 2.0;
        let ident_size = ident_galley.size();
        let dist_size  = dist_galley.size();
        let block_h = ident_size.y + line_gap + dist_size.y;
        let top_y = tip_win.y - block_h - 6.0;

        let ident_x = tip_win.x - ident_size.x * 0.5;
        let dist_x  = tip_win.x - dist_size.x * 0.5;

        painter.galley(egui::pos2(ident_x, top_y), ident_galley, ident_color);
        painter.galley(egui::pos2(dist_x, top_y + ident_size.y + line_gap), dist_galley, dist_color);
    }

    Ok(())
}
