//! Tunable flight-model constants exposed as a Bevy `Resource`.
//!
//! All physics and controller systems read from [`FlightModelConfig`] instead
//! of using hard-coded `const` values, so the debug menu can mutate them at
//! runtime without a recompile.

use bevy::prelude::*;

use super::aero_surface_config::AeroSurfaceConfig;
use super::aircraft_physics::ROOT_SCALE;

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

    // --- Aerodynamics ------------------------------------------------------
    /// Per-axis *supplemental* rotational damping (X=roll, Y=yaw, Z=pitch).
    /// Torque = -aero_damp * airspeed * ang_vel per axis.
    ///
    /// The primary pitch/yaw/roll rate damping already emerges from the aero
    /// surfaces: when the body rotates, each surface's `ang_vel × rel_pos`
    /// airflow changes its local AoA and produces a restoring moment. This term
    /// is only a small numerical top-up on top of that — keep it low so it does
    /// not double-count the physical damping.
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

    // --- Landing gear ------------------------------------------------------
    // Spring-damper suspension feel, shared by every strut. Geometry (wheel
    // positions) lives in `landing_gear.rs`; these set how the gear responds.
    /// Suspension stiffness per strut (N/m). Higher = firmer, less squat under
    /// load; too high makes touchdown jittery.
    pub gear_spring: f32,
    /// Suspension damping per strut (N·s/m). Soaks up the spring's bounce so the
    /// aircraft settles instead of oscillating. Roughly critical near
    /// `2·√(gear_spring · mass_per_wheel)`.
    pub gear_damping: f32,
    /// Main-gear strut natural (uncompressed) length in metres — the ride height
    /// of the two rear wheels. Larger values park the tail higher.
    pub gear_rest_length: f32,
    /// Nose-gear strut natural (uncompressed) length in metres. Independent of
    /// the mains so the resting pitch attitude can be set: longer than the mains
    /// sits the aircraft nose-up, shorter sits it nose-down.
    pub gear_nose_rest_length: f32,
    /// Lateral tyre grip (N·s/m): viscous resistance to sliding sideways, so the
    /// aircraft tracks straight on rollout. Capped by the strut's normal load.
    pub gear_grip: f32,
    /// Rolling resistance coefficient (Crr, dimensionless): fore-and-aft tyre
    /// drag as a fraction of the strut's normal load. Real tyres are ~0.02–0.05,
    /// so the wheels roll nearly freely and the drag fades as lift unloads the
    /// gear on the takeoff roll. (Was a speed-proportional N·s/m term that grew
    /// large enough to cancel thrust near rotate speed.)
    pub gear_rolling_resistance: f32,
    /// Brake strength: extra rolling-resistance coefficient added while the
    /// brakes (B) are held. Like rolling resistance it scales with wheel load,
    /// so ~0.5–0.8 gives a firm but realistic ground deceleration. Fades near a
    /// standstill, so it slows the rollout rather than locking a parked aircraft.
    pub gear_brake_strength: f32,

    // Gear geometry — strut mount points in the body frame (metres). The struts
    // hang straight down (body −Y) by `gear_rest_length` from these; the wheel
    // layout is built in `landing_gear::gear_mounts`.
    /// Nose-wheel station: distance forward of the origin (+Z), metres.
    pub gear_nose_z: f32,
    /// Main-gear station: distance forward of the origin (+Z), metres. Usually
    /// just aft of the CoM so the aircraft sits slightly nose-up on its wheels.
    pub gear_main_z: f32,
    /// Main-gear track: lateral distance between the two main wheels (metres).
    /// Each main sits at ±half this on the body X axis.
    pub gear_track: f32,
    /// Height of every strut mount above the origin (+Y), metres. Lower mounts
    /// tuck the wheels closer to the belly; combined with `gear_rest_length`
    /// this sets how far the wheels reach below the fuselage.
    pub gear_mount_height: f32,

    // --- Mass & inertia ----------------------------------------------------
    /// Empty (zero-fuel, no occupants, no baggage) airframe mass (kg).
    /// C172 basic empty weight ≈ 767 kg. The fuel/cargo/occupant load is added
    /// on top in [`FlightModelConfig::loaded_mass_properties`].
    pub mass: f32,
    /// Empty-airframe principal moments of inertia about the BODY axes (kg·m²):
    /// X = pitch, Y = yaw, Z = roll (nose is +Z, wings span ±X, up is +Y).
    /// Higher = more sluggish rotation on that axis. C172: pitch 1825, yaw 2667,
    /// roll 1285. The load adds to these via the parallel-axis theorem.
    pub angular_inertia: Vec3,
    /// Avian's intrinsic angular damping (1/s). The aerodynamic rotational
    /// damping is modelled separately in `aero_damp`; this is an extra
    /// velocity-independent decay, normally left at 0.
    pub angular_damping: f32,

    // --- Loadout (mass that shifts CoM & inertia realistically) ------------
    /// Left wing-tank fuel (kg), 0 → full (~75 kg). Independent of the right
    /// tank so an imbalance offsets the CoM laterally and rolls the aircraft.
    pub fuel_left_kg: f32,
    /// Right wing-tank fuel (kg), 0 → full (~75 kg).
    pub fuel_right_kg: f32,
    /// Baggage load (kg), 0 → max (~54 kg / 120 lb). Sits aft of the rear seats.
    pub cargo_kg: f32,
    /// Number of occupants on board (1–4), filled pilot → front-R → rear seats.
    /// Each is treated as a standard ~86 kg adult.
    pub passengers: u32,

    // --- Visual ------------------------------------------------------------
    /// Local-space offset (×0.1 → metres) of the GLTF mesh relative to the
    /// physics origin. Purely cosmetic — lets the model be lined up with the
    /// simulated wing/tail positions without touching the flight model.
    pub model_offset: Vec3,
    /// Local-space center of mass (×0.1 → metres). +Z forward (nose), +Y up.
    /// Moving aft shortens the static margin (livelier/less stable pitch);
    /// moving down increases pendulum roll stability. Every aerodynamic moment
    /// arm is measured from this point, so it genuinely sets the trim and
    /// stability. Synced to the rigid body at runtime.
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
            pitch_sensitivity:    0.80,
            roll_sensitivity:     0.25,
            yaw_sensitivity:      0.60,
            throttle_rate:        0.5,
            servo_tau:            0.2,
  
            elevator_trim:        -0.06,

            // Supplemental damping only — the tail/fin/wings already provide the
            // primary rate damping aerodynamically. Halved from the old
            // (1.0, 9.0, 2.5) to stop double-counting; re-tune via the slider.
            aero_damp:            Vec3::new(0.5, 4.5, 1.25),

            fuselage_drag:        Vec3::new(60.0, 10.0, 0.15),
            air_density:          1.2,
            gravity:              9.81,
            prediction_fraction:  0.5,

            auto_level_strength:  18.0,
            bank_turn_strength:   12.0,

            thrust_max:           2_600.0,

            // Loaded C172 ≈ 1000 kg over 3 wheels (~330 kg each). gear_spring
            // ~60 kN/m squats ≈5 cm under that load; gear_damping ≈ 2·√(k·m)
            // ≈ 9 kN·s/m is near-critical so it settles without bouncing.
            gear_spring:             100_000.0,
            gear_damping:            15_000.0,
            gear_rest_length:        1.1,
            gear_nose_rest_length:   1.1,
            gear_grip:               6_000.0,
            gear_rolling_resistance: 0.03,
            gear_brake_strength:     0.6,

            // Tricycle layout: nose wheel forward, mains just aft of the CoM
            // with a ~2.5 m track, mounts tucked just below the origin.
            gear_nose_z:             2.35,
            gear_main_z:             0.2,
            gear_track:              2.5,
            gear_mount_height:       -0.15,

            mass:                 767.0, // C172 basic empty weight
            angular_inertia:      Vec3::new(1825.0, 2667.0, 1285.0),
            angular_damping:      0.0,

            fuel_left_kg:         FUEL_TANK_MAX_KG / 2.0, // full tanks
            fuel_right_kg:        FUEL_TANK_MAX_KG / 2.0,
            cargo_kg:             0.0,
            passengers:           1,           // pilot only

            model_offset:         Vec3::new(0.0, -12.0, 11.0),
            // Local units (×0.1 → metres): 1.5 m forward of the wing AC (+Z) and
            // 0.1 m up. A fairly forward CoM → large static margin, so the model
            // is very pitch-stable / nose-heavy. Every aerodynamic moment arm is
            // measured from here, so this directly sets trim and stability.
            center_of_mass:       Vec3::new(0.0, 1.0, 8.0),

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

