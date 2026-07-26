//! Graphics window — toggled from the menu bar. Fullscreen, UI scale,
//! render resolution (Low/High), and terrain render distance.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::{RenderScale, ShadowQuality, UiScale, is_fullscreen, request_toggle_fullscreen};
use crate::fog::FogSettings;
use crate::terrain::{RunwayLightsEnabled, WorldGenConfig};
use crate::ui::menu_bar::MenuBar;

pub struct GraphicsMenuPlugin;

impl Plugin for GraphicsMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_graphics_menu.in_set(crate::ui::UiSet));
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_graphics_menu(
    mut bar: ResMut<MenuBar>,
    mut ui_scale: ResMut<UiScale>,
    mut render_scale: ResMut<RenderScale>,
    mut shadow_quality: ResMut<ShadowQuality>,
    mut fog: ResMut<FogSettings>,
    mut runway_lights: ResMut<RunwayLightsEnabled>,
    mut world_gen: ResMut<WorldGenConfig>,
    mut contexts: EguiContexts,
) -> Result {
    if !bar.graphics { return Ok(()); }

    let ctx = contexts.ctx_mut()?;

    // Edited as a snapshot and written back only on real change — same
    // pattern as world_menu.rs's Terrain tab, so egui's every-frame slider
    // touch doesn't spam change-detection and defeat the terrain regen
    // debounce (regenerate_terrain in terrain/streaming.rs only rebuilds
    // when WorldGenConfig actually differs from its last-seen snapshot).
    let mut w = world_gen.clone();

    egui::Window::new("Graphics")
        .open(&mut bar.graphics)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(120.0, 48.0))
        .default_width(220.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.heading("Display");
            ui.separator();

            let mut fullscreen = is_fullscreen();
            if ui.checkbox(&mut fullscreen, "Fullscreen (F11)").clicked() {
                request_toggle_fullscreen();
            }

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Render Resolution");
            ui.horizontal(|ui| {
                if ui.selectable_label(*render_scale == RenderScale::Low, "Low").clicked() {
                    *render_scale = RenderScale::Low;
                }
                if ui.selectable_label(*render_scale == RenderScale::Medium, "Medium").clicked() {
                    *render_scale = RenderScale::Medium;
                }
                if ui.selectable_label(*render_scale == RenderScale::High, "High").clicked() {
                    *render_scale = RenderScale::High;
                }
                if ui.selectable_label(*render_scale == RenderScale::Ultra, "Ultra").clicked() {
                    *render_scale = RenderScale::Ultra;
                }
            });
            ui.label(format!("{}×{}", render_scale.width(), render_scale.height()));

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Shadows");
            ui.horizontal(|ui| {
                if ui.selectable_label(*shadow_quality == ShadowQuality::Off, "Off").clicked() {
                    *shadow_quality = ShadowQuality::Off;
                }
                if ui.selectable_label(*shadow_quality == ShadowQuality::Low, "Low").clicked() {
                    *shadow_quality = ShadowQuality::Low;
                }
                if ui.selectable_label(*shadow_quality == ShadowQuality::Medium, "Medium").clicked() {
                    *shadow_quality = ShadowQuality::Medium;
                }
                if ui.selectable_label(*shadow_quality == ShadowQuality::High, "High").clicked() {
                    *shadow_quality = ShadowQuality::High;
                }
            });

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Fog");
            ui.checkbox(&mut fog.enabled, "Enabled");

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Runway Lights");
            ui.checkbox(&mut runway_lights.0, "Enabled");
            ui.label("Only rendered at night regardless of this setting.");

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Render Distance");
            ui.label("How far terrain streams in before it's culled.");
            ui.add(egui::Slider::new(&mut w.render_distance, 2..=100)
                .text("Chunks")
                .integer());

            ui.add_space(10.0);
            ui.separator();
            ui.heading("UI Scale");
            ui.label("Size of menus and instruments.");
            // Drags a local copy, not `ui_scale.0` directly: applying every
            // intermediate value while dragging changes the size of the
            // slider widget itself mid-drag (bigger scale -> bigger slider
            // -> mouse movement maps to an even bigger value change), which
            // spirals into the UI ballooning uncontrollably as you drag.
            // Only committing on release breaks that feedback loop.
            let mut pending = ui_scale.0;
            let response = ui.add(egui::Slider::new(&mut pending, 0.5..=2.0).text("scale"));
            if response.drag_stopped() || response.lost_focus() {
                ui_scale.0 = pending;
            }
        });

    if w != *world_gen {
        *world_gen = w;
    }

    Ok(())
}
