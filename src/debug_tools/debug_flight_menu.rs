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

use crate::physics::aero_surface_config::AeroSurfaceConfig;
use crate::physics::flight_config::{CARGO_MAX_KG, FUEL_TANK_MAX_KG, FlightModelConfig};

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
                .default_open(false)
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
            // Aerodynamics
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Aerodynamics")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.aero_damp.x, 0.0..=200.0)
                        .text("Aero damp roll (X)"));
                    ui.add(egui::Slider::new(&mut cfg.aero_damp.y, 0.0..=200.0)
                        .text("Aero damp yaw (Y)"));
                    ui.add(egui::Slider::new(&mut cfg.aero_damp.z, 0.0..=200.0)
                        .text("Aero damp pitch (Z)"));
                    ui.add(egui::Slider::new(&mut cfg.fuselage_drag.z, 0.0..=2.0)
                        .text("Fuselage drag fwd (CdA)"));
                    ui.add(egui::Slider::new(&mut cfg.fuselage_drag.x, 0.0..=100.0)
                        .text("Fuselage drag side (CdA)"));
                    ui.add(egui::Slider::new(&mut cfg.fuselage_drag.y, 0.0..=100.0)
                        .text("Fuselage drag vert (CdA)"));
                    ui.add(egui::Slider::new(&mut cfg.air_density, 0.1..=2.0)
                        .text("Air density (kg/m³)"));
                    ui.add(egui::Slider::new(&mut cfg.gravity, 0.0..=20.0)
                        .text("Gravity (m/s²)"));
                    ui.add(egui::Slider::new(&mut cfg.prediction_fraction, 0.0..=1.0)
                        .text("Prediction fraction"));
                    ui.add(egui::Slider::new(&mut cfg.ground_effect_strength, 0.0..=10.0)
                        .text("Ground effect strength"));
                    ui.add(egui::Slider::new(&mut cfg.ground_effect_span, 1.0..=20.0)
                        .text("Ground effect span (m)"));
                });

            // ---------------------------------------------------------------
            // Flight assists
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Flight Assists")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.auto_level_strength, 0.0..=500.0)
                        .text("Auto-level strength"));
                    ui.add(egui::Slider::new(&mut cfg.bank_turn_strength, 0.0..=500.0)
                        .text("Bank-to-turn strength"));
                });

            // ---------------------------------------------------------------
            // Engine
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Engine")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.thrust_max, 0.0..=20_000.0)
                        .text("Max thrust (N)"));
                    ui.add(egui::Slider::new(&mut cfg.engine_spool_up_tau, 0.05..=10.0)
                        .text("Spool-up tau (s)")
                        .logarithmic(true));
                    ui.add(egui::Slider::new(&mut cfg.engine_spool_down_tau, 0.05..=15.0)
                        .text("Spool-down tau (s)")
                        .logarithmic(true));
                    ui.add(egui::Slider::new(&mut cfg.engine_crank_rps, 0.0..=20.0)
                        .text("Crank speed (rev/s)"));
                    ui.add(egui::Slider::new(&mut cfg.engine_start_secs, 0.1..=10.0)
                        .text("Start time (s)"));
                });

            // ---------------------------------------------------------------
            // Propeller (visual placeholder; G shows the gizmo)
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Propeller")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label("Position (local units, ×0.1 → m)");
                    ui.add(egui::Slider::new(&mut cfg.propeller.prop_position.x, -30.0..=30.0)
                        .text("Prop X (local)"));
                    ui.add(egui::Slider::new(&mut cfg.propeller.prop_position.y, -30.0..=30.0)
                        .text("Prop Y up (local)"));
                    ui.add(egui::Slider::new(&mut cfg.propeller.prop_position.z, -30.0..=90.0)
                        .text("Prop Z fwd (local)"));
                    ui.add(egui::Slider::new(&mut cfg.propeller.prop_radius, 0.1..=3.0)
                        .text("Prop radius (m)"));

                    ui.separator();
                    ui.add(egui::Slider::new(&mut cfg.propeller.prop_idle_rps, 0.0..=30.0)
                        .text("Idle spin (rev/s)"));
                    ui.add(egui::Slider::new(&mut cfg.propeller.prop_max_rps, 0.0..=80.0)
                        .text("Full-throttle spin (rev/s)"));
                });

            // ---------------------------------------------------------------
            // Landing gear (spring-damper suspension feel)
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Landing Gear")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_spring, 0.0..=200_000.0)
                        .text("Spring stiffness (N/m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_damping, 0.0..=40_000.0)
                        .text("Damping (N·s/m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_rest_length, 0.1..=2.0)
                        .text("Main strut length (m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_nose_rest_length, 0.1..=2.0)
                        .text("Nose strut length (m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_grip, 0.0..=20_000.0)
                        .text("Lateral grip (N·s/m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_rolling_resistance, 0.0..=0.3)
                        .text("Rolling resistance (Crr)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_brake_strength, 0.0..=2.0)
                        .text("Brake strength (B)"));

                    ui.separator();
                    ui.label("Geometry (G to show struts)");
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_nose_z, -3.0..=4.0)
                        .text("Nose wheel fwd (m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_main_z, -3.0..=4.0)
                        .text("Main gear fwd (m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_track, 0.0..=6.0)
                        .text("Main gear track (m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_mount_height, -2.0..=1.0)
                        .text("Mount height (m)"));
                });

            // ---------------------------------------------------------------
            // Mass & inertia (synced to the Avian rigid body at runtime)
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Mass & Inertia")
                .default_open(false)
                .show(ui, |ui| {
                    // Live readout of the empty airframe + current loadout.
                    let (loaded_mass, loaded_com, loaded_inertia) =
                        cfg.loaded_mass_properties();
                    ui.label(format!("Loaded mass: {loaded_mass:.0} kg"));
                    ui.label(format!(
                        "Loaded CoM (m): {:.2}, {:.2}, {:.2}",
                        loaded_com.x, loaded_com.y, loaded_com.z
                    ));
                    ui.label(format!(
                        "Loaded inertia: {:.0} / {:.0} / {:.0}",
                        loaded_inertia.x, loaded_inertia.y, loaded_inertia.z
                    ));

                    ui.separator();
                    ui.label("Loadout");
                    ui.add(egui::Slider::new(&mut cfg.cargo.fuel_left_kg, 0.0..=FUEL_TANK_MAX_KG)
                        .text("Fuel L wing (kg)"));
                    ui.add(egui::Slider::new(&mut cfg.cargo.fuel_right_kg, 0.0..=FUEL_TANK_MAX_KG)
                        .text("Fuel R wing (kg)"));
                    ui.add(egui::Slider::new(&mut cfg.cargo.cargo_kg, 0.0..=CARGO_MAX_KG)
                        .text("Cargo (kg)"));
                    ui.add(egui::Slider::new(&mut cfg.cargo.passengers, 1..=4)
                        .text("Occupants")
                        .integer());

                    ui.separator();
                    ui.label("Empty airframe");
                    // Minimum mass avoids a divide-by-zero in F = ma.
                    ui.add(egui::Slider::new(&mut cfg.mass, 50.0..=5_000.0)
                        .text("Empty mass (kg)"));
                    ui.add(egui::Slider::new(&mut cfg.angular_inertia.x, 50.0..=10_000.0)
                        .text("Inertia pitch X (kg·m²)"));
                    ui.add(egui::Slider::new(&mut cfg.angular_inertia.y, 50.0..=10_000.0)
                        .text("Inertia yaw Y (kg·m²)"));
                    ui.add(egui::Slider::new(&mut cfg.angular_inertia.z, 50.0..=10_000.0)
                        .text("Inertia roll Z (kg·m²)"));
                    ui.add(egui::Slider::new(&mut cfg.angular_damping, 0.0..=5.0)
                        .text("Angular damping (1/s)"));
                });

            // ---------------------------------------------------------------
            // Visual (cosmetic mesh alignment, no effect on physics)
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Visual")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.model_offset.x, -50.0..=50.0)
                        .text("Model offset X (local)"));
                    ui.add(egui::Slider::new(&mut cfg.model_offset.y, -50.0..=50.0)
                        .text("Model offset Y (local)"));
                    ui.add(egui::Slider::new(&mut cfg.model_offset.z, -50.0..=50.0)
                        .text("Model offset Z (local)"));
                });

            // ---------------------------------------------------------------
            // Balance — rigid-body center of mass (local units, ×0.1 → metres)
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Balance (CoM)")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.center_of_mass.x, -20.0..=20.0)
                        .text("CoM X (local)"));
                    ui.add(egui::Slider::new(&mut cfg.center_of_mass.y, -20.0..=20.0)
                        .text("CoM Y up (local)"));
                    ui.add(egui::Slider::new(&mut cfg.center_of_mass.z, -30.0..=30.0)
                        .text("CoM Z fwd (local)"));
                });

            // ---------------------------------------------------------------
            // Aerodynamic surfaces — per-surface geometry & stall behaviour.
            // Edits are pushed onto the live surfaces by apply_config_to_entities.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Surfaces")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut cfg.wing_incidence, -5.0..=10.0)
                        .text("Wing incidence (°)"));
                    ui.separator();
                    egui::CollapsingHeader::new("Wing")
                        .show(ui, |ui| surface_controls(ui, &mut cfg.wing));
                    egui::CollapsingHeader::new("Aileron")
                        .show(ui, |ui| surface_controls(ui, &mut cfg.aileron));
                    egui::CollapsingHeader::new("Elevator (h-stab)")
                        .show(ui, |ui| surface_controls(ui, &mut cfg.elevator));
                    egui::CollapsingHeader::new("Rudder (v-stab)")
                        .show(ui, |ui| surface_controls(ui, &mut cfg.rudder));
                    egui::CollapsingHeader::new("Body lift")
                        .show(ui, |ui| surface_controls(ui, &mut cfg.body_lift));
                });
        });
    Ok(())
}