// Loadout geometry. Positions are in metres in the rigid-body frame
// (+Z forward/nose, +Y up, +X right), relative to the transform origin — the
// same frame as `center_of_mass * ROOT_SCALE`. These are approximate C172
// stations, enough to make fuel/baggage/occupants shift the CoM and inertia
// the way a real loadout does (e.g. aft baggage makes it tail-heavy).

/// Standard adult occupant mass (kg) — FAA summer standard.
pub const OCCUPANT_MASS: f32 = 86.0;
/// Full usable fuel per wing tank (kg): ~28 US gal of avgas each, ~56 gal total.
pub const FUEL_TANK_MAX_KG: f32 = 75.0;
/// Maximum baggage (kg): the C172's 120 lb compartment limit.
pub const CARGO_MAX_KG: f32 = 54.0;

/// Seat positions in fill order: pilot (front-L), front-R, rear-L, rear-R.
const SEAT_POS: [Vec3; 4] = [
    Vec3::new(-0.28, 0.7, 0.3),
    Vec3::new(0.28, 0.7, 0.3),
    Vec3::new(-0.28, 0.7, -0.5),
    Vec3::new(0.28, 0.7, -0.5),
];
/// Wing-tank fuel centroids — high (in the wings) and near the wing AC. The ±X
/// lever arm is what turns a left/right fuel imbalance into a roll moment.
const FUEL_POS_LEFT: Vec3 = Vec3::new(-2.0, 1.0, 0.1);
const FUEL_POS_RIGHT: Vec3 = Vec3::new(2.0, 1.0, 0.1);
/// Baggage compartment, aft of the rear seats.
const CARGO_POS: Vec3 = Vec3::new(0.0, 0.3, -1.1);

