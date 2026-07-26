//! Always-on flight instrument cluster: artificial horizon, airspeed/
//! altitude/vertical-speed tapes (with a barometric readout folded into the
//! altitude window), heading compass, an RPM tachometer, a throttle/mixture
//! quadrant, and a flap/trim panel. Ported from the old sim's `hud.rs` and
//! restyled to match this sim's "compact dark glass" theme (see
//! `ui::style`). Hidden in free camera mode, same as the old sim hid it
//! outside its `FlightMode::FreeFlight` check. Fuel/cargo/passenger load
//! visuals live in `ui::plane_menu` instead, since that's config state
//! rather than a live flight reading.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui::{self, Frame}};

use crate::camera::CameraMode;
use crate::lights::LightTimers;
use crate::physics::aircraft_physics::{AircraftRoot, EngineState};
use crate::physics::flight_config::FlightModelConfig;
use crate::plane::{Airplane, PlaneState};

// Shared palette — mirrors ui::style's "compact dark glass" theme so the
// instrument bezels read as part of the same UI, not a bolted-on overlay.
const PANEL_BG:    egui::Color32 = egui::Color32::from_rgba_premultiplied(20, 27, 43, 235);
const BORDER:      egui::Color32 = egui::Color32::from_rgb(45, 74, 122);
const TEXT:        egui::Color32 = egui::Color32::from_rgb(209, 217, 230);
const TEXT_DIM:    egui::Color32 = egui::Color32::from_rgb(139, 154, 181);
const ACCENT:      egui::Color32 = egui::Color32::from_rgb(59, 130, 246);
const WARN:        egui::Color32 = egui::Color32::from_rgb(235, 90, 90);
const CAUTION:     egui::Color32 = egui::Color32::from_rgb(230, 200, 80);
const GOOD:        egui::Color32 = egui::Color32::from_rgb(90, 200, 130);
const SKY_COLOR:   egui::Color32 = egui::Color32::from_rgb(45, 95, 165);
const GROUND_COLOR:egui::Color32 = egui::Color32::from_rgb(80, 60, 35);

// C172-realistic reference speeds (kt) for the airspeed tape color bands.
// No stall/Vne fields exist in FlightModelConfig yet, so these are fixed
// rather than invented config knobs — revisit if the config grows them.
const V_STALL_KT: f32 = 48.0;
const V_CRUISE_KT: f32 = 120.0;
const V_NE_KT: f32 = 163.0;

const MS_TO_KT: f32 = 1.943_844;
const M_TO_FT: f32 = 3.280_84;

pub struct InstrumentPanelPlugin;

impl Plugin for InstrumentPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, draw_instrument_panel.in_set(crate::ui::UiSet));
    }
}

fn panel_frame() -> Frame {
    Frame::new()
        .fill(PANEL_BG)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(egui::CornerRadius::from(6u8))
        .inner_margin(egui::Margin::same(8))
}


