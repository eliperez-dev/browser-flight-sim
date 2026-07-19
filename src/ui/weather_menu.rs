//! Weather & time-of-day window — toggled from the menu bar.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::fog::FogSettings;
use crate::sky::DayNightCycle;
use crate::ui::menu_bar::MenuBar;

pub struct WeatherMenuPlugin;

impl Plugin for WeatherMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_weather.in_set(crate::ui::UiSet));
    }
}

fn format_game_time(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn draw_weather(
    mut bar: ResMut<MenuBar>,
    mut sky: ResMut<DayNightCycle>,
    mut fog: ResMut<FogSettings>,
    mut contexts: EguiContexts,
) -> Result {
    if !bar.weather { return Ok(()); }

    let ctx = contexts.ctx_mut()?;

    egui::Window::new("Weather")
        .open(&mut bar.weather)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(120.0, 48.0))
        .default_width(280.0)
        .resizable(false)
        .show(ctx, |ui| {
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
        });

    Ok(())
}
