//! Exterior aircraft lights for the Cessna 172.
//!
//! Four systems, all automatic — no checklist required:
//!   - **Nav lights** (red L wingtip, green R wingtip, white tail): always on.
//!   - **Strobes** (white, all three positions): pulse at `strobe_period`.
//!   - **Beacon** (red belly): pulses at `beacon_period` while the engine runs.
//!   - **Landing light** (white spotlight on the nose): toggled with **L**.
//!
//! All light positions and intensities are exposed as [`LightsConfig`] inside
//! [`FlightModelConfig`] and are editable live from the F3 "Lights" debug panel.
//! [`apply_lights_to_entities`] pushes changes from that panel onto the world.

use bevy::prelude::*;

use crate::camera::PIXEL_LAYER;
use crate::physics::aircraft_physics::{AircraftRoot, EngineState, ROOT_SCALE};
use crate::physics::flight_config::FlightModelConfig;
use crate::plane::Airplane;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct AircraftLightsPlugin;

impl Plugin for AircraftLightsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
            toggle_landing_light,
            animate_strobes,
            animate_beacon,
            apply_lights_to_entities,
        ));
    }
}

// ---------------------------------------------------------------------------
// Marker components
// ---------------------------------------------------------------------------

/// Red nav light — left wingtip.
#[derive(Component)] pub struct NavLightLeft;
/// Green nav light — right wingtip.
#[derive(Component)] pub struct NavLightRight;
/// White nav light — tail.
#[derive(Component)] pub struct NavLightTail;

/// White anti-collision strobe — left wingtip.
#[derive(Component)] pub struct StrobeLeft;
/// White anti-collision strobe — right wingtip.
#[derive(Component)] pub struct StrobeRight;
/// White anti-collision strobe — tail.
#[derive(Component)] pub struct StrobeTail;

/// Red anti-collision beacon — belly.
#[derive(Component)] pub struct Beacon;

/// White landing spotlight — nose.
#[derive(Component)] pub struct LandingLight;

// ---------------------------------------------------------------------------
// Timing state on the aircraft root entity
// ---------------------------------------------------------------------------

/// Per-aircraft light timing. Added to the aircraft root entity alongside
/// [`AircraftRoot`] so it survives the entity across scene reloads.
#[derive(Component)]
pub struct LightTimers {
    /// Accumulated time within the current strobe period (seconds).
    pub strobe_t: f32,
    /// Accumulated time within the current beacon period (seconds).
    pub beacon_t: f32,
    /// Whether the landing light is currently on.
    pub landing_light_on: bool,
}

impl Default for LightTimers {
    fn default() -> Self {
        Self { strobe_t: 0.0, beacon_t: 0.0, landing_light_on: true }
    }
}

// ---------------------------------------------------------------------------
// Spawn helper — called from `spawn_aircraft`
// ---------------------------------------------------------------------------