#[allow(clippy::too_many_arguments)]
fn draw_instrument_panel(
    mut contexts: EguiContexts,
    camera_mode: Res<CameraMode>,
    mut cfg: ResMut<FlightModelConfig>,
    mut plane_q: Query<(&Transform, &mut PlaneState, &LinearVelocity, &mut AircraftRoot, &mut LightTimers), With<Airplane>>,
) -> Result {
    // Only makes sense while actually looking at the aircraft in flight —
    // Free and Chase both repurpose WASD/EQ for camera movement, so flight
    // controls (and this HUD) are suppressed the same way in both.
    if matches!(*camera_mode, CameraMode::Free | CameraMode::Chase) {
        return Ok(());
    }
    let Ok((transform, mut state, velocity, mut root, mut lights)) = plane_q.single_mut() else { return Ok(()) };

    let forward = transform.forward().as_vec3();
    let heading = heading_from_forward(forward);
    let pitch = pitch_from_forward(forward);
    let roll = roll_from_transform(transform);

    let speed_kt = state.speed * MS_TO_KT;
    let altitude_ft = transform.translation.y * M_TO_FT;
    let vertical_speed_fpm = velocity.0.y * M_TO_FT * 60.0;

    let ctx = contexts.ctx_mut()?;

    // Every bottom-row panel is laid out from explicit widths so adjacent
    // windows stack at a fixed gap (rather than hand-tuned offsets that drift
    // out of sync whenever one panel's width changes).
    const MARGIN: f32 = 22.0; // distance from the screen edge to the outermost panels
    const GAP: f32 = 16.0; // breathing room between adjacent panels in a cluster
    const TAPE_WINDOW_HEIGHT: f32 = 260.0; // IAS height
    const VS_WINDOW_HEIGHT: f32 = 225.0; // slightly shorter than IAS
    const ALT_WINDOW_HEIGHT: f32 = 225.0; // shorter container than VS/IAS
    const TAPE_WIDTH: f32 = 72.0; // ALT and VS width
    const ATTITUDE_WIDTH: f32 = 146.0;
    const ATTITUDE_HEIGHT: f32 = 190.0;

    // Right-anchored cluster, laid right-to-left: ALT, then VS, then ATTITUDE.
    let alt_offset = -MARGIN;
    let vspeed_offset = alt_offset - TAPE_WIDTH - GAP;
    let attitude_offset = vspeed_offset - TAPE_WIDTH - GAP;

    egui::Window::new("instrument_altitude")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::RIGHT_BOTTOM, [alt_offset, -MARGIN])
        .fixed_size([TAPE_WIDTH, ALT_WINDOW_HEIGHT])
        .min_height(ALT_WINDOW_HEIGHT)
        .max_height(ALT_WINDOW_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("ALT (ft)").size(11.0).color(TEXT_DIM));
                draw_altitude_tape(ui, altitude_ft);
                ui.add_space(2.0);
                ui.separator();
                draw_baro_readout(ui, cfg.air_density, transform.translation.y);
            });
        });

    egui::Window::new("instrument_vspeed")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::RIGHT_BOTTOM, [vspeed_offset, -MARGIN])
        .fixed_size([TAPE_WIDTH, VS_WINDOW_HEIGHT])
        .min_height(VS_WINDOW_HEIGHT)
        .max_height(VS_WINDOW_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("VS (fpm)").size(11.0).color(TEXT_DIM));
            });
            let tape_height = ui.available_height();
            ui.vertical_centered(|ui| {
                draw_vspeed_tape(ui, vertical_speed_fpm, tape_height);
            });
        });

    egui::Window::new("instrument_attitude")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::RIGHT_BOTTOM, [attitude_offset, -MARGIN])
        .fixed_size([ATTITUDE_WIDTH, ATTITUDE_HEIGHT])
        .min_height(ATTITUDE_HEIGHT)
        .max_height(ATTITUDE_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("ATTITUDE").size(11.0).color(TEXT_DIM));
                draw_artificial_horizon(ui, pitch, roll);
            });
        });

    egui::Window::new("instrument_airspeed")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [MARGIN, -MARGIN])
        .fixed_size([TAPE_WIDTH, TAPE_WINDOW_HEIGHT])
        .min_height(TAPE_WINDOW_HEIGHT)
        .max_height(TAPE_WINDOW_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("IAS (kt)").size(11.0).color(TEXT_DIM));
            });
            ui.add_space((ui.available_height() - 220.0).max(0.0) / 2.0);
            ui.vertical_centered(|ui| {
                draw_airspeed_tape(ui, speed_kt);
            });
        });

    // Left-anchored cluster: ENGINE, then THR/MIX, then FLAP/TRIM, then
    // BUTTONS, laid left-to-right starting just after the airspeed tape, all
    // sharing the same row height so nothing has to hand-fit a stacked panel
    // on top of another.
    const ENGINE_WIDTH: f32 = 142.0;
    // Tall enough to fit ENGINE's gauge + IGN indicator stack (its tallest
    // content column) without clipping — THR/MIX and FLAP/TRIM share this
    // same height even though their own content is a bit shorter.
    const ENGINE_HEIGHT: f32 = 170.0;
    const QUADRANT_WIDTH: f32 = 74.0;
    const SURFACES_WIDTH: f32 = 74.0;
    // Content width: 3 switches * 40 + 2 * 3px spacing + 2 * 8px inner margin.
    const BUTTONS_WIDTH: f32 = 142.0;
    const BUTTONS_HEIGHT: f32 = 70.0; // just the label + one switch row, not the full row height
    // Content width: one 58px indicator + one 40px switch + 3px spacing + 2 * 8px inner margin.
    const BRAKES_WIDTH: f32 = 117.0;
    const BRAKES_HEIGHT: f32 = 70.0;

    let engine_offset = MARGIN + TAPE_WIDTH + GAP;
    let quadrant_offset = engine_offset + ENGINE_WIDTH + GAP;
    let surfaces_offset = quadrant_offset + QUADRANT_WIDTH + GAP;
    let buttons_offset = surfaces_offset + SURFACES_WIDTH + GAP;
    let brakes_offset = buttons_offset + BUTTONS_WIDTH + GAP;

    egui::Window::new("instrument_engine")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [engine_offset, -MARGIN])
        .fixed_size([ENGINE_WIDTH, ENGINE_HEIGHT])
        .min_height(ENGINE_HEIGHT)
        .max_height(ENGINE_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("ENGINE").size(11.0).color(TEXT_DIM));
            });
            ui.add_space((ui.available_height() - 96.0 - 4.0 - 26.0).max(0.0) / 2.0);
            ui.vertical_centered(|ui| {
                draw_engine_gauge(ui, &root, cfg.propeller.prop_max_rps);
                ui.add_space(4.0);
                let (ign_label, ign_on, ign_color) = match root.engine_state {
                    EngineState::Off => ("OFF", false, OFF_TEXT),
                    EngineState::Cranking => (" START...", true, CAUTION),
                    EngineState::Running => ("ON", true, GOOD),
                };
                light_indicator(ui, &format!("IGN {ign_label}"), ign_on, ign_color);
            });
        });

    egui::Window::new("instrument_quadrant")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [quadrant_offset, -MARGIN])
        .fixed_size([QUADRANT_WIDTH, ENGINE_HEIGHT])
        .min_height(ENGINE_HEIGHT)
        .max_height(ENGINE_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("THR / MIX").size(11.0).color(TEXT_DIM));
            });
            ui.add_space((ui.available_height() - 146.0).max(0.0) / 2.0);
            const LEVER_PAIR_WIDTH: f32 = 28.0 * 2.0 + 2.0;
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - LEVER_PAIR_WIDTH).max(0.0) / 2.0);
                draw_throttle_mixture_levers(ui, &mut root);
            });
        });

    egui::Window::new("instrument_surfaces")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [surfaces_offset, -MARGIN])
        .fixed_size([SURFACES_WIDTH, ENGINE_HEIGHT])
        .min_height(ENGINE_HEIGHT)
        .max_height(ENGINE_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("FLAP / TRIM").size(11.0).color(TEXT_DIM));
            });
            ui.add_space((ui.available_height() - 146.0).max(0.0) / 2.0);
            const LEVER_PAIR_WIDTH: f32 = 28.0 * 2.0 + 2.0;
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - LEVER_PAIR_WIDTH).max(0.0) / 2.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                draw_flap_lever(ui, &mut root);
                draw_trim_lever(ui, &mut cfg.elevator_trim);
            });
        });

    egui::Window::new("instrument_buttons")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [buttons_offset, -MARGIN])
        .fixed_size([BUTTONS_WIDTH, BUTTONS_HEIGHT])
        .min_height(BUTTONS_HEIGHT)
        .max_height(BUTTONS_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("LIGHTS").size(11.0).color(TEXT_DIM));
                draw_switch_panel(ui, &mut lights);
            });
        });

    egui::Window::new("instrument_brakes")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_BOTTOM, [brakes_offset, -MARGIN])
        .fixed_size([BRAKES_WIDTH, BRAKES_HEIGHT])
        .min_height(BRAKES_HEIGHT)
        .max_height(BRAKES_HEIGHT)
        .frame(panel_frame())
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("BRAKES").size(11.0).color(TEXT_DIM));
                draw_brake_panel(ui, &mut state);
            });
        });

    egui::Window::new("instrument_heading")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .resizable(false)
        .anchor(egui::Align2::LEFT_TOP, [20.0, 20.0])
        .fixed_size([120.0, 140.0])
        .min_height(140.0)
        .max_height(140.0)
        .frame(panel_frame().inner_margin(egui::Margin::same(3)))
        .show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(TEXT);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("COMPASS").size(11.0).color(TEXT_DIM));
                draw_heading_compass(ui, heading);
                ui.label(egui::RichText::new(format!("{:03.0}°", heading)).size(13.0).color(TEXT));
            });
        });

    Ok(())
}

