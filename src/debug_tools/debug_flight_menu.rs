//! In-game debug panel for tweaking flight-model constants at runtime.
//!
//! Toggle via the menu bar ("Flight Model" button) or press **F3**.
//! All sliders write directly into [`FlightModelConfig`], so changes take
//! effect on the very next physics tick.

use avian3d::prelude::{AngularVelocity, LinearVelocity};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::camera::FixedCameraMounts;
use crate::debug_tools::debug_hud::DebugHud;
use crate::fog::FogSettings;
use crate::lights::LightTimers;
use crate::map::MapIconSettings;
use crate::physics::aero_surface_config::AeroSurfaceConfig;
use crate::physics::aircraft_physics::AircraftRoot;
use crate::physics::flight_config::{CARGO_MAX_KG, FUEL_TANK_MAX_KG, FlightModelConfig};
use crate::plane::{Airplane, PlaneState, spawn_position};
use crate::sky::DayNightCycle;
use crate::terrain::{BiomeShape, WorldGenConfig, WorldGenerator};
use crate::ui::menu_bar::MenuBar;
use crate::water::WaterSettings;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

/// Registers the debug menu system and its visibility toggle.
pub struct DebugFlightMenuPlugin;

impl Plugin for DebugFlightMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            // Disable auto-attach so we can pin the egui context to a specific
            // camera (the screen-space Camera2d) via PrimaryEguiContext in main.rs.
            .add_systems(Startup, disable_auto_primary_context)
            .add_systems(Update, toggle_menu_key)
            // draw_menu must run inside EguiPrimaryContextPass so it targets
            // the camera marked with PrimaryEguiContext rather than defaulting
            // to whichever camera bevy_egui picks first.
            .add_systems(EguiPrimaryContextPass, draw_menu.in_set(crate::ui::UiSet));
    }
}