/// Spawns all exterior light entities as children of `parent` (the aircraft
/// root). Returns them so the caller can pass them to `add_children`.
pub fn spawn_aircraft_lights(
    commands: &mut Commands,
    cfg: &FlightModelConfig,
) -> [Entity; 8] {
    let lc = &cfg.lights;
    let s = ROOT_SCALE; // ×0.1: local units → metres

    // Nav left (red)
    let nav_l = commands.spawn((
        PointLight {
            color: Color::srgb(1.0, 0.05, 0.05),
            intensity: lc.nav_intensity,
            radius: 0.05,
            range: 80.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.nav_left_pos * s),
        NavLightLeft,
        PIXEL_LAYER,
    )).id();

    // Nav right (green)
    let nav_r = commands.spawn((
        PointLight {
            color: Color::srgb(0.05, 1.0, 0.15),
            intensity: lc.nav_intensity,
            radius: 0.05,
            range: 80.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.nav_right_pos * s),
        NavLightRight,
        PIXEL_LAYER,
    )).id();

    // Nav tail (white)
    let nav_t = commands.spawn((
        PointLight {
            color: Color::WHITE,
            intensity: lc.nav_intensity,
            radius: 0.05,
            range: 80.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.nav_tail_pos * s),
        NavLightTail,
        PIXEL_LAYER,
    )).id();

    // Strobe left (white, starts off)
    let str_l = commands.spawn((
        PointLight {
            color: Color::WHITE,
            intensity: 0.0,
            radius: 0.1,
            range: 300.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.strobe_left_pos * s),
        StrobeLeft,
        PIXEL_LAYER,
    )).id();

    // Strobe right (white, starts off)
    let str_r = commands.spawn((
        PointLight {
            color: Color::WHITE,
            intensity: 0.0,
            radius: 0.1,
            range: 300.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.strobe_right_pos * s),
        StrobeRight,
        PIXEL_LAYER,
    )).id();

    // Strobe tail (white, starts off)
    let str_t = commands.spawn((
        PointLight {
            color: Color::WHITE,
            intensity: 0.0,
            radius: 0.1,
            range: 300.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.strobe_tail_pos * s),
        StrobeTail,
        PIXEL_LAYER,
    )).id();

    // Beacon (red, belly, starts off)
    let beacon = commands.spawn((
        PointLight {
            color: Color::srgb(1.0, 0.05, 0.05),
            intensity: 0.0,
            radius: 0.1,
            range: 150.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(lc.beacon_pos * s),
        Beacon,
        PIXEL_LAYER,
    )).id();

    // Landing light (white spotlight, nose-forward, starts off)
    let pitch_rad = lc.landing_pitch_deg.to_radians();
    let landing = commands.spawn((
        SpotLight {
            color: Color::srgb(1.0, 0.98, 0.92),
            intensity: 0.0,
            range: 600.0,
            radius: 0.1,
            shadows_enabled: false,
            outer_angle: lc.landing_outer_deg.to_radians(),
            inner_angle: lc.landing_inner_deg.to_radians(),
            ..default()
        },
        // SpotLight shines along local -Z; nose is +Z in body frame, so rotate
        // 180° about Y to flip it forward, then pitch down by landing_pitch_deg.
        Transform::from_translation(lc.landing_pos * s)
            .with_rotation(
                Quat::from_rotation_y(std::f32::consts::PI)
                    * Quat::from_rotation_x(pitch_rad),
            ),
        LandingLight,
        PIXEL_LAYER,
    )).id();

    [nav_l, nav_r, nav_t, str_l, str_r, str_t, beacon, landing]
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Toggle landing light with **L**.
pub fn toggle_landing_light(
    keys: Res<ButtonInput<KeyCode>>,
    mut timers_q: Query<&mut LightTimers, With<Airplane>>,
) {
    if !keys.just_pressed(KeyCode::KeyN) { return; }
    for mut t in &mut timers_q {
        t.landing_light_on = !t.landing_light_on;
    }
}

/// Drive strobe lights: a brief bright flash once per `strobe_period`.
#[allow(clippy::type_complexity)]
pub fn animate_strobes(
    time: Res<Time>,
    cfg: Res<FlightModelConfig>,
    mut timers_q: Query<&mut LightTimers, With<Airplane>>,
    mut str_l_q: Query<&mut PointLight, (With<StrobeLeft>, Without<StrobeRight>, Without<StrobeTail>)>,
    mut str_r_q: Query<&mut PointLight, (With<StrobeRight>, Without<StrobeLeft>, Without<StrobeTail>)>,
    mut str_t_q: Query<&mut PointLight, (With<StrobeTail>, Without<StrobeLeft>, Without<StrobeRight>)>,
) {
    let lc = &cfg.lights;
    for mut t in &mut timers_q {
        t.strobe_t = (t.strobe_t + time.delta_secs()) % lc.strobe_period;
        let on = t.strobe_t < lc.strobe_on_time;
        let intensity = if on { lc.strobe_intensity } else { 0.0 };
        for mut l in &mut str_l_q { l.intensity = intensity; }
        for mut l in &mut str_r_q { l.intensity = intensity; }
        for mut l in &mut str_t_q { l.intensity = intensity; }
    }
}

/// Drive beacon: on while engine is running, pulses at `beacon_period`.
pub fn animate_beacon(
    time: Res<Time>,
    cfg: Res<FlightModelConfig>,
    mut aircraft_q: Query<(&AircraftRoot, &mut LightTimers), With<Airplane>>,
    mut beacon_q: Query<&mut PointLight, With<Beacon>>,
) {
    let lc = &cfg.lights;
    for (root, mut t) in aircraft_q.iter_mut() {
        let engine_on = root.engine_state == EngineState::Running
            || root.engine_state == EngineState::Cranking;
        if engine_on {
            t.beacon_t = (t.beacon_t + time.delta_secs()) % lc.beacon_period;
        }
        // Smooth sine pulse while engine is on; off when engine is off.
        let intensity = if engine_on {
            let phase = (t.beacon_t / lc.beacon_period) * std::f32::consts::TAU;
            let pulse = (phase.cos() * 0.5 + 0.5).powf(3.0); // sharpen into a spike
            pulse * lc.beacon_intensity
        } else {
            0.0
        };
        for mut l in &mut beacon_q {
            l.intensity = intensity;
        }
    }
}

/// Push [`FlightModelConfig::lights`] changes onto the live light entities, and
/// drive the landing light on/off from [`LightTimers::landing_on`].
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn apply_lights_to_entities(
    cfg: Res<FlightModelConfig>,
    timers_q: Query<&LightTimers, With<Airplane>>,
    mut nav_l_q:    Query<(&mut PointLight, &mut Transform), (With<NavLightLeft>,  Without<NavLightRight>, Without<NavLightTail>, Without<StrobeLeft>, Without<StrobeRight>, Without<StrobeTail>, Without<Beacon>, Without<LandingLight>)>,
    mut nav_r_q:    Query<(&mut PointLight, &mut Transform), (With<NavLightRight>, Without<NavLightLeft>,  Without<NavLightTail>, Without<StrobeLeft>, Without<StrobeRight>, Without<StrobeTail>, Without<Beacon>, Without<LandingLight>)>,
    mut nav_t_q:    Query<(&mut PointLight, &mut Transform), (With<NavLightTail>,  Without<NavLightLeft>,  Without<NavLightRight>,Without<StrobeLeft>, Without<StrobeRight>, Without<StrobeTail>, Without<Beacon>, Without<LandingLight>)>,
    mut str_l_q:    Query<(&mut PointLight, &mut Transform), (With<StrobeLeft>,  Without<NavLightLeft>, Without<NavLightRight>, Without<NavLightTail>, Without<StrobeRight>, Without<StrobeTail>, Without<Beacon>, Without<LandingLight>)>,
    mut str_r_q:    Query<(&mut PointLight, &mut Transform), (With<StrobeRight>, Without<NavLightLeft>, Without<NavLightRight>, Without<NavLightTail>, Without<StrobeLeft>,  Without<StrobeTail>, Without<Beacon>, Without<LandingLight>)>,
    mut str_t_q:    Query<(&mut PointLight, &mut Transform), (With<StrobeTail>,  Without<NavLightLeft>, Without<NavLightRight>, Without<NavLightTail>, Without<StrobeLeft>,  Without<StrobeRight>,Without<Beacon>, Without<LandingLight>)>,
    mut beacon_q:   Query<(&mut PointLight, &mut Transform), (With<Beacon>, Without<NavLightLeft>, Without<NavLightRight>, Without<NavLightTail>, Without<StrobeLeft>, Without<StrobeRight>, Without<StrobeTail>, Without<LandingLight>)>,
    mut landing_q:  Query<(&mut SpotLight, &mut Transform), (With<LandingLight>, Without<NavLightLeft>, Without<NavLightRight>, Without<NavLightTail>, Without<StrobeLeft>, Without<StrobeRight>, Without<StrobeTail>, Without<Beacon>)>,
) {
    let lc = &cfg.lights;
    let s = ROOT_SCALE;

    // Nav intensities and positions (config-driven only)
    if cfg.is_changed() {
        for (mut l, mut tf) in &mut nav_l_q {
            l.intensity = lc.nav_intensity;
            tf.translation = lc.nav_left_pos * s;
        }
        for (mut l, mut tf) in &mut nav_r_q {
            l.intensity = lc.nav_intensity;
            tf.translation = lc.nav_right_pos * s;
        }
        for (mut l, mut tf) in &mut nav_t_q {
            l.intensity = lc.nav_intensity;
            tf.translation = lc.nav_tail_pos * s;
        }
        for (_, mut tf) in &mut str_l_q { tf.translation = lc.strobe_left_pos * s; }
        for (_, mut tf) in &mut str_r_q { tf.translation = lc.strobe_right_pos * s; }
        for (_, mut tf) in &mut str_t_q { tf.translation = lc.strobe_tail_pos * s; }
        for (_, mut tf) in &mut beacon_q  { tf.translation = lc.beacon_pos * s; }

        for (mut sl, mut tf) in &mut landing_q {
            tf.translation = lc.landing_pos * s;
            let pitch_rad = lc.landing_pitch_deg.to_radians();
            tf.rotation = Quat::from_rotation_y(std::f32::consts::PI)
                * Quat::from_rotation_x(pitch_rad);
            sl.outer_angle = lc.landing_outer_deg.to_radians();
            sl.inner_angle = lc.landing_inner_deg.to_radians();
        }
    }

    // Landing light on/off from LightTimers
    let landing_on = timers_q.iter().next().map(|t| t.landing_light_on).unwrap_or(false);
    for (mut sl, _) in &mut landing_q {
        sl.intensity = if landing_on { lc.landing_intensity } else { 0.0 };
    }
}