fn wrap_360(angle: f32) -> f32 {
    let a = angle % 360.0;
    if a < 0.0 { a + 360.0 } else { a }
}

fn heading_from_forward(forward: Vec3) -> f32 {
    wrap_360(f32::atan2(forward.x, -forward.z).to_degrees() + 90.0)
}

fn pitch_from_forward(forward: Vec3) -> f32 {
    let horizontal = (forward.x * forward.x + forward.z * forward.z).sqrt();
    f32::atan2(forward.y, horizontal).to_degrees()
}

fn roll_from_transform(transform: &Transform) -> f32 {
    let up = transform.up().as_vec3();
    let right = transform.right().as_vec3();
    f32::atan2(right.y, up.y).to_degrees()
}

fn draw_artificial_horizon(ui: &mut egui::Ui, pitch: f32, roll: f32) {
    let (response, painter) = ui.allocate_painter(egui::Vec2::new(126.0, 140.0), egui::Sense::hover());
    let center = egui::Pos2::new(response.rect.center().x, response.rect.top() + 60.0);
    let radius = 56.0;

    let pitch_offset = (pitch / 90.0) * radius;
    let roll_rad = (-roll).to_radians();
    let (sin_roll, cos_roll) = (roll_rad.sin(), roll_rad.cos());

    let rotate = |x: f32, y: f32| -> egui::Pos2 {
        egui::Pos2::new(center.x + x * cos_roll - y * sin_roll, center.y + x * sin_roll + y * cos_roll)
    };

    painter.circle_filled(center, radius + 1.0, SKY_COLOR);

    // Ground half-plane, clipped to the circle by intersecting the boundary
    // at each of 360 sampled angles — the same construction as the old HUD.
    let segments = 180;
    let mut ground_points = Vec::new();
    let mut prev_is_ground = false;
    for i in 0..=segments {
        let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let (cx, cy) = (radius * angle.cos(), radius * angle.sin());
        let local_y = -cx * sin_roll + cy * cos_roll;
        let is_ground = local_y > -pitch_offset;

        if i > 0 && is_ground != prev_is_ground {
            let prev_angle = ((i - 1) as f32 / segments as f32) * std::f32::consts::TAU;
            let (px, py) = (radius * prev_angle.cos(), radius * prev_angle.sin());
            let prev_local_y = -px * sin_roll + py * cos_roll;
            let t = (-pitch_offset - prev_local_y) / (local_y - prev_local_y);
            ground_points.push(egui::Pos2::new(center.x + px + t * (cx - px), center.y + py + t * (cy - py)));
        }
        if is_ground {
            ground_points.push(egui::Pos2::new(center.x + cx, center.y + cy));
        }
        prev_is_ground = is_ground;
    }
    if ground_points.len() >= 3 {
        painter.add(egui::Shape::convex_polygon(ground_points, GROUND_COLOR, egui::Stroke::NONE));
    }

    // Pitch ladder, every 10°, majors at 30°/90° with numeric labels.
    for i in -6..=6i32 {
        let angle = i as f32 * 10.0;
        let y_offset = (angle / 90.0) * radius - pitch_offset;
        if y_offset.abs() >= radius * 1.4 {
            continue;
        }
        let is_major = i % 3 == 0;
        let half_len = if is_major { 20.0 } else { 10.0 };
        let width = if is_major { 2.0 } else { 1.0 };
        painter.line_segment([rotate(-half_len, y_offset), rotate(half_len, y_offset)], egui::Stroke::new(width, TEXT));
        if i != 0 && is_major {
            painter.text(rotate(half_len + 12.0, y_offset), egui::Align2::LEFT_CENTER,
                format!("{}", angle.abs() as i32), egui::FontId::proportional(9.0), TEXT);
        }
    }

    // Fixed aircraft reference (wings-level bug, doesn't rotate with roll).
    painter.line_segment(
        [egui::Pos2::new(center.x - 55.0, center.y), egui::Pos2::new(center.x - 8.0, center.y)],
        egui::Stroke::new(3.0, ACCENT),
    );
    painter.line_segment(
        [egui::Pos2::new(center.x + 8.0, center.y), egui::Pos2::new(center.x + 55.0, center.y)],
        egui::Stroke::new(3.0, ACCENT),
    );
    painter.circle_filled(center, 2.5, ACCENT);

    painter.circle_stroke(center, radius, egui::Stroke::new(1.5, BORDER));

    // Numeric pitch/roll readout in the corners, below the dial circle.
    painter.text(
        egui::Pos2::new(response.rect.left(), response.rect.bottom()),
        egui::Align2::LEFT_BOTTOM,
        format!("P {pitch:+.0}°"),
        egui::FontId::proportional(10.0),
        TEXT_DIM,
    );
    painter.text(
        egui::Pos2::new(response.rect.right(), response.rect.bottom()),
        egui::Align2::RIGHT_BOTTOM,
        format!("R {roll:+.0}°"),
        egui::FontId::proportional(10.0),
        TEXT_DIM,
    );
}

