//! Tunable flight-model constants exposed as a Bevy `Resource`.
//!
//! All physics and controller systems read from [`FlightModelConfig`] instead
//! of using hard-coded `const` values, so the debug menu can mutate them at
//! runtime without a recompile.

use bevy::prelude::*;

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
    /// Rotational damping coefficient.  Torque = -coeff * airspeed * ang_vel.
    /// Higher values reduce pitch/roll oscillations at speed (ζ closer to 1).
    pub aero_damp: f32,
    /// Air density (kg/m³). Standard sea-level ISA = 1.225.
    pub air_density: f32,
    /// Gravitational acceleration (m/s²).
    pub gravity: f32,
    /// Fraction of the time-step used for the trapezoidal velocity prediction.
    /// 0.5 = midpoint (second-order accurate), 0 = forward-Euler.
    pub prediction_fraction: f32,

    // --- Engine ------------------------------------------------------------
    /// Maximum static thrust at full throttle (Newtons).
    pub thrust_max: f32,
}

impl Default for FlightModelConfig {
    fn default() -> Self {
        Self {
            pitch_sensitivity:    0.65,
            roll_sensitivity:     0.45,
            yaw_sensitivity:      0.30,
            throttle_rate:        0.5,
            servo_tau:            0.12,
            elevator_trim:        0.0,

            stall_speed:          28.0,
            authority_limit_speed: 66.0,
            vne_speed:            84.0,
            vne_authority:        0.35,

            aero_damp:            150.0,
            air_density:          1.2,
            gravity:              9.81,
            prediction_fraction:  0.5,

            thrust_max:           4_800.0,
        }
    }
}
