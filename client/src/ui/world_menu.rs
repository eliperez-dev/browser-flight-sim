//! World window — toggled from the menu bar. Tabbed: Weather (time of day,
//! fog) and Terrain (world-generation seed/scale/streaming, mirroring the
//! subset of sliders in the Dev Tools "World Generation" section).

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::fog::FogSettings;
use crate::sky::DayNightCycle;
use crate::terrain::WorldGenConfig;
use crate::ui::menu_bar::MenuBar;

pub struct WorldMenuPlugin;

impl Plugin for WorldMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldMenuTab>()
            .add_systems(EguiPrimaryContextPass, draw_world.in_set(crate::ui::UiSet));
    }
}

#[derive(Resource, Default, PartialEq, Eq)]
enum WorldMenuTab {
    #[default]
    Weather,
    Terrain,
}

fn format_game_time(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn draw_world(
    mut bar: ResMut<MenuBar>,
    mut tab: ResMut<WorldMenuTab>,
    mut sky: ResMut<DayNightCycle>,
    mut fog: ResMut<FogSettings>,
    mut world: ResMut<WorldGenConfig>,
    mut contexts: EguiContexts,
) -> Result {
    if !bar.world { return Ok(()); }

    let ctx = contexts.ctx_mut()?;

    // Edit a snapshot and write back only on real change — same pattern as the
    // Dev Tools World Generation section, so egui's every-frame slider touch
    // doesn't spam change-detection and defeat the terrain regen debounce.
    let mut w = world.clone();

    egui::Window::new("World")
        .open(&mut bar.world)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(120.0, 48.0))
        .default_width(300.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut *tab, WorldMenuTab::Weather, "Weather");
                ui.selectable_value(&mut *tab, WorldMenuTab::Terrain, "Terrain");
            });
            ui.separator();

            match *tab {
                WorldMenuTab::Weather => draw_weather_tab(ui, &mut sky, &mut fog),
                WorldMenuTab::Terrain => draw_terrain_tab(ui, &mut w),
            }
        });

    if w != *world {
        *world = w;
    }

    Ok(())
}

fn draw_weather_tab(ui: &mut egui::Ui, sky: &mut DayNightCycle, fog: &mut FogSettings) {
    ui.heading("Time of Day");
    ui.separator();

    ui.label(format!("Clock: {}", format_game_time(sky.time_of_day)));
    ui.add(egui::Slider::new(&mut sky.time_of_day, 0.0..=1.0)
        .text("Time of day"));

    ui.horizontal(|ui| {
        let paused = sky.speed == 0.0;
        if ui.button(if paused { "▶  Play" } else { "⏸  Pause" }).clicked() {
            sky.speed = if paused { 0.005 } else { 0.0 };
        }
    });

    ui.add_space(8.0);
    ui.heading("Fog");
    ui.separator();

    ui.checkbox(&mut fog.enabled, "Fog enabled");
    ui.add_enabled(
        fog.enabled,
        egui::Slider::new(&mut fog.visibility, 200.0..=100_000.0)
            .text("Visibility (m)")
            .logarithmic(true),
    );
    ui.add_enabled(
        fog.enabled,
        egui::Slider::new(&mut fog.directional_light_exponent, 1.0..=50.0)
            .text("Sun glow exponent"),
    );
}

fn draw_terrain_tab(ui: &mut egui::Ui, w: &mut WorldGenConfig) {
    ui.heading("World Generation");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Seed:");
        ui.add(egui::DragValue::new(&mut w.seed).speed(1.0));
        if ui.button("Randomize").clicked() {
            w.seed = w.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
    });
    ui.add(egui::Slider::new(&mut w.horizontal_scale, 0.5..=10.0)
        .text("Horizontal scale"));
    ui.add(egui::Slider::new(&mut w.height_scale, 10.0..=600.0)
        .text("Vertical scale (relief)"));

    ui.add_space(8.0);
    ui.heading("Streaming & LOD");
    ui.separator();

    ui.add(egui::Slider::new(&mut w.render_distance, 2..=100)
        .text("Render distance (chunks)")
        .integer());
    ui.add(egui::Slider::new(&mut w.max_chunks_per_frame, 1..=50)
        .text("Max chunk builds / frame")
        .integer());
}