/// Renders sliders for one aerodynamic surface's geometry and stall behaviour.
/// Shared by every surface section so the controls stay consistent.
fn surface_controls(ui: &mut egui::Ui, c: &mut AeroSurfaceConfig) {
    ui.add(egui::Slider::new(&mut c.lift_slope, 0.1..=10.0)
        .text("Lift slope (1/rad)"));
    ui.add(egui::Slider::new(&mut c.skin_friction, 0.1..=0.2)
        .text("Skin friction (Cd)"));
    ui.add(egui::Slider::new(&mut c.zero_lift_aoa, -10.0..=10.0)
        .text("Zero-lift AoA (°)"));
    ui.add(egui::Slider::new(&mut c.stall_angle_high, 0.1..=30.0)
        .text("Stall angle high (°)"));
    ui.add(egui::Slider::new(&mut c.stall_angle_low, -30.0..=0.1)
        .text("Stall angle low (°)"));
    ui.add(egui::Slider::new(&mut c.chord, 0.1..=5.0)
        .text("Chord (m)"));
    ui.add(egui::Slider::new(&mut c.span, 0.1..=10.0)
        .text("Span (m)"));
    ui.add(egui::Slider::new(&mut c.aspect_ratio, 0.1..=15.0)
        .text("Aspect ratio"));
    ui.add(egui::Slider::new(&mut c.flap_fraction, 0.1..=1.0)
        .text("Flap fraction"));
}