fn draw_altitude_tape(ui: &mut egui::Ui, altitude_ft: f32) {
    let (response, painter) = ui.allocate_painter(egui::Vec2::new(56.0, 160.0), egui::Sense::hover());
    let rect = response.rect;
    let center_y = rect.center().y;

    let step = 50.0;
    let px_per_ft = 0.35;
    let start = ((altitude_ft - 500.0) / step).floor() * step;
    let end = start + 1000.0;

    for alt in (start as i32..=end as i32).step_by(step as usize) {
        let y = center_y + (altitude_ft - alt as f32) * px_per_ft;
        if y < rect.top() || y > rect.bottom() { continue; }
        let is_major = alt % 200 == 0;
        let tick_len = if is_major { 10.0 } else { 6.0 };
        painter.line_segment(
            [egui::Pos2::new(rect.right() - tick_len, y), egui::Pos2::new(rect.right(), y)],
            egui::Stroke::new(1.5, TEXT_DIM),
        );
    }

    draw_center_bug_small(&painter, rect, center_y, format!("{:.0}", altitude_ft));
}

/// Live vertical-speed indicator (VSI): a static ±2000 fpm scale (no
/// scrolling, since vertical speed has no "current altitude" reference to
/// tape against) with a needle-style bug marking the live rate. Positive
/// (climbing) shown above centre in green, negative (descending) below in
/// caution/warn depending on how steep.
fn draw_vspeed_tape(ui: &mut egui::Ui, vs_fpm: f32, height: f32) {
    let (response, painter) = ui.allocate_painter(egui::Vec2::new(56.0, height), egui::Sense::hover());
    let rect = response.rect;
    let center_y = rect.center().y;

    const VS_RANGE_FPM: f32 = 2000.0;
    // Scaled down from the full painter height so adjacent 1000 fpm ticks sit
    // closer together (a smaller vertical "throw") instead of spanning the
    // whole tall container end to end.
    let px_per_fpm = (rect.height() / 2.0 - 10.0) / VS_RANGE_FPM * 0.55;

    for mark in [-3000, -2000, -1000, 0, 1000, 2000, 3000] {
        let y = center_y - mark as f32 * px_per_fpm;
        let is_zero = mark == 0;
        let tick_len = if is_zero { 12.0 } else { 8.0 };
        painter.line_segment(
            [egui::Pos2::new(rect.right() - tick_len, y), egui::Pos2::new(rect.right(), y)],
            egui::Stroke::new(1.5, TEXT_DIM),
        );
        painter.text(egui::Pos2::new(rect.left() + 2.0, y), egui::Align2::LEFT_CENTER,
            format!("{}", mark), egui::FontId::proportional(11.0), TEXT_DIM);
    }

    // Round near-zero rates to 0 so the label doesn't flicker between -1/1
    // fpm from small physics noise while essentially level.
    let vs_display = if vs_fpm.abs() < 20.0 { 0.0 } else { vs_fpm };
    let vs_clamped = vs_display.clamp(-VS_RANGE_FPM, VS_RANGE_FPM);
    draw_center_bug_small(&painter, rect, center_y - vs_clamped * px_per_fpm, format!("{:.0}", vs_display));
}

