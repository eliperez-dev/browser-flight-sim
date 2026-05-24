//! In-game debug panel for tweaking flight-model constants at runtime.
//!
//! Press **F3** to show / hide the panel.  All sliders write directly into
//! [`FlightModelConfig`], so changes take effect on the very next physics tick.
//! The "Reset to defaults" button restores the original hand-tuned values.
//!
//! The panel is drawn with `bevy_egui` and intentionally uses an immediate-mode
//! style: no extra state, no events — just sliders that mutate the resource.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::physics::flight_config::FlightModelConfig;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// Registers the debug menu system and its visibility toggle.
pub struct DebugFlightMenuPlugin;

impl Plugin for DebugFlightMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<DebugMenuVisible>()
            // Disable auto-attach so we can pin the egui context to a specific
            // camera (the screen-space Camera2d) via PrimaryEguiContext in main.rs.
            .add_systems(Startup, disable_auto_primary_context)
            .add_systems(Update, toggle_menu)
            // draw_menu must run inside EguiPrimaryContextPass so it targets
            // the camera marked with PrimaryEguiContext rather than defaulting
            // to whichever camera bevy_egui picks first.
            .add_systems(EguiPrimaryContextPass, draw_menu);
    }
}

fn disable_auto_primary_context(mut settings: ResMut<EguiGlobalSettings>) {
    // Without this, bevy_egui auto-creates a primary context on the first camera
    // it finds — which may be the pixel-canvas Camera3d rendering to a texture.
    // We instead mark the screen-space Camera2d with PrimaryEguiContext in main.rs.
    settings.auto_create_primary_context = false;
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Whether the debug flight-model panel is currently open.
#[derive(Resource, Default)]
pub struct DebugMenuVisible(pub bool);

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Toggle panel visibility with F3.
fn toggle_menu(
    keys: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<DebugMenuVisible>,
) {
    if keys.just_pressed(KeyCode::F3) {
        visible.0 = !visible.0;
    }
}

/// Draw the egui panel when visible.
///
/// Each slider clamps to a physically sensible range so it is hard to
/// accidentally enter a value that crashes the simulation (e.g. zero servo tau
/// would cause a NaN in the exponential).
///
/// Runs on [`EguiPrimaryContextPass`], which targets the camera marked with
/// [`bevy_egui::PrimaryEguiContext`] — the screen-space Camera2d in main.rs.
fn draw_menu(
    mut contexts: EguiContexts,
    visible: Res<DebugMenuVisible>,
    mut cfg: ResMut<FlightModelConfig>,
) -> Result {
    if !visible.0 { return Ok(()); }

    let defaults = FlightModelConfig::default();

    egui::SidePanel::right("flight_debug_panel")
        .resizable(true)
        .min_width(280.0)
        .show(contexts.ctx_mut()?, |ui| {
            ui.heading("Flight Model Debug");
            ui.label("Press F3 to close");
            ui.separator();

            if ui.button("Reset to defaults").clicked() {
                *cfg = defaults.clone();
            }

            ui.separator();

            // ---------------------------------------------------------------
            // Control surfaces
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Control")
                .default_open(true)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.pitch_sensitivity, 0.0..=2.0)
                        .text("Pitch sensitivity"));
                    ui.add(egui::Slider::new(&mut cfg.roll_sensitivity, 0.0..=2.0)
                        .text("Roll sensitivity"));
                    ui.add(egui::Slider::new(&mut cfg.yaw_sensitivity, 0.0..=2.0)
                        .text("Yaw sensitivity"));
                    ui.add(egui::Slider::new(&mut cfg.throttle_rate, 0.05..=5.0)
                        .text("Throttle rate (1/s)"));
                    // Minimum tau prevents division-by-zero in the exp() call.
                    ui.add(egui::Slider::new(&mut cfg.servo_tau, 0.01..=1.0)
                        .text("Servo tau (s)")
                        .logarithmic(true));
                    ui.add(egui::Slider::new(&mut cfg.elevator_trim, -0.5..=0.5)
                        .text("Elevator trim (rad)"));
                });

            // ---------------------------------------------------------------
            // Speed envelope
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Speed Envelope")
                .default_open(true)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.stall_speed, 5.0..=80.0)
                        .text("Vs — stall (m/s)"));
                    ui.add(egui::Slider::new(&mut cfg.authority_limit_speed, 20.0..=150.0)
                        .text("Vno — limit (m/s)"));
                    ui.add(egui::Slider::new(&mut cfg.vne_speed, 40.0..=200.0)
                        .text("Vne — never exceed (m/s)"));
                    ui.add(egui::Slider::new(&mut cfg.vne_authority, 0.0..=1.0)
                        .text("Authority at Vne"));
                });

            // ---------------------------------------------------------------
            // Aerodynamics
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Aerodynamics")
                .default_open(true)
                .show(ui, |ui| {
                    // Rotational drag coefficient: torque = -coeff * speed * ang_vel.
                    // ~150 gives ζ ≈ 0.7 (well-damped) at cruise.
                    ui.add(egui::Slider::new(&mut cfg.aero_damp, 0.0..=500.0)
                        .text("Aero damping (N·m/rad/s)"));
                    ui.add(egui::Slider::new(&mut cfg.air_density, 0.1..=2.0)
                        .text("Air density (kg/m³)"));
                    ui.add(egui::Slider::new(&mut cfg.gravity, 0.0..=20.0)
                        .text("Gravity (m/s²)"));
                    ui.add(egui::Slider::new(&mut cfg.prediction_fraction, 0.0..=1.0)
                        .text("Prediction fraction"));
                });

            // ---------------------------------------------------------------
            // Engine
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Engine")
                .default_open(true)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.thrust_max, 0.0..=20_000.0)
                        .text("Max thrust (N)"));
                });
        });
    Ok(())
}
