//! In-world waypoint labels for loaded runways.
//!
//! The vertical stalk is a 3D mesh (spawned by the runway streamer). This
//! module controls stalk visibility and draws the egui text label projected
//! from the stalk tip's world position.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::{OuterCamera, PIXEL_HEIGHT, PIXEL_WIDTH};
use crate::plane::Airplane;
use crate::terrain::{WaypointStalk, WorldGenerator, airport_name, runway_ident};
use crate::terrain::TerrainCamera;

/// Must match STALK_TIP_OFFSET / STALK_BASE_OFFSET in runway.rs.
const STALK_TIP_OFFSET: f32 = 1000.0;
const STALK_BASE_OFFSET: f32 = 2.0;

/// Hide everything (stalk + label) closer than this.
const MIN_DIST_KM: f32 = 3.0;
/// Begin fading at this distance; invisible by FAR_DIST_KM.
const FADE_START_KM: f32 = 30.0;
const FAR_DIST_KM: f32 = 120.0;

pub struct WaypointsPlugin;

impl Plugin for WaypointsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_stalk_visibility);
        app.add_systems(EguiPrimaryContextPass, draw_waypoint_labels);
    }
}

/// Hides the 3D stalk mesh when the camera is within MIN_DIST_KM.
/// Runs in Update so Visibility is set before the render frame.
pub fn update_stalk_visibility(
    plane_q: Query<&Transform, With<Airplane>>,
    mut stalks: Query<(&WaypointStalk, &Transform, &mut Visibility)>,
) {
    let Ok(plane_tf) = plane_q.single() else { return };
    let plane_pos = plane_tf.translation;

    for (_stalk, stalk_tf, mut vis) in &mut stalks {
        let pos = stalk_tf.translation;
        let dist_km = Vec2::new(pos.x - plane_pos.x, pos.z - plane_pos.z).length() / 1000.0;
        *vis = if dist_km < MIN_DIST_KM {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

pub fn draw_waypoint_labels(
    mut contexts: EguiContexts,
    generator: Res<WorldGenerator>,
    stalks: Query<(&WaypointStalk, &Transform)>,
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

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("waypoint_layer"),
    ));

    let to_win = |cp: Vec2| egui::pos2(
        cp.x * canvas_scale + canvas_offset_x,
        cp.y * canvas_scale + canvas_offset_y,
    );

    for (stalk, stalk_tf) in &stalks {
        let pos = stalk_tf.translation;
        let dist_km = Vec2::new(pos.x - plane_pos.x, pos.z - plane_pos.z).length() / 1000.0;
        if dist_km < MIN_DIST_KM { continue; }

        let alpha = (1.0 - (dist_km - FADE_START_KM).max(0.0) / (FAR_DIST_KM - FADE_START_KM))
            .clamp(0.0, 1.0);
        if alpha <= 0.01 { continue; }

        // Tip is at the top of the stalk. The stalk transform is at the midpoint,
        // so tip_y = pos.y + (stalk_h / 2).
        let stalk_h = STALK_TIP_OFFSET - STALK_BASE_OFFSET;
        let tip_world = Vec3::new(pos.x, pos.y + stalk_h * 0.5, pos.z);

        let Ok(tip_canvas) = inner_cam.world_to_viewport(inner_gtf, tip_world) else { continue };
        let tip_win = to_win(tip_canvas);

        if tip_win.x < -200.0 || tip_win.x > win_w + 200.0
        || tip_win.y < -100.0 || tip_win.y > win_h + 100.0 {
            continue;
        }

        let a = (alpha * 255.0) as u8;
        let dot_color   = egui::Color32::from_rgba_unmultiplied(255, 255, 255, a);
        let ident_color = egui::Color32::from_rgba_unmultiplied(255, 255, 255, a);
        let kind_color  = egui::Color32::from_rgba_unmultiplied(255, 255, 255, a);
        let dist_color  = egui::Color32::from_rgba_unmultiplied(255, 220, 220, a);

        painter.circle_filled(tip_win, 3.5, dot_color);

        let ident     = runway_ident(seed, stalk.cell);
        let kind_text = airport_name(seed, stalk.cell, stalk.kind);
        let dist_text = format!("{dist_km:.1} km");

        let font_ident = egui::FontId::proportional(16.0);
        let font_kind  = egui::FontId::proportional(16.0);
        let font_dist  = egui::FontId::proportional(16.0);

        let ident_galley = painter.layout_no_wrap(ident,      font_ident, ident_color);
        let kind_galley  = painter.layout_no_wrap(kind_text.to_string(), font_kind, kind_color);
        let dist_galley  = painter.layout_no_wrap(dist_text,  font_dist,  dist_color);

        let gap = 2.0;
        let ident_sz = ident_galley.size();
        let kind_sz  = kind_galley.size();
        let dist_sz  = dist_galley.size();
        let block_h = ident_sz.y + gap + kind_sz.y + gap + dist_sz.y;
        let top_y = tip_win.y - block_h - 6.0;

        let cx = tip_win.x;
        painter.galley(egui::pos2(cx - ident_sz.x * 0.5, top_y),                              ident_galley, ident_color);
        painter.galley(egui::pos2(cx - kind_sz.x  * 0.5, top_y + ident_sz.y + gap),           kind_galley,  kind_color);
        painter.galley(egui::pos2(cx - dist_sz.x  * 0.5, top_y + ident_sz.y + gap + kind_sz.y + gap), dist_galley,  dist_color);
    }

    Ok(())
}