/// Compact version of `draw_center_bug` sized for the narrower altitude/VS
/// tapes (72px window vs. the airspeed tape's original wider layout).
fn draw_center_bug_small(painter: &egui::Painter, rect: egui::Rect, center_y: f32, label: String) {
    let center_x = rect.center().x;
    let (w, h, tip) = (56.0, 24.0, 7.0);
    let points = vec![
        egui::Pos2::new(center_x - w / 2.0, center_y - h / 2.0),
        egui::Pos2::new(center_x + w / 2.0 - tip, center_y - h / 2.0),
        egui::Pos2::new(center_x + w / 2.0, center_y),
        egui::Pos2::new(center_x + w / 2.0 - tip, center_y + h / 2.0),
        egui::Pos2::new(center_x - w / 2.0, center_y + h / 2.0),
    ];
    painter.add(egui::Shape::convex_polygon(points, egui::Color32::from_rgb(28, 40, 64), egui::Stroke::new(1.0, ACCENT)));
    painter.text(egui::Pos2::new(center_x - 3.0, center_y), egui::Align2::CENTER_CENTER,
        label, egui::FontId::proportional(15.0), TEXT);
}

fn draw_airspeed_tape(ui: &mut egui::Ui, speed_kt: f32) {
    let (response, painter) = ui.allocate_painter(egui::Vec2::new(56.0, 220.0), egui::Sense::hover());
    let rect = response.rect;
    let center_y = rect.center().y;
    let px_per_kt = 1.9;

    // The low-speed warning band's floor is derived from the painter's own
    // height (however far below 0 the tape can currently scroll) rather than
    // a fixed buffer, so the red never reads as hard-clipped at the 0 line
    // no matter how far the tape is showing into negative speed.
    let visible_floor_kt = speed_kt - (rect.bottom() - center_y) / px_per_kt - 20.0;

    let bands = [
        (visible_floor_kt, V_STALL_KT, WARN),
        (V_STALL_KT, V_CRUISE_KT, GOOD),
        (V_CRUISE_KT, V_NE_KT, CAUTION),
        (V_NE_KT, V_NE_KT * 1.4, WARN),
    ];
    for (lo, hi, color) in bands {
        let y_top = center_y + (speed_kt - hi) * px_per_kt;
        let y_bot = center_y + (speed_kt - lo) * px_per_kt;
        if y_bot < rect.top() || y_top > rect.bottom() { continue; }
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::Pos2::new(rect.left(), y_top.max(rect.top())),
                egui::Pos2::new(rect.left() + 8.0, y_bot.min(rect.bottom())),
            ),
            egui::CornerRadius::ZERO,
            color,
        );
    }

    let step = 10.0;
    let start = ((speed_kt - 70.0) / step).floor() * step;
    let end = start + 140.0;
    for spd in (start.max(0.0) as i32..=end as i32).step_by(step as usize) {
        let y = center_y + (speed_kt - spd as f32) * px_per_kt;
        if y < rect.top() || y > rect.bottom() { continue; }
        let is_major = spd % 20 == 0;
        let tick_len = if is_major { 10.0 } else { 6.0 };
        painter.line_segment(
            [egui::Pos2::new(rect.left() + 10.0, y), egui::Pos2::new(rect.left() + 10.0 + tick_len, y)],
            egui::Stroke::new(1.5, TEXT_DIM),
        );
        if is_major {
            painter.text(egui::Pos2::new(rect.right() - 2.0, y), egui::Align2::RIGHT_CENTER,
                format!("{}", spd), egui::FontId::proportional(11.0), TEXT);
        }
    }

    draw_center_bug_small(&painter, rect, center_y, format!("{:.0}", speed_kt));
}

fn draw_heading_compass(ui: &mut egui::Ui, heading: f32) {
    let (response, painter) = ui.allocate_painter(egui::Vec2::new(112.0, 112.0), egui::Sense::hover());
    let center = response.rect.center();
    let radius = 34.0;

    painter.circle_stroke(center, radius, egui::Stroke::new(1.5, BORDER));

    for (label, angle) in [("N", 0.0), ("E", 90.0), ("S", 180.0), ("W", 270.0)] {
        let rad = (angle - heading - 90.0).to_radians();
        let pos = egui::Pos2::new(center.x + (radius + 14.0) * rad.cos(), center.y + (radius + 14.0) * rad.sin());
        painter.text(pos, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), TEXT);
    }

    for deg in (0..360).step_by(30) {
        let rad = (deg as f32 - heading - 90.0).to_radians();
        let is_cardinal = deg % 90 == 0;
        let inner = if is_cardinal { radius - 9.0 } else { radius - 5.0 };
        let p1 = egui::Pos2::new(center.x + inner * rad.cos(), center.y + inner * rad.sin());
        let p2 = egui::Pos2::new(center.x + radius * rad.cos(), center.y + radius * rad.sin());
        painter.line_segment([p1, p2], egui::Stroke::new(1.0, TEXT_DIM));
    }

    // Fixed aircraft-heading bug at the top of the ring.
    painter.line_segment(
        [egui::Pos2::new(center.x, center.y - radius - 6.0), egui::Pos2::new(center.x, center.y - radius + 6.0)],
        egui::Stroke::new(3.0, ACCENT),
    );
}

/// Bright annunciator-style colors: dim when off (just outline/text), lit
/// with a filled bright background when on, matching the "brightness of the
/// outline/text vs. filled" look of real cockpit switch panels.
const NAV_LIT:    egui::Color32 = egui::Color32::from_rgb(90, 200, 130);
const STROBE_LIT: egui::Color32 = egui::Color32::from_rgb(230, 230, 230);
const LAND_LIT:   egui::Color32 = egui::Color32::from_rgb(250, 220, 90);
const BRAKE_LIT:  egui::Color32 = egui::Color32::from_rgb(235, 90, 90);
const PARK_LIT:   egui::Color32 = egui::Color32::from_rgb(235, 90, 90);
const OFF_FILL:   egui::Color32 = egui::Color32::from_rgb(15, 19, 28);
const OFF_TEXT:   egui::Color32 = egui::Color32::from_rgb(90, 100, 120);