fn disable_auto_primary_context(mut settings: ResMut<EguiGlobalSettings>) {
    // Without this, bevy_egui auto-creates a primary context on the first camera
    // it finds — which may be the pixel-canvas Camera3d rendering to a texture.
    // We instead mark the screen-space Camera2d with PrimaryEguiContext in main.rs.
    settings.auto_create_primary_context = false;
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// F3 keyboard shortcut mirrors the menu bar toggle.
fn toggle_menu_key(keys: Res<ButtonInput<KeyCode>>, mut bar: ResMut<MenuBar>) {
    if keys.just_pressed(KeyCode::F3) {
        bar.flight_model = !bar.flight_model;
    }
}

/// Draw the egui window when the menu bar has it open.
///
/// Each slider clamps to a physically sensible range so it is hard to
/// accidentally enter a value that crashes the simulation (e.g. zero servo tau
/// would cause a NaN in the exponential).
///
/// Runs on [`EguiPrimaryContextPass`], which targets the camera marked with
/// [`bevy_egui::PrimaryEguiContext`] — the screen-space Camera2d in main.rs.
#[allow(clippy::too_many_arguments)]
fn draw_menu(
    mut contexts: EguiContexts,
    mut bar: ResMut<MenuBar>,
    mut cfg: ResMut<FlightModelConfig>,
    mut world: ResMut<WorldGenConfig>,
    mut fog: ResMut<FogSettings>,
    mut water: ResMut<WaterSettings>,
    mut map_icons: ResMut<MapIconSettings>,
    mut sky: ResMut<DayNightCycle>,
    mut light_timers_q: Query<&mut LightTimers, With<Airplane>>,
    mut fixed_cams: ResMut<FixedCameraMounts>,
    hud: Res<DebugHud>,
    world_gen: Res<WorldGenerator>,
    mut plane_q: Query<(&mut Transform, &mut LinearVelocity, &mut AngularVelocity, &mut PlaneState, &mut AircraftRoot), With<Airplane>>,
) -> Result {
    if !bar.flight_model { return Ok(()); }

    let defaults = FlightModelConfig::default();

    // Edit a snapshot of the world config and write it back only if a value
    // actually changed. Touching `world` mutably (as egui sliders do every frame)
    // would otherwise flag it changed continuously and defeat the regen debounce.
    let mut w = world.clone();

    egui::Window::new("Flight Model")
        .open(&mut bar.flight_model)
        .order(egui::Order::Tooltip)
        .default_pos(egui::pos2(16.0, 48.0))
        .default_width(300.0)
        .resizable(true)
        .show(contexts.ctx_mut()?, |ui| {
            // Wrap the whole panel so the (long) list of sections scrolls with the
            // mouse wheel instead of overflowing the window.
            egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("Flight Model Debug");
            ui.separator();

            if ui.button("Reset to defaults").clicked() {
                *cfg = defaults.clone();
                w = WorldGenConfig::default();
            }

            ui.separator();

            // ---------------------------------------------------------------
            // Quick Cheats — one-off actions for fast iteration/testing,
            // as opposed to the tunable sliders/values below.
            // ---------------------------------------------------------------
            ui.heading("Quick Cheats");
            if ui.button("+1000 m altitude").clicked()
                && let Ok((mut tf, ..)) = plane_q.single_mut()
            {
                tf.translation.y += 1000.0;
            }
            if ui.button("Reset Plane to Runway").clicked()
                && let Ok((mut transform, mut lin_vel, mut ang_vel, mut state, mut root)) = plane_q.single_mut()
            {
                *transform = Transform::from_translation(spawn_position(world_gen.as_ref()))
                    .with_scale(Vec3::splat(0.1));
                lin_vel.0 = Vec3::ZERO;
                ang_vel.0 = Vec3::ZERO;
                state.crashed = false;
                root.throttle_percent = 0.0;
            }

            ui.separator();

            // ---------------------------------------------------------------
            // Live flight/state readout (formerly the on-screen white HUD text).
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Debug Readout")
                .default_open(true)
                .show(ui, |ui| {
                    for (label, value) in &hud.entries {
                        ui.label(format!("{label}: {value}"));
                    }
                });

            ui.separator();

            // ---------------------------------------------------------------
            // World generation — edits trigger a debounced terrain rebuild
            // (see terrain::streaming::regenerate_terrain).
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("World Generation")
                .default_open(false)
                .show(ui, |ui| {
                    // --- Base: seed + global scales ----------------------------
                    egui::CollapsingHeader::new("Base")
                        .id_salt("worldgen_base")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Seed:");
                                ui.add(egui::DragValue::new(&mut w.seed).speed(1.0));
                                if ui.button("Randomize").clicked() {
                                    // LCG step: a fresh pseudo-random seed each click.
                                    w.seed = w.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                                }
                            });
                            ui.add(egui::Slider::new(&mut w.horizontal_scale, 0.5..=10.0)
                                .text("Horizontal scale (tightness)"));
                            ui.add(egui::Slider::new(&mut w.height_scale, 10.0..=600.0)
                                .text("Height scale (relief)"));
                        });

                    // --- Climate: how the map is distributed across biomes -----
                    egui::CollapsingHeader::new("Climate & distribution")
                        .id_salt("worldgen_climate")
                        .show(ui, |ui| {
                            ui.add(egui::Slider::new(&mut w.biome_size, 0.1..=8.0)
                                .text("Biome size"));
                            ui.separator();
                            // Bias shifts the whole map along the axis; contrast
                            // (>1) sharpens biome boundaries, (<1) blends to middle.
                            ui.label("Temperature (cold ↔ hot)");
                            ui.add(egui::Slider::new(&mut w.temp_bias, -0.5..=0.5)
                                .text("Temp bias"));
                            ui.add(egui::Slider::new(&mut w.temp_contrast, 0.0..=4.0)
                                .text("Temp contrast"));
                            ui.separator();
                            ui.label("Humidity (dry ↔ wet) — also drives ocean amount");
                            ui.add(egui::Slider::new(&mut w.humidity_bias, -0.5..=0.5)
                                .text("Humidity bias"));
                            ui.add(egui::Slider::new(&mut w.humidity_contrast, 0.0..=4.0)
                                .text("Humidity contrast"));
                            ui.separator();
                            ui.label("Geography (realism)");
                            // Altitude cooling: high ground trends cold/snowy. 0 = off.
                            ui.add(egui::Slider::new(&mut w.temp_lapse, 0.0..=1.0)
                                .text("Altitude cooling (/1000m)"));
                            // Latitude banding: warm equators, cold bands between. 0 = off.
                            ui.add(egui::Slider::new(&mut w.latitude_strength, 0.0..=0.5)
                                .text("Latitude strength"));
                            ui.add(egui::Slider::new(&mut w.latitude_band, 5_000.0..=200_000.0)
                                .text("Latitude band (m)")
                                .logarithmic(true));
                        });

                    // --- Per-biome shaping (corners of the climate square) ------
                    egui::CollapsingHeader::new("Biomes")
                        .id_salt("worldgen_biomes")
                        .show(ui, |ui| {
                            // Elevation: raw offset (×height_scale → m). Relief:
                            // amplitude multiplier. Abundance: blend weight (1 =
                            // neutral). Edits rebuild the world (debounced).
                            biome_controls(ui, "Grasslands (cold, dry)", &mut w.grasslands);
                            biome_controls(ui, "Taiga (cold, wet)", &mut w.taiga);
                            biome_controls(ui, "Desert (hot, dry)", &mut w.desert);
                            biome_controls(ui, "Forest (hot, wet)", &mut w.forest);
                        });

                    // --- Oceans (geography-driven, independent of climate) ------
                    egui::CollapsingHeader::new("Oceans")
                        .id_salt("worldgen_oceans")
                        .show(ui, |ui| {
                            // Higher sea level = more of the map underwater.
                            ui.add(egui::Slider::new(&mut w.sea_level_threshold, 0.0..=1.0)
                                .text("Sea level (coverage)"));
                            ui.add(egui::Slider::new(&mut w.continent_size, 0.2..=8.0)
                                .text("Continent size"));
                            ui.add(egui::Slider::new(&mut w.ocean_transition_width, 0.02..=0.6)
                                .text("Coastline width"));
                            ui.add(egui::Slider::new(&mut w.ocean_depth, 0.0..=10.0)
                                .text("Basin depth (raw units)"));
                            // Wet coasts, dry interiors — the ocean→humidity link.
                            ui.add(egui::Slider::new(&mut w.coastal_humidity, 0.0..=1.0)
                                .text("Coastal humidity"));
                        });

                    // --- Streaming + LOD (apply live, no world rebuild) ---------
                    egui::CollapsingHeader::new("Streaming & LOD")
                        .id_salt("worldgen_streaming")
                        .show(ui, |ui| {
                            ui.add(egui::Slider::new(&mut w.render_distance, 2..=100)
                                .text("Render distance (chunks)")
                                .integer());
                            ui.add(egui::Slider::new(&mut w.max_chunks_per_frame, 1..=50)
                                .text("Max chunk builds / frame")
                                .integer());

                            ui.separator();
                            ui.label("LOD bands (near → far): max distance & detail");
                            for (i, (dist, subs)) in w.lod_levels.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(format!("L{i}"));
                                    ui.add(egui::Slider::new(dist, 0.5..=40.0)
                                        .text("dist"));
                                    ui.add(egui::Slider::new(subs, 1..=32)
                                        .text("subdiv")
                                        .integer());
                                });
                            }
                        });

                    ui.separator();
                    if ui.button("Reset world gen").clicked() {
                        w = WorldGenConfig::default();
                    }
                });

            // ---------------------------------------------------------------
            // Sky / Day-Night cycle — drives sun, ambient, sky colour & stars.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Sky / Day-Night")
                .default_open(false)
                .show(ui, |ui| {
                    ui.label(format!("Clock: {}", format_game_time(sky.time_of_day)));
                    ui.add(egui::Slider::new(&mut sky.time_of_day, 0.0..=1.0)
                        .text("Time of day"));
                    if ui.button(if sky.speed == 0.0 { "Play" } else { "Pause" }).clicked() {
                        sky.speed = if sky.speed == 0.0 { 0.005 } else { 0.0 };
                    }
                    ui.add(egui::Slider::new(&mut sky.speed, 0.0..=0.2)
                        .text("Time speed")
                        .logarithmic(true));
                    ui.add(egui::Slider::new(&mut sky.inclination, -1.0..=1.0)
                        .text("Orbit inclination (rad)"));
                    ui.checkbox(&mut sky.tint_fog, "Tint fog to sky colour");
                    ui.add_enabled(
                        sky.tint_fog,
                        egui::Slider::new(&mut sky.fog_haze, 0.0..=1.0)
                            .text("Fog haze (desaturation)"),
                    );

                    ui.separator();
                    if ui.button("Reset sky").clicked() {
                        *sky = DayNightCycle::default();
                    }
                });

            // ---------------------------------------------------------------
            // Aircraft lights — positions/intensities live-update from here.
            // The landing light (L key) toggle is also exposed as a checkbox.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Lights")
                .default_open(false)
                .show(ui, |ui| {
                    let lc = &mut cfg.lights;

                    // Landing light on/off (mirrors the L keybind)
                    if let Ok(mut timers) = light_timers_q.single_mut() {
                        ui.checkbox(&mut timers.landing_light_on, "Landing light (L)");
                    }

                    ui.separator();
                    ui.label("Nav lights (always on)");
                    ui.add(egui::Slider::new(&mut lc.nav_intensity, 0.0..=5_000.0)
                        .text("Nav intensity (lux)"));
                    ui.label("Left wingtip (red)");
                    ui.add(egui::Slider::new(&mut lc.nav_left_pos.x, -800.0..=0.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.nav_left_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.nav_left_pos.z, -800.0..=800.0).text("Z"));
                    ui.label("Right wingtip (green)");
                    ui.add(egui::Slider::new(&mut lc.nav_right_pos.x, 0.0..=800.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.nav_right_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.nav_right_pos.z, -800.0..=800.0).text("Z"));
                    ui.label("Tail (white)");
                    ui.add(egui::Slider::new(&mut lc.nav_tail_pos.x, -300.0..=300.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.nav_tail_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.nav_tail_pos.z, -800.0..=0.0).text("Z"));

                    ui.separator();
                    ui.label("Strobes (auto-flash)");
                    ui.add(egui::Slider::new(&mut lc.strobe_intensity, 0.0..=100_000.0)
                        .text("Strobe intensity (lux)").logarithmic(true));
                    ui.add(egui::Slider::new(&mut lc.strobe_period, 0.3..=3.0)
                        .text("Strobe period (s)"));
                    ui.add(egui::Slider::new(&mut lc.strobe_on_time, 0.01..=0.3)
                        .text("Flash duration (s)"));
                    ui.label("Strobe left");
                    ui.add(egui::Slider::new(&mut lc.strobe_left_pos.x, -800.0..=0.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.strobe_left_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.strobe_left_pos.z, -800.0..=800.0).text("Z"));
                    ui.label("Strobe right");
                    ui.add(egui::Slider::new(&mut lc.strobe_right_pos.x, 0.0..=800.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.strobe_right_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.strobe_right_pos.z, -800.0..=800.0).text("Z"));
                    ui.label("Strobe tail");
                    ui.add(egui::Slider::new(&mut lc.strobe_tail_pos.x, -300.0..=300.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.strobe_tail_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.strobe_tail_pos.z, -800.0..=0.0).text("Z"));

                    ui.separator();
                    ui.label("Beacon (engine-on pulse)");
                    ui.add(egui::Slider::new(&mut lc.beacon_intensity, 0.0..=20_000.0)
                        .text("Beacon intensity (lux)"));
                    ui.add(egui::Slider::new(&mut lc.beacon_period, 0.3..=3.0)
                        .text("Beacon period (s)"));
                    ui.add(egui::Slider::new(&mut lc.beacon_pos.x, -3000.0..=3000.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.beacon_pos.y, -2000.0..=1000.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.beacon_pos.z, -5000.0..=5000.0).text("Z"));

                    ui.separator();
                    ui.label("Landing light (L key)");
                    ui.add(egui::Slider::new(&mut lc.landing_intensity, 0.0..=20_000_000.0)
                        .text("Landing intensity (lux)").logarithmic(true));
                    ui.add(egui::Slider::new(&mut lc.landing_pitch_deg, -20.0..=20.0)
                        .text("Pitch down (°)"));
                    ui.add(egui::Slider::new(&mut lc.landing_outer_deg, 5.0..=140.0)
                        .text("Outer cone (°)"));
                    ui.add(egui::Slider::new(&mut lc.landing_inner_deg, 1.0..=140.0)
                        .text("Inner cone (°)"));
                    ui.add(egui::Slider::new(&mut lc.landing_pos.x, -300.0..=300.0).text("X"));
                    ui.add(egui::Slider::new(&mut lc.landing_pos.y, -200.0..=300.0).text("Y"));
                    ui.add(egui::Slider::new(&mut lc.landing_pos.z, -100.0..=600.0).text("Z"));

                    ui.separator();
                    if ui.button("Reset lights").clicked() {
                        *lc = crate::physics::flight_config::LightsConfig::default();
                    }
                });

            // ---------------------------------------------------------------
            // Fog — edits apply live to the camera's DistanceFog.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Fog")
                .default_open(false)
                .show(ui, |ui| {
                    ui.checkbox(&mut fog.enabled, "Enabled (1)");
                    ui.add(egui::Slider::new(&mut fog.visibility, 200.0..=100_000.0)
                        .text("Visibility (m)")
                        .logarithmic(true));
                    ui.add(egui::Slider::new(&mut fog.directional_light_exponent, 1.0..=50.0)
                        .text("Sun glow exponent"));

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.color_edit_button_rgb(&mut fog.color);
                        ui.label("Fog color");
                    });
                    ui.horizontal(|ui| {
                        ui.color_edit_button_rgb(&mut fog.extinction_color);
                        ui.label("Extinction (near)");
                    });
                    ui.horizontal(|ui| {
                        ui.color_edit_button_rgb(&mut fog.inscattering_color);
                        ui.label("Inscattering (far)");
                    });
                    ui.horizontal(|ui| {
                        ui.color_edit_button_rgb(&mut fog.directional_light_color);
                        ui.label("Sun glow color");
                    });

                    ui.separator();
                    if ui.button("Reset fog").clicked() {
                        *fog = FogSettings::default();
                    }
                });

            // ---------------------------------------------------------------
            // Fixed camera mounts — nose/tail/wingtip rigid offsets, editable
            // live so a mount can be dialed in while flying (Camera menu
            // switches which one is active). Offsets are metres in the
            // plane's local space; yaw/pitch aim the camera once mounted.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Fixed Cameras")
                .default_open(false)
                .show(ui, |ui| {
                    for mount in fixed_cams.mounts.iter_mut() {
                        ui.label(mount.name);
                        ui.add(egui::Slider::new(&mut mount.offset.x, -8.0..=8.0).text("X"));
                        ui.add(egui::Slider::new(&mut mount.offset.y, -4.0..=4.0).text("Y"));
                        ui.add(egui::Slider::new(&mut mount.offset.z, -10.0..=10.0).text("Z"));
                        ui.add(egui::Slider::new(&mut mount.yaw, -std::f32::consts::PI..=std::f32::consts::PI)
                            .text("Yaw (rad)"));
                        ui.add(egui::Slider::new(&mut mount.pitch, -1.5..=1.5)
                            .text("Pitch (rad)"));
                        ui.separator();
                    }
                    if ui.button("Reset fixed cameras").clicked() {
                        *fixed_cams = FixedCameraMounts::default();
                    }
                });

            // ---------------------------------------------------------------
            // Water — a single shiny plane at sea level; edits apply live.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Water")
                .default_open(false)
                .show(ui, |ui| {
                    ui.checkbox(&mut water.enabled, "Enabled");
                    ui.add(egui::Slider::new(&mut water.sea_level, -600.0..=200.0)
                        .text("Sea level (m)"));
                    ui.add(egui::Slider::new(&mut water.perceptual_roughness, 0.0..=1.0)
                        .text("Roughness (glossiness)"));
                    ui.add(egui::Slider::new(&mut water.metallic, 0.0..=1.0)
                        .text("Metallic"));

                    ui.horizontal(|ui| {
                        ui.color_edit_button_rgb(&mut water.color);
                        ui.label("Water color");
                    });

                    ui.separator();
                    if ui.button("Reset water").clicked() {
                        *water = WaterSettings::default();
                    }
                });

            // ---------------------------------------------------------------
            // Map icons — pixel sizes for the F4 map overlay; edits apply live.
            // ---------------------------------------------------------------
            egui::CollapsingHeader::new("Map Icons")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add(egui::Slider::new(&mut map_icons.plane_len, 2.0..=30.0)
                        .text("Plane length"));
                    ui.add(egui::Slider::new(&mut map_icons.plane_width, 1.0..=20.0)
                        .text("Plane width"));
                    ui.add(egui::Slider::new(&mut map_icons.camera_size, 2.0..=30.0)
                        .text("Camera size"));
                    ui.add(egui::Slider::new(&mut map_icons.airport_circle, 2.0..=24.0)
                        .text("Airport circle (zoomed out)"));
                    ui.add(egui::Slider::new(&mut map_icons.runway_width, 0.5..=8.0)
                        .text("Runway line width"));
                    ui.add(egui::Slider::new(&mut map_icons.selected_ring, 3.0..=24.0)
                        .text("Selected ring"));
                    ui.add(egui::Slider::new(&mut map_icons.breadcrumb_len, 1.0..=12.0)
                        .text("Breadcrumb dash"));
                    ui.add(egui::Slider::new(&mut map_icons.waypoint_marker, 2.0..=20.0)
                        .text("Waypoint marker"));
                    ui.add(egui::Slider::new(&mut map_icons.label_font, 6.0..=24.0)
                        .text("Label font"));

                    ui.separator();
                    if ui.button("Reset map icons").clicked() {
                        *map_icons = MapIconSettings::default();
                    }
                });

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
                    ui.add(egui::Slider::new(&mut cfg.pitch_assist_strength, 0.0..=500.0)
                        .text("Pitch assist strength"));
                    ui.add(egui::Slider::new(&mut cfg.pitch_rate_damp, 0.0..=500.0)
                        .text("Pitch rate damp"));
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
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_nose_mount_height, -2.0..=1.0)
                        .text("Nose mount height (m)"));
                    ui.add(egui::Slider::new(&mut cfg.landing_gear.gear_main_mount_height, -2.0..=1.0)
                        .text("Main mount height (m)"));
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
        });

    // Commit world-gen edits only when something actually changed, so the
    // resource's change-detection drives the (debounced) terrain rebuild.
    if w != *world {
        *world = w;
    }
    Ok(())
}

