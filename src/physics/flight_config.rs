//! Tunable flight-model constants exposed as a Bevy `Resource`.
//!
//! All physics and controller systems read from [`FlightModelConfig`] instead
//! of using hard-coded `const` values, so the debug menu can mutate them at
//! runtime without a recompile.

use bevy::prelude::*;

use super::aero_surface_config::AeroSurfaceConfig;

/// All tunable constants for the flight model.
///
/// Grouped by subsystem so the debug panel can display them in logical sections.
/// `Default` reflects the original hand-tuned values and can be restored at any
/// time with the "Reset to defaults" button in the debug panel.
#[derive(Resource, Clone)]
pub struct FlightModelConfig {
    // --- Control surfaces --------------------------------------------------
    /// Elevator deflection scale applied to the pitch input axis (0–1 range).
    pub pitch_sensitivity: f32,
    /// Aileron deflection scale applied to the roll input axis (0–1 range).
    pub roll_sensitivity: f32,
    /// Rudder deflection scale applied to the yaw input axis (0–1 range).
    pub yaw_sensitivity: f32,
    /// Throttle change rate in percent per second (e.g. 0.5 = full range in 2 s).
    pub throttle_rate: f32,
    /// Servo first-order time constant (seconds). Smaller = snappier response.
    pub servo_tau: f32,
    /// Static elevator trim offset (radians). Positive pitches nose up.
    pub elevator_trim: f32,

    // --- Speed envelope ----------------------------------------------------
    /// Vs — clean stall speed (m/s).  Below this, control authority ramps to 0.
    pub stall_speed: f32,
    /// Vno — max structural cruise speed (m/s).  Authority is full below this.
    pub authority_limit_speed: f32,
    /// Vne — never-exceed speed (m/s).  Authority floor is reached here.
    pub vne_speed: f32,
    /// Authority fraction retained at Vne and above (0–1).
    pub vne_authority: f32,

    // --- Aerodynamics ------------------------------------------------------
    /// Per-axis rotational damping (X=roll, Y=yaw, Z=pitch).
    /// Torque = -aero_damp * airspeed * ang_vel per axis.
    pub aero_damp: Vec3,
    /// Fuselage form drag as Cd·A per *body* axis (X=side/flank, Y=belly/top,
    /// Z=nose/forward). Drag = -0.5·ρ·v·|v|·CdA per axis, applied at the CoM.
    /// The nose is streamlined (small Z); the flanks and belly are not, so a
    /// high-AoA pull or a skid broadsides the body and bleeds energy. Without
    /// this the body has no drag and the aircraft flies like a steerable
    /// velocity vector.
    pub fuselage_drag: Vec3,
    /// Air density (kg/m³). Standard sea-level ISA = 1.225.
    pub air_density: f32,
    /// Gravitational acceleration (m/s²).
    pub gravity: f32,
    /// Fraction of the time-step used for the trapezoidal velocity prediction.
    /// 0.5 = midpoint (second-order accurate), 0 = forward-Euler.
    pub prediction_fraction: f32,

    // --- Flight assists ----------------------------------------------------
    /// Roll auto-level strength. Torque = -coeff * bank_angle * airspeed.
    pub auto_level_strength: f32,
    /// Bank-to-turn strength. Yaw torque = coeff * bank_angle * airspeed.
    pub bank_turn_strength: f32,

    // --- Engine ------------------------------------------------------------
    /// Maximum static thrust at full throttle (Newtons).
    pub thrust_max: f32,

    // --- Visual ------------------------------------------------------------
    /// Local-space offset (×0.1 → metres) of the GLTF mesh relative to the
    /// physics origin. Purely cosmetic — lets the model be lined up with the
    /// simulated wing/tail positions without touching the flight model.
    pub model_offset: Vec3,
    /// Local-space center of mass (×0.1 → metres). +Z forward (nose), +Y up.
    /// Moving aft shortens the static margin (livelier pitch); moving down
    /// increases pendulum roll stability. Synced to the rigid body at runtime.
    pub center_of_mass: Vec3,

    // --- Aerodynamic surfaces ---------------------------------------------
    // Per-surface geometry and stall behaviour. `spawn_aircraft` builds the
    // surfaces from these at startup, and `apply_config_to_entities` pushes
    // any later edit back onto the matching live surfaces (keyed by control
    // input type), so every wing/tail parameter is tunable at runtime.
    /// Main wing panels (carry the flaps — `ControlInputType::Flap`).
    pub wing: AeroSurfaceConfig,
    /// Outboard aileron panels (`ControlInputType::Roll`).
    pub aileron: AeroSurfaceConfig,
    /// Horizontal stabilizer / elevator (`ControlInputType::Pitch`).
    pub elevator: AeroSurfaceConfig,
    /// Vertical stabilizer / rudder (`ControlInputType::Yaw`).
    pub rudder: AeroSurfaceConfig,
    /// Fuselage lift surfaces — small non-control panels at the body.
    pub body_lift: AeroSurfaceConfig,
}

impl Default for FlightModelConfig {
    fn default() -> Self {
        Self {
            pitch_sensitivity:    0.50,
            roll_sensitivity:     0.25,
            yaw_sensitivity:      0.40,
            throttle_rate:        0.5,
            servo_tau:            0.15,
  
            elevator_trim:        0.02,

            stall_speed:          28.0,
            authority_limit_speed: 66.0,
            vne_speed:            84.0,
            vne_authority:        0.35,

            aero_damp:            Vec3::new(1.0, 9.0, 2.5),

            fuselage_drag:        Vec3::new(60.0, 10.0, 0.15),
            air_density:          1.2,
            gravity:              9.81,
            prediction_fraction:  0.5,

            auto_level_strength:  18.0,
            bank_turn_strength:   12.0,

            thrust_max:           2_600.0,

            model_offset:         Vec3::new(0.0, -12.0, 11.0),
            center_of_mass:       Vec3::new(0.0, 1.0, 15.0),

            // Main wing: ~16.2 m² per panel area, full-wing AR ≈ 7, 20% flaps.
            wing: AeroSurfaceConfig {
                flap_fraction: 0.2,
                span: 3.65,
                aspect_ratio: 7.0,
                ..AeroSurfaceConfig::default()
            },
            // Ailerons: outer ~28% of span, 35% chord, ~1.5 m each.
            aileron: AeroSurfaceConfig {
                flap_fraction: 0.35,
                span: 1.5,
                aspect_ratio: 7.0,
                ..AeroSurfaceConfig::default()
            },
            // Tail sized to a real C172: horizontal ≈ 3.4 m², vertical ≈ 2.2 m².
            elevator: AeroSurfaceConfig::stabilizer(3.4, 1.0),
            rudder:   AeroSurfaceConfig::stabilizer(2.2, 0.8),
            // Small fuselage lift panels (no flaps).
            body_lift: AeroSurfaceConfig {
                flap_fraction: 0.0,
                span: 0.5,
                aspect_ratio: 0.5 / 1.57,
                ..AeroSurfaceConfig::default()
            },
        }
    }
}