/// Grid of bright toggle/annunciator indicators: NAV, STROBE, and LANDING
/// lights (clickable — flips the same `LightTimers` fields the exterior
/// lights read from). The IGNITION status light lives in the ENGINE panel,
/// and BRAKE/PARK have their own panel — see `draw_brake_panel`.
/// Toggled switches fill with their lit color when on; read-only ones just
/// reflect live state and aren't clickable.
fn draw_switch_panel(ui: &mut egui::Ui, lights: &mut LightTimers) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        let content_width = 3.0 * 40.0 + 2.0 * 3.0;
        ui.allocate_ui(egui::Vec2::new(content_width, 26.0), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(|ui| {
                light_switch(ui, "NAV", &mut lights.nav_on, NAV_LIT);
                light_switch(ui, "STRB", &mut lights.strobe_on, STROBE_LIT);
                light_switch(ui, "LAND", &mut lights.landing_light_on, LAND_LIT);
            });
        });
    });
}

/// BRAKES panel: a read-only BRAKE annunciator (lit whenever the wheel
/// brakes are actually applied, from any source — B, auto parking brake, or
/// PARK) next to a clickable PARK toggle that latches the brakes on
/// regardless of throttle/speed via `PlaneState::parking_brake_set`.
fn draw_brake_panel(ui: &mut egui::Ui, state: &mut PlaneState) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        let content_width = 58.0 + 40.0 + 3.0;
        ui.allocate_ui(egui::Vec2::new(content_width, 26.0), |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.horizontal(|ui| {
                light_indicator(ui, "BRAKE", state.braking, BRAKE_LIT);
                light_switch(ui, "PARK", &mut state.parking_brake_set, PARK_LIT);
            });
        });
    });
}

/// Alpha used for a lit switch's fill — the glow is a translucent tint over
/// the panel background rather than a solid block, so the bright outline
/// and text (still fully opaque) read as the "on" cue, not the fill alone.
const LIT_FILL_ALPHA: u8 = 70;

fn lit_fill(color: egui::Color32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), LIT_FILL_ALPHA)
}

/// A single clickable rectangular switch. Semi-transparent tint of
/// `lit_color` plus a bright outline/text when `*on`, otherwise a dark fill
/// with dim outline/text — clicking anywhere on the rectangle flips it.
fn light_switch(ui: &mut egui::Ui, label: &str, on: &mut bool, lit_color: egui::Color32) {
    let size = egui::Vec2::new(40.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
    }

    let painter = ui.painter();
    let (fill, text_color, stroke_color) = if *on {
        (lit_fill(lit_color), lit_color, lit_color)
    } else {
        (OFF_FILL, OFF_TEXT, OFF_TEXT)
    };
    painter.rect_filled(rect, egui::CornerRadius::from(3u8), fill);
    painter.rect_stroke(rect, egui::CornerRadius::from(3u8), egui::Stroke::new(1.0, stroke_color), egui::StrokeKind::Outside);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(10.0), text_color);
}

/// A read-only annunciator light — same visual language as `light_switch`
/// but not clickable, just reflecting `on`'s current state.
fn light_indicator(ui: &mut egui::Ui, label: &str, on: bool, lit_color: egui::Color32) {
    let size = egui::Vec2::new(58.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());

    let painter = ui.painter();
    let (fill, text_color, stroke_color) = if on {
        (lit_fill(lit_color), lit_color, lit_color)
    } else {
        (OFF_FILL, OFF_TEXT, OFF_TEXT)
    };
    painter.rect_filled(rect, egui::CornerRadius::from(3u8), fill);
    painter.rect_stroke(rect, egui::CornerRadius::from(3u8), egui::Stroke::new(1.0, stroke_color), egui::StrokeKind::Outside);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(10.0), text_color);
}