impl FlightModelConfig {
    /// Effective mass properties of the empty airframe plus the current load.
    ///
    /// Returns `(mass_kg, center_of_mass_metres, principal_inertia)` where the
    /// CoM is in metres in the rigid-body frame (ready for Avian's
    /// `CenterOfMass`) and the inertia is the principal moments (X=pitch,
    /// Y=yaw, Z=roll) about that loaded CoM. The empty airframe is treated as a
    /// point mass at its CoM carrying `angular_inertia`; each load item is a
    /// point mass at its station. CoM is the mass-weighted average and inertia
    /// is combined via the parallel-axis theorem.
    pub fn loaded_mass_properties(&self) -> (f32, Vec3, Vec3) {
        let empty_com = self.center_of_mass * ROOT_SCALE; // local units → metres

        // Gather (mass, position) for every load item present.
        let mut loads: Vec<(f32, Vec3)> = Vec::new();
        if self.fuel_left_kg > 0.0 {
            loads.push((self.fuel_left_kg, FUEL_POS_LEFT));
        }
        if self.fuel_right_kg > 0.0 {
            loads.push((self.fuel_right_kg, FUEL_POS_RIGHT));
        }
        if self.cargo_kg > 0.0 {
            loads.push((self.cargo_kg, CARGO_POS));
        }
        for seat in SEAT_POS.iter().take(self.passengers.min(4) as usize) {
            loads.push((OCCUPANT_MASS, *seat));
        }

        // Total mass and mass-weighted CoM.
        let mut total_mass = self.mass;
        let mut weighted = self.mass * empty_com;
        for &(m, r) in &loads {
            total_mass += m;
            weighted += m * r;
        }
        let com = weighted / total_mass;

        // Per-axis parallel-axis contribution of a point mass `m` at `r`.
        let parallel_axis = |m: f32, r: Vec3| {
            let d = r - com;
            Vec3::new(
                m * (d.y * d.y + d.z * d.z), // about X → pitch
                m * (d.x * d.x + d.z * d.z), // about Y → yaw
                m * (d.x * d.x + d.y * d.y), // about Z → roll
            )
        };
        let mut inertia = self.angular_inertia + parallel_axis(self.mass, empty_com);
        for &(m, r) in &loads {
            inertia += parallel_axis(m, r);
        }

        (total_mass, com, inertia)
    }
}
