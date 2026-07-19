//! My Plane window — pilot-facing controls for the aircraft: engine power,
//! loadout, flight assists, and control sensitivity. Toggled from the menu bar.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::physics::flight_config::{CARGO_MAX_KG, FUEL_TANK_MAX_KG, FlightModelConfig};
use crate::ui::menu_bar::MenuBar;

pub struct PlaneMenuPlugin;

impl Plugin for PlaneMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_plane_menu.in_set(crate::ui::UiSet));
    }
}

fn draw_plane_menu(
    mut bar: ResMut<MenuBar>,
    mut cfg: ResMut<FlightModelConfig>,
    mut contexts: EguiContexts,
) -> Result {
    if !bar.my_plane { return Ok(()); }

    let ctx = contexts.ctx_mut()?;

    egui::Window::new("My Plane")
        .open(&mut bar.my_plane)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(240.0, 48.0))
        .default_width(280.0)
        .resizable(false)
        .show(ctx, |ui| {
            // ── Engine ──────────────────────────────────────────────────────
            ui.heading("Engine");
            ui.separator();
            ui.add(egui::Slider::new(&mut cfg.thrust_max, 0.0..=20_000.0)
                .text("Max thrust (N)"));

            // ── Loadout ─────────────────────────────────────────────────────
            ui.add_space(8.0);
            ui.heading("Loadout");
            ui.separator();

            let (loaded_mass, _, _) = cfg.loaded_mass_properties();
            ui.label(format!("Total mass: {loaded_mass:.0} kg"));
            ui.add_space(4.0);

            ui.add(egui::Slider::new(&mut cfg.cargo.fuel_left_kg, 0.0..=FUEL_TANK_MAX_KG)
                .text("Fuel left wing (kg)"));
            ui.add(egui::Slider::new(&mut cfg.cargo.fuel_right_kg, 0.0..=FUEL_TANK_MAX_KG)
                .text("Fuel right wing (kg)"));
            ui.add(egui::Slider::new(&mut cfg.cargo.cargo_kg, 0.0..=CARGO_MAX_KG)
                .text("Cargo (kg)"));
            ui.add(egui::Slider::new(&mut cfg.cargo.passengers, 1..=4)
                .text("Occupants")
                .integer());

            // ── Flight Assists ───────────────────────────────────────────────
            ui.add_space(8.0);
            ui.heading("Flight Assists");
            ui.separator();

            ui.add(egui::Slider::new(&mut cfg.auto_level_strength, 0.0..=500.0)
                .text("Auto-level strength"));
            ui.add(egui::Slider::new(&mut cfg.pitch_assist_strength, 0.0..=500.0)
                .text("Pitch assist strength"));

            // ── Control Sensitivity ──────────────────────────────────────────
            ui.add_space(8.0);
            ui.heading("Control Sensitivity");
            ui.separator();

            ui.add(egui::Slider::new(&mut cfg.pitch_sensitivity, 0.0..=2.0)
                .text("Pitch"));
            ui.add(egui::Slider::new(&mut cfg.roll_sensitivity, 0.0..=2.0)
                .text("Roll"));
            ui.add(egui::Slider::new(&mut cfg.yaw_sensitivity, 0.0..=2.0)
                .text("Yaw"));
        });

    Ok(())
}