/// Arc tachometer — dial position is engine RPM against the propeller's
/// governed redline (`prop_max_rps`), not throttle lever position, since
/// those two only agree at steady state.
fn draw_engine_gauge(ui: &mut egui::Ui, root: &AircraftRoot, max_rps: f32) {
    let (response, painter) = ui.allocate_painter(egui::Vec2::new(120.0, 96.0), egui::Sense::hover());
    let rect = response.rect;
    let radius = 58.0;
    let center = egui::Pos2::new(rect.center().x, rect.top() + radius + 2.0);

    let rpm = root.engine_rps * 60.0;
    let max_rpm = max_rps * 60.0;

    let start_angle = std::f32::consts::PI;
    let sweep = std::f32::consts::PI; // half-circle dial, 0..max_rpm
    let rpm_to_angle = |r: f32| start_angle - sweep * (r / max_rpm).clamp(0.0, 1.0);

    // Arc track, redline in the top 10%.
    let segments = 60;
    for i in 0..segments {
        let t1 = i as f32 / segments as f32;
        let t2 = (i + 1) as f32 / segments as f32;
        let a1 = rpm_to_angle(t1 * max_rpm);
        let a2 = rpm_to_angle(t2 * max_rpm);
        let inner = radius - 7.0;
        let poly = vec![
            egui::Pos2::new(center.x + radius * a1.cos(), center.y - radius * a1.sin()),
            egui::Pos2::new(center.x + radius * a2.cos(), center.y - radius * a2.sin()),
            egui::Pos2::new(center.x + inner * a2.cos(), center.y - inner * a2.sin()),
            egui::Pos2::new(center.x + inner * a1.cos(), center.y - inner * a1.sin()),
        ];
        let color = if t1 > 0.9 { WARN } else { egui::Color32::from_rgb(28, 40, 64) };
        painter.add(egui::Shape::convex_polygon(poly, color, egui::Stroke::NONE));
    }

    // Hundreds-of-RPM tick marks.
    let tick_step = 200.0;
    let mut r = 0.0;
    while r <= max_rpm + 1.0 {
        let angle = rpm_to_angle(r);
        let is_major = (r / 1000.0).fract().abs() < 1e-3;
        let tick_start = radius - if is_major { 12.0 } else { 8.0 };
        let p1 = egui::Pos2::new(center.x + tick_start * angle.cos(), center.y - tick_start * angle.sin());
        let p2 = egui::Pos2::new(center.x + radius * angle.cos(), center.y - radius * angle.sin());
        painter.line_segment([p1, p2], egui::Stroke::new(1.0, TEXT_DIM));
        r += tick_step;
    }

    // RPM needle.
    let needle_angle = rpm_to_angle(rpm);
    let needle_len = radius - 14.0;
    let tip = egui::Pos2::new(center.x + needle_len * needle_angle.cos(), center.y - needle_len * needle_angle.sin());
    painter.line_segment([center, tip], egui::Stroke::new(3.0, ACCENT));
    painter.circle_filled(center, 4.0, ACCENT);

    painter.text(
        egui::Pos2::new(center.x, center.y + 8.0),
        egui::Align2::CENTER_TOP,
        format!("{:.0} RPM", rpm),
        egui::FontId::proportional(16.0),
        TEXT,
    );
}

const THROTTLE_HANDLE: egui::Color32 = egui::Color32::from_rgb(30, 30, 30);
const MIXTURE_HANDLE: egui::Color32 = egui::Color32::from_rgb(200, 45, 45);

const FLAP_MAX_DEG: f32 = 30.0;

/// Throttle and mixture quadrant: two vertical rails with draggable lever
/// handles, styled after a real light-aircraft quadrant (black throttle
/// knob, red mixture knob). Forward/up = pushed in = more power/richer.
fn draw_throttle_mixture_levers(ui: &mut egui::Ui, root: &mut AircraftRoot) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        lever(ui, "THR", &mut root.throttle_percent, 0.0..=1.0, THROTTLE_HANDLE, None,
            Some(|t| format!("{:.0}%", t * 100.0)));

        lever(ui, "MIX", &mut root.mixture, 0.0..=1.0, MIXTURE_HANDLE, None,
            Some(|t| format!("{:.0}%", t * 100.0)));
    });
}

/// Flap lever. Inverted: pushed up (rail t=1) means retracted (0°), pulled
/// down means fully extended (30°), matching the throttle/mixture "up =
/// less drag/more power" convention on the adjacent panel. Snapped to 10°
/// steps to match the C172 notch detents (see airplane_controller.rs's
/// FLAP_NOTCHES_DEG) rather than a continuous lever, since this plane's
/// flaps are notch-only.
fn draw_flap_lever(ui: &mut egui::Ui, root: &mut AircraftRoot) {
    let mut flap_deg = root.flap_target.to_degrees();
    lever(ui, "FLAP", &mut flap_deg, FLAP_MAX_DEG..=0.0, ACCENT, Some(10.0),
        Some(|t| format!("{:.0}°", FLAP_MAX_DEG * (1.0 - t))));
    root.flap_target = flap_deg.to_radians();
}

const TRIM_HANDLE: egui::Color32 = egui::Color32::from_rgb(230, 200, 60);
const TRIM_MAX_DEG: f32 = 10.0;

/// Elevator trim lever, stepped in single degrees, bounded to ±10° (this
/// sim's trim range is otherwise unbounded in `FlightModelConfig` — the
/// debug menu's raw slider goes to ±0.5 rad ≈ ±28.6° — but the cockpit
/// lever caps at a realistic ±10° for hand-flying trim adjustments).
/// `elevator_trim` is in radians (positive = nose up); the lever itself
/// works in whole degrees and converts at the boundary.
fn draw_trim_lever(ui: &mut egui::Ui, elevator_trim: &mut f32) {
    let mut trim_deg = elevator_trim.to_degrees().clamp(-TRIM_MAX_DEG, TRIM_MAX_DEG);
    lever(ui, "TRIM", &mut trim_deg, -TRIM_MAX_DEG..=TRIM_MAX_DEG, TRIM_HANDLE, Some(1.0),
        Some(|t| format!("{:+.0}°", -TRIM_MAX_DEG + t * (2.0 * TRIM_MAX_DEG)))); // t is the 0..1 rail fraction; this maps it back to the -10..10 label
    *elevator_trim = trim_deg.to_radians();
}

