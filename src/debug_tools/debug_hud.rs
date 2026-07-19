use bevy::{diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin}, prelude::*};

use crate::{FpsText, camera::{CameraMode, FreeCam}, fog::FogSettings, physics::aircraft_physics::{AircraftRoot, EngineState}, plane::{Airplane, PlaneState}};

/// Shared debug overlay. Any system can push entries into `entries` each frame;
/// the Dev Tools egui window renders the vec as a live readout.
/// Entries are cleared at the start of each populate pass, so the content
/// always reflects the current frame — no stale values accumulate.
#[derive(Resource, Default)]
pub struct DebugHud {
    /// Each entry is a (label, value) pair rendered as "LABEL: value".
    pub entries: Vec<(&'static str, String)>,
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
    fog: Res<FogSettings>,
    cam_query: Query<&Transform, With<FreeCam>>,
    plane_query: Query<(&Transform, &PlaneState, &AircraftRoot), With<Airplane>>,
) {
    hud.entries.clear();

    hud.entries.push(("CAM", match &*mode {
        CameraMode::Free  => "FREE".into(),
        CameraMode::Orbit => "ORBIT".into(),
        CameraMode::Fixed(i) => format!("FIXED[{i}]"),
    }));
    hud.entries.push(("FOG", if fog.enabled { "ON\t [1]" } else { "OFF \t[1]" }.into()));

    if let Ok(tf) = cam_query.single() {
        let p = tf.translation;
        hud.entries.push(("POS", format!("X={:.1}  Y={:.1}  Z={:.1}", p.x, p.y, p.z)));
    }

    if let Ok((tf, state, root)) = plane_query.single() {
        // 1 m/s = 1.943_844 knots (1 kt = 1 nautical mile/hr = 1852 m / 3600 s).
        hud.entries.push(("SPD",    format!("{:.1} m/s", state.speed)));
        hud.entries.push(("KTS",    format!("{:.1} kt",  state.speed * 1.943_844)));
        hud.entries.push(("ALT",    format!("{:.1} m",   tf.translation.y)));
        hud.entries.push(("GND",    if state.on_ground { "ON GROUND" } else { "AIRBORNE" }.into()));
        if state.crashed {
            hud.entries.push(("STATUS", "CRASHED".into()));
        }
        hud.entries.push(("ENGINE", match root.engine_state {
            EngineState::Off      => "OFF  \t[I]",
            EngineState::Cranking => "CRANKING...",
            EngineState::Running  => "RUNNING",
        }.into()));
        hud.entries.push(("MIXTURE",    format!("{:.0}%\t[K/L]", root.mixture * 100.0)));
        hud.entries.push(("THROTTLE",    format!("{:.0}%\t[-/+]",    root.throttle_percent * 100.0)));
        hud.entries.push(("FLAPS",  format!("{:.0}     \t[</>]",    root.flap_setting.to_degrees())));
        hud.entries.push(("BRK",    if state.braking { "ON  \t[B]" } else { "OFF \t[B]" }.into()));
        hud.entries.push(("RPM",    format!("{:.0}",     root.engine_rps * 60.0)));
        hud.entries.push(("LIFT",   format!("{:.0}%",    state.lift_pct * 100.0)));
        hud.entries.push(("THRUST", format!("{:.0} N",   state.thrust)));
        hud.entries.push(("DRAG",   format!("{:.0} N",    state.drag)));
        hud.entries.push(("  SURFACE", format!("{:.0} N",    state.drag_surface)));
        hud.entries.push(("  FUSELAGE", format!("{:.0} N",    state.drag_fuselage)));
    }
}