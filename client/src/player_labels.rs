//! In-world name + distance labels above remote players' aircraft, drawn the
//! same way `waypoints.rs` labels runways: an egui overlay projected each
//! frame from the entity's world position onto the pixel-art canvas.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::{OuterCamera, RenderScale, UiScale, WorldToOverlay, fade_alpha};
use crate::network::RemotePlayer;
use crate::plane::Airplane;
use crate::terrain::TerrainCamera;

/// Height above the aircraft's origin the label floats at, roughly clearing
/// the tail fin so it doesn't overlap the model.
const LABEL_HEIGHT_OFFSET: f32 = 2.5;

/// Fade out labels for very distant players so the sky doesn't fill up with
/// text; mirrors the far end of waypoints.rs's fade range.
const FADE_START_KM: f32 = 30.0;
const FAR_DIST_KM: f32 = 120.0;

pub struct PlayerLabelsPlugin;

impl Plugin for PlayerLabelsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_player_labels);
    }
}

pub fn draw_player_labels(
    mut contexts: EguiContexts,
    // Reads the ghost's actual rendered Transform, not RemoteTarget's raw
    // latest sample — the ghost itself is drawn from an interpolated,
    // intentionally-delayed position (see network.rs's RENDER_DELAY), and
    // labeling the raw un-delayed sample instead made the name tag visibly
    // run ahead of the plane it's supposed to label.
    render_scale: Res<RenderScale>,
    ui_scale: Res<UiScale>,
    remotes: Query<(&RemotePlayer, &Transform), Without<Airplane>>,
    plane_q: Query<&Transform, With<Airplane>>,
    inner_cam_q: Query<(&Camera, &GlobalTransform), With<TerrainCamera>>,
    outer_proj_q: Query<&Projection, With<OuterCamera>>,
    windows: Query<&Window>,
) -> Result {
    if remotes.is_empty() {
        return Ok(());
    }

    let Ok(plane_tf) = plane_q.single() else { return Ok(()) };
    let plane_pos = plane_tf.translation;
    let Ok((inner_cam, inner_gtf)) = inner_cam_q.single() else { return Ok(()) };

    let overlay = WorldToOverlay::new(*render_scale, ui_scale.0, outer_proj_q.single().ok(), windows.single().ok());

    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("player_label_layer"),
    ));

    for (player, transform) in &remotes {
        let pos = transform.translation;
        let dist_km = Vec2::new(pos.x - plane_pos.x, pos.z - plane_pos.z).length() / 1000.0;

        let alpha = fade_alpha(dist_km, FADE_START_KM, FAR_DIST_KM);
        if alpha <= 0.01 { continue; }

        let label_world = pos + Vec3::new(0.0, LABEL_HEIGHT_OFFSET, 0.0);
        let Some(label_win) = overlay.project(inner_cam, inner_gtf, label_world) else { continue };

        let a = (alpha * 255.0) as u8;
        let name_color = egui::Color32::from_rgba_unmultiplied(120, 200, 255, a);
        let dist_color = egui::Color32::from_rgba_unmultiplied(220, 220, 220, a);

        painter.circle_filled(label_win, 3.0, name_color);

        let dist_text = format!("{dist_km:.1} km");
        let font = egui::FontId::proportional(14.0);
        let name_galley = painter.layout_no_wrap(player.name.clone(), font.clone(), name_color);
        let dist_galley = painter.layout_no_wrap(dist_text, font, dist_color);

        let gap = 1.0;
        let name_sz = name_galley.size();
        let dist_sz = dist_galley.size();
        let block_h = name_sz.y + gap + dist_sz.y;
        let top_y = label_win.y - block_h - 6.0;
        let cx = label_win.x;

        painter.galley(egui::pos2(cx - name_sz.x * 0.5, top_y), name_galley, name_color);
        painter.galley(egui::pos2(cx - dist_sz.x * 0.5, top_y + name_sz.y + gap), dist_galley, dist_color);
    }

    Ok(())
}