/// A single vertical rail-and-handle slider. `value` is mapped linearly over
/// `range` (start = bottom of rail, end = top — pass a reversed range to
/// invert the direction). The handle is dragged directly rather than exposed
/// as a numeric egui::Slider, so it reads as a physical lever. `label_fn`, if
/// given, receives the 0..1 rail fraction (bottom..top) to format a readout
/// shown below the rail.
///
/// Wrapped in a fixed-width `allocate_ui` because a bare `vertical_centered`
/// nested inside the caller's `ui.horizontal` would request unbounded width
/// from its parent, which both broke short labels like "MIX" (wrapped to one
/// character per line) and could blow out the whole window's auto-sized
/// width — the same bug hit the loadout gauges in plane_menu.rs.
fn lever(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    handle_color: egui::Color32,
    step: Option<f32>,
    label_fn: Option<impl Fn(f32) -> String>,
) {
    ui.allocate_ui(egui::vec2(28.0, 146.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(label).size(10.0).color(TEXT_DIM));

            let rail_size = egui::Vec2::new(18.0, 108.0);
            let (rect, response) = ui.allocate_exact_size(rail_size, egui::Sense::click_and_drag());

            let (start, end) = (*range.start(), *range.end());
            if let Some(pos) = response.interact_pointer_pos() {
                let t = 1.0 - ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                let mut new_value = start + t * (end - start);
                if let Some(step) = step {
                    new_value = (new_value / step).round() * step;
                }
                *value = new_value;
            }

            let painter = ui.painter();
            let rail_x = rect.center().x;

            // Rail slot.
            painter.line_segment(
                [egui::Pos2::new(rail_x, rect.top() + 6.0), egui::Pos2::new(rail_x, rect.bottom() - 6.0)],
                egui::Stroke::new(4.0, egui::Color32::from_rgb(10, 13, 20)),
            );
            painter.line_segment(
                [egui::Pos2::new(rail_x, rect.top() + 6.0), egui::Pos2::new(rail_x, rect.bottom() - 6.0)],
                egui::Stroke::new(1.0, BORDER),
            );

            let t = ((*value - start) / (end - start)).clamp(0.0, 1.0);
            let handle_y = rect.bottom() - 6.0 - t * (rect.height() - 12.0);
            let handle_center = egui::Pos2::new(rail_x, handle_y);

            // Lever handle: a rounded knob with a short grip bar, pushed forward
            // (up the rail) at high values like a real quadrant lever.
            painter.rect_filled(
                egui::Rect::from_center_size(handle_center, egui::Vec2::new(20.0, 12.0)),
                egui::CornerRadius::from(3u8),
                handle_color,
            );
            painter.rect_stroke(
                egui::Rect::from_center_size(handle_center, egui::Vec2::new(20.0, 12.0)),
                egui::CornerRadius::from(3u8),
                egui::Stroke::new(1.0, egui::Color32::BLACK),
                egui::StrokeKind::Outside,
            );

            // Reserve the readout row's height even when there's no label
            // (the flap lever), so its panel lines up with siblings that do
            // show one (throttle/mixture) at the same overall window height.
            match label_fn {
                Some(label_fn) => { ui.label(egui::RichText::new(label_fn(t)).size(10.0).color(TEXT_DIM)); }
                None => { ui.add_space(ui.text_style_height(&egui::TextStyle::Body)); }
            }
        });
    });
}

// ISA (International Standard Atmosphere) troposphere model, used to derive
// a barometric reading that actually varies with altitude. The sim's
// `air_density` config has no altitude dependence of its own (it's a fixed
// scalar fed straight into the aero/drag forces), so it's treated here as
// the sea-level reference density (equivalent to setting local QNH) and the
// lapse-rate formula does the rest as the aircraft climbs/descends.
const ISA_LAPSE_RATE: f32 = 0.0065; // K/m
const ISA_SEA_LEVEL_TEMP_K: f32 = 288.15; // 15°C
const SPECIFIC_GAS_CONSTANT: f32 = 287.05; // J/(kg·K), dry air
const PA_TO_INHG: f32 = 1.0 / 3386.39;

/// Returns (density kg/m³, pressure inHg) at `altitude_m` above the sim's
/// spawn/sea-level reference, given the sea-level density from config.
fn baro_at_altitude(sea_level_density: f32, altitude_m: f32) -> (f32, f32) {
    let temp_k = (ISA_SEA_LEVEL_TEMP_K - ISA_LAPSE_RATE * altitude_m).max(1.0);
    let temp_ratio = temp_k / ISA_SEA_LEVEL_TEMP_K;
    // Standard barometric formula exponent for the troposphere (g/(R*L) ≈ 5.2559).
    let exponent = 5.2559;
    let pressure_ratio = temp_ratio.powf(exponent);
    let sea_level_pressure_pa = sea_level_density * SPECIFIC_GAS_CONSTANT * ISA_SEA_LEVEL_TEMP_K;
    let pressure_pa = sea_level_pressure_pa * pressure_ratio;
    let density = pressure_pa / (SPECIFIC_GAS_CONSTANT * temp_k);
    (density, pressure_pa * PA_TO_INHG)
}

fn draw_baro_readout(ui: &mut egui::Ui, sea_level_density: f32, altitude_m: f32) {
    let (density, pressure_inhg) = baro_at_altitude(sea_level_density, altitude_m);

    ui.label(egui::RichText::new(format!("{:.2} inHg", pressure_inhg)).size(14.0).color(TEXT));
    ui.label(egui::RichText::new(format!("ρ {:.3} kg/m³", density)).size(11.0).color(TEXT_DIM));
}