/// Formats a `0.0..1.0` time-of-day as a 24-hour `HH:MM` clock (0.0 = 00:00).
fn format_game_time(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// Renders the elevation + relief sliders for one biome, inside its own
/// collapsing header so the four biomes stay compact in the panel.
fn biome_controls(ui: &mut egui::Ui, name: &str, b: &mut BiomeShape) {
    egui::CollapsingHeader::new(name)
        .id_salt(name)
        .show(ui, |ui| {
            ui.add(egui::Slider::new(&mut b.elevation, -5.0..=15.0)
                .text("Elevation (raw)"));
            ui.add(egui::Slider::new(&mut b.relief, 0.0..=3.0)
                .text("Relief (ruggedness)"));
            ui.add(egui::Slider::new(&mut b.abundance, 0.0..=4.0)
                .text("Abundance (how much)"));
        });
}

/// Renders sliders for one aerodynamic surface's geometry and stall behaviour.
/// Shared by every surface section so the controls stay consistent.
fn surface_controls(ui: &mut egui::Ui, c: &mut AeroSurfaceConfig) {
    ui.add(egui::Slider::new(&mut c.lift_slope, 0.1..=10.0)
        .text("Lift slope (1/rad)"));
    ui.add(egui::Slider::new(&mut c.skin_friction, 0.0..=0.2)
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
