use bevy::{diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin}, prelude::*};

use crate::{FpsText, camera::{CameraMode, FreeCam}, fog::FogEnabled, physics::aircraft_physics::AircraftRoot, plane::{Airplane, PlaneState}};

/// Shared debug overlay. Any system can push entries into `entries` each frame;
/// `render_debug_hud` turns the vec into the on-screen text.
/// Entries are cleared at the start of each populate pass, so the content
/// always reflects the current frame — no stale values accumulate.
#[derive(Resource, Default)]
pub struct DebugHud {
    /// Each entry is a (label, value) pair rendered as "LABEL: value".
    pub entries: Vec<(&'static str, String)>,
}

/// Marker for the single text entity that displays the debug overlay.
#[derive(Component)]
pub struct DebugHudText;

/// Reads the current entries in `DebugHud` and writes them to the overlay text.
/// Must run after whatever system populated `DebugHud` that frame.
pub fn render_debug_hud(
    hud: Res<DebugHud>,
    mut query: Query<&mut Text, With<DebugHudText>>,
) {
    let Ok(mut text) = query.single_mut() else { return };

    **text = hud.entries
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
}


pub fn update_fps(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<FpsText>>,
) {
    let Ok(mut text) = query.single_mut() else { return };
    if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS)
        && let Some(value) = fps.smoothed() {
            **text = format!("FPS: {:.0}", value);
        }
}

pub fn populate_debug_hud(
    mut hud: ResMut<DebugHud>,
    mode: Res<CameraMode>,
    fog: Res<FogEnabled>,
    cam_query: Query<&Transform, With<FreeCam>>,
    plane_query: Query<(&Transform, &PlaneState, &AircraftRoot), With<Airplane>>,
) {
    hud.entries.clear();

    hud.entries.push(("CAM", match &*mode {
        CameraMode::Free  => "FREE".into(),
        CameraMode::Orbit => "ORBIT".into(),
        CameraMode::Chase => "CHASE".into(),
    }));
    hud.entries.push(("FOG", if fog.0 { "ON  [1]" } else { "OFF [1]" }.into()));

    if let Ok(tf) = cam_query.single() {
        let p = tf.translation;
        hud.entries.push(("POS", format!("X={:.1}  Y={:.1}  Z={:.1}", p.x, p.y, p.z)));
    }

    if let Ok((tf, state, root)) = plane_query.single() {
        let (yaw, pitch, roll) = tf.rotation.to_euler(EulerRot::YXZ);
        hud.entries.push(("SPD",    format!("{:.1} m/s", state.speed)));
        hud.entries.push(("ALT",    format!("{:.1} m",   tf.translation.y)));
        hud.entries.push(("GND",    if state.on_ground { "ON GROUND" } else { "AIRBORNE" }.into()));
        hud.entries.push(("BRK",    if state.braking { "ON  [B]" } else { "OFF [B]" }.into()));
        hud.entries.push(("THR",    format!("{:.0}%",    root.throttle_percent * 100.0)));
        hud.entries.push(("RPM",    format!("{:.0}",     root.engine_rps * 60.0)));
        hud.entries.push(("FLAPS",  format!("{:.0} degrees",    root.flap_setting.to_degrees())));
        // Actual thrust from the spooled engine RPM, not the raw throttle.
        hud.entries.push(("THRUST", format!("{:.0} N",   state.thrust)));
        hud.entries.push(("DRAG",   format!("{:.0} N",    state.drag)));
        hud.entries.push(("  SURF", format!("{:.0} N",    state.drag_surface)));
        hud.entries.push(("  FUSE", format!("{:.0} N",    state.drag_fuselage)));
        hud.entries.push(("LIFT",   format!("{:.0}%",    state.lift_pct * 100.0)));
        hud.entries.push(("PITCH",  format!("{:.1}",    pitch.to_degrees())));
        hud.entries.push(("ROLL",   format!("{:.1}",    roll.to_degrees())));
        hud.entries.push(("YAW",    format!("{:.1}",    yaw.to_degrees())));
    }
}