//! My Plane window — pilot-facing controls for the aircraft: engine power,
//! loadout, flight assists, and control sensitivity. Toggled from the menu bar.

use avian3d::prelude::{AngularVelocity, LinearVelocity};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

use crate::camera::CameraMode;
use crate::physics::aircraft_physics::AircraftRoot;
use crate::physics::flight_config::{CARGO_MAX_KG, FUEL_TANK_MAX_KG, FlightModelConfig};
use crate::plane::{Airplane, PlaneState, reset_to_runway};
use crate::terrain::WorldGenerator;
use crate::ui::menu_bar::MenuBar;

// Mirrors instrument_panel.rs's palette so the loadout gauges read as the
// same instrument family as the rest of the panel.
const BORDER:   egui::Color32 = egui::Color32::from_rgb(45, 74, 122);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(139, 154, 181);
const ACCENT:   egui::Color32 = egui::Color32::from_rgb(59, 130, 246);
const WARN:     egui::Color32 = egui::Color32::from_rgb(235, 90, 90);
const CAUTION:  egui::Color32 = egui::Color32::from_rgb(230, 200, 80);
const GOOD:     egui::Color32 = egui::Color32::from_rgb(90, 200, 130);
// Fuel fill uses yellow/amber tones (distinct from cargo's green) so the two
// gauge types read differently at a glance even at a full fill level.
const FUEL_LOW:  egui::Color32 = WARN;
const FUEL_MID:  egui::Color32 = egui::Color32::from_rgb(235, 165, 40);
const FUEL_FULL: egui::Color32 = egui::Color32::from_rgb(235, 200, 60);

pub struct PlaneMenuPlugin;

impl Plugin for PlaneMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            (draw_plane_menu, draw_crash_banner).in_set(crate::ui::UiSet),
        );
    }
}

/// Always-on (not menu-gated) banner shown while `PlaneState.crashed` is set,
/// telling the player how to recover. `react_to_crash` (hull_collision.rs)
/// has already dropped the camera into chase mode by the time this is visible.
fn draw_crash_banner(
    mut contexts: EguiContexts,
    plane_q: Query<&PlaneState, With<Airplane>>,
) -> Result {
    let Ok(state) = plane_q.single() else { return Ok(()) };
    if !state.crashed {
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    egui::Area::new(egui::Id::new("crash_banner"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(egui::Color32::from_rgba_unmultiplied(30, 10, 10, 220))
                .stroke(egui::Stroke::new(1.5, WARN))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.colored_label(WARN, egui::RichText::new("CRASHED").size(20.0).strong());
                        ui.label(egui::RichText::new("Press R to reset to runway").color(TEXT_DIM));
                    });
                });
        });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_plane_menu(
    mut bar: ResMut<MenuBar>,
    mut cfg: ResMut<FlightModelConfig>,
    mut contexts: EguiContexts,
    world_gen: Res<WorldGenerator>,
    mut camera_mode: ResMut<CameraMode>,
    mut plane_q: Query<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity, &mut PlaneState, &mut AircraftRoot), With<Airplane>>,
) -> Result {
    if !bar.my_plane { return Ok(()); }

    let ctx = contexts.ctx_mut()?;

    egui::Window::new("My Plane")
        .open(&mut bar.my_plane)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(240.0, 48.0))
        .default_width(300.0)
        .resizable(true)
        .show(ctx, |ui| {
            // ── Reset ────────────────────────────────────────────────────────
            // Puts the aircraft back on the runway at the same spot and
            // orientation as the initial spawn (main.rs::setup), zeroing
            // velocities and transient flags — a quick fix for a glitched
            // position (stuck in terrain, flipped over, etc.) without
            // restarting the whole sim.
            if ui.button("Reset Plane to Runway").clicked()
                && let Ok((mut transform, mut lin_vel, mut ang_vel, mut state, mut root)) = plane_q.single_mut()
            {
                reset_to_runway(&mut transform, &mut lin_vel, &mut ang_vel, &mut state, &mut root, world_gen.as_ref());
                *camera_mode = CameraMode::Orbit;
            }

            // ── Engine ──────────────────────────────────────────────────────
            ui.add_space(8.0);
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

            ui.add_space(6.0);
            // Fixed-width strip so the painter-based gauges can't blow out
            // the window's auto-sized width the way they did nested inside
            // a plain `ui.horizontal` (each vertical_centered child would
            // request unbounded width from its horizontal parent).
            ui.allocate_ui(egui::vec2(280.0, 80.0), |ui| {
                ui.horizontal(|ui| {
                    // Fuel tanks sit right next to each other (tight spacing)
                    // since they're the same kind of gauge; a wider gap
                    // separates them from cargo/seats.
                    ui.spacing_mut().item_spacing.x = 3.0;
                    fill_bar(ui, "L TANK", cfg.cargo.fuel_left_kg, FUEL_TANK_MAX_KG, FillKind::Fuel, |f| format!("{f:.0}"));
                    fill_bar(ui, "R TANK", cfg.cargo.fuel_right_kg, FUEL_TANK_MAX_KG, FillKind::Fuel, |f| format!("{f:.0}"));
                    ui.add_space(12.0);
                    fill_bar(ui, "CARGO", cfg.cargo.cargo_kg, CARGO_MAX_KG, FillKind::Cargo, |f| format!("{f:.0}"));
                    ui.add_space(12.0);
                    seat_grid(ui, cfg.cargo.passengers);
                });
            });

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

/// Which color ramp a `fill_bar` uses — fuel reads yellow/amber, cargo reads
/// green, so the two gauge types are visually distinct even both when full.
#[derive(Clone, Copy)]
enum FillKind {
    Fuel,
    Cargo,
}

impl FillKind {
    fn color(self, frac: f32) -> egui::Color32 {
        match self {
            FillKind::Fuel => if frac < 0.15 { FUEL_LOW } else if frac < 0.3 { FUEL_MID } else { FUEL_FULL },
            FillKind::Cargo => if frac < 0.15 { WARN } else if frac < 0.3 { CAUTION } else { GOOD },
        }
    }
}

/// A small vertical fill-bar gauge (fuel/cargo level), styled like the
/// instrument panel's fuel bars. `label_fn` formats the value shown below it.
fn fill_bar(ui: &mut egui::Ui, label: &str, value: f32, max: f32, kind: FillKind, label_fn: impl Fn(f32) -> String) {
    ui.allocate_ui(egui::vec2(40.0, 80.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(label).size(9.0).color(TEXT_DIM));

            let size = egui::Vec2::new(20.0, 48.0);
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let painter = ui.painter();

            painter.rect_filled(rect, egui::CornerRadius::from(3u8), egui::Color32::from_rgb(10, 13, 20));
            painter.rect_stroke(rect, egui::CornerRadius::from(3u8), egui::Stroke::new(1.0, BORDER), egui::StrokeKind::Outside);

            let frac = (value / max).clamp(0.0, 1.0);
            let fill_height = rect.height() * frac;
            let fill_rect = egui::Rect::from_min_max(
                egui::Pos2::new(rect.left(), rect.bottom() - fill_height),
                egui::Pos2::new(rect.right(), rect.bottom()),
            );
            painter.rect_filled(fill_rect, egui::CornerRadius::from(3u8), kind.color(frac));

            ui.label(egui::RichText::new(label_fn(value)).size(9.0).color(TEXT_DIM));
        });
    });
}

/// Occupancy readout: a 2x2 seat grid. Seat 1 (the pilot/PIC) is always
/// shown filled since `passengers` is clamped to a minimum of 1 (the pilot
/// is never absent), and is labelled distinctly (accent blue) from the
/// remaining passenger seats (green). Filled in seat order 1,2,3,4 reading
/// left-to-right, top-to-bottom.
fn seat_grid(ui: &mut egui::Ui, passengers: u32) {
    ui.allocate_ui(egui::vec2(56.0, 80.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("SEATS").size(9.0).color(TEXT_DIM));

            let seat_size = egui::Vec2::new(22.0, 22.0);
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
            for row in 0..2u32 {
                ui.horizontal(|ui| {
                    for col in 0..2u32 {
                        let seat = row * 2 + col + 1;
                        let (rect, _) = ui.allocate_exact_size(seat_size, egui::Sense::hover());
                        let painter = ui.painter();
                        painter.rect_stroke(rect, egui::CornerRadius::from(3u8), egui::Stroke::new(1.0, BORDER), egui::StrokeKind::Outside);

                        let occupied = seat <= passengers.max(1);
                        let color = if seat == 1 { ACCENT } else { GOOD };
                        painter.rect_filled(
                            rect,
                            egui::CornerRadius::from(3u8),
                            if occupied { color } else { egui::Color32::from_rgb(10, 13, 20) },
                        );
                    }
                });
            }

            ui.label(egui::RichText::new(format!("{}/4", passengers.max(1))).size(9.0).color(TEXT_DIM));
        });
    });
}
