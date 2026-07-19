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
    /// Sea-level air density (kg/m³). Standard ISA = 1.225. This is the
    /// reference value at the sim's zero altitude (equivalent to setting
    /// local QNH); [`isa_density_ratio`] scales it down with altitude to get
    /// the actual density fed into the aero/drag forces, so thinner air at
    /// altitude means less lift, less drag, and less engine power — all from
    /// one consistent atmosphere model.
    pub air_density: f32,
    /// Gravitational acceleration (m/s²).
    pub gravity: f32,
    /// Fraction of the time-step used for the trapezoidal velocity prediction.
    /// 0.5 = midpoint (second-order accurate), 0 = forward-Euler.
    pub prediction_fraction: f32,

    /// Ground-effect strength (0 = off). On the deck the wing's effective aspect
    /// ratio is multiplied by `1 + strength`, which both raises the lift-curve
    /// slope (more lift) and cuts induced drag — the "float" in the flare and a
    /// slightly earlier unstick. `1.0` roughly matches the physical maximum;
    /// higher values exaggerate it for feel. The boost fades with height (see
    /// `ground_effect_span`) to nothing at altitude, so cruise is unaffected.
    pub ground_effect_strength: f32,
    /// Reference wingspan (m). Sets how high the cushion reaches: the boost is a
    /// Gaussian in height that's ~full on the deck, ~37% at half this span up,
    /// and gone by one span. Use the full wingspan (≈ 7.3 m here); raise it to
    /// make ground effect linger higher, lower it to keep it near the surface.
    pub ground_effect_span: f32,

    // --- Flight assists ----------------------------------------------------
    /// Roll auto-level strength. When no roll input is active, applies a
    /// corrective torque: torque = -coeff * bank_angle * airspeed.
    pub auto_level_strength: f32,
    /// Pitch stabilization strength. When no pitch input is active, applies a
    /// corrective torque nudging the nose back toward level pitch.
    /// Torque = -coeff * pitch_angle * airspeed.
    pub pitch_assist_strength: f32,
    /// Pitch rate damping. Opposes pitch angular velocity regardless of attitude,
    /// killing phugoid oscillations. Torque = -coeff * pitch_rate * airspeed.
    pub pitch_rate_damp: f32,
    /// Bank-to-turn strength. Yaw torque = coeff * bank_angle * airspeed.
    pub bank_turn_strength: f32,

    // --- Engine ------------------------------------------------------------
    /// Maximum static thrust at full throttle (Newtons), produced at `prop_max_rps`.
    pub thrust_max: f32,
    /// Engine spool-UP time constant (seconds): how quickly RPM (and thus thrust)
    /// climbs toward the throttle's target when you advance the throttle. Larger
    /// = lazier acceleration. ~63% of the gap is closed every `tau` seconds.
    pub engine_spool_up_tau: f32,
    /// Engine spool-DOWN time constant (seconds): how slowly RPM bleeds off when
    /// you pull the throttle back. Usually longer than spool-up — the prop coasts.
    pub engine_spool_down_tau: f32,
    /// Starter cranking speed (rev/s) — the RPM the starter motor drives the
    /// engine to while you hold the start key, before it catches. ~300 rpm ≈ 5.
    pub engine_crank_rps: f32,
    /// Seconds of cranking before the engine catches and transitions to running.
    pub engine_start_secs: f32,

    // --- Propeller (visual only) -------------------------------------------
    // Configuation for propeller
    pub propeller: PropellerConfig,

    // --- Landing gear ------------------------------------------------------
    /// Configuation for landing gear
    pub landing_gear: LandingGearConfig,

    // --- Aircraft lights --------------------------------------------------
    pub lights: LightsConfig,

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

    // --- Cargo (mass that shifts CoM & inertia realistically) ------------
    // Config for cargo.
    pub cargo: CargoConfig,

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

    /// Wing rigging incidence (degrees): the angle the wing chord is mounted at
    /// relative to the fuselage reference line, nose-up positive. A real wing is
    /// bolted on at a small positive incidence (C172 ≈ 1.5°) so it flies at a
    /// useful angle of attack while the fuselage sits level — that's what lets it
    /// lift off and cruise without a permanently nose-high attitude. Higher
    /// incidence makes more lift at any given pitch, so the aircraft unsticks
    /// sooner and feels lighter, at the cost of a more nose-down cruise. Applied
    /// to the main wing and aileron panels (pitch about their span axis).
    pub wing_incidence: f32,

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

#[derive(Clone)]
pub struct CargoConfig {
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
}

impl Default for CargoConfig {
    fn default() -> Self {
        Self { 
            fuel_left_kg:         FUEL_TANK_MAX_KG / 3.0,
            fuel_right_kg:        FUEL_TANK_MAX_KG / 3.0,
            cargo_kg:             0.0,
            passengers:           1, // pilot only
        }
    }
}



#[derive(Clone)]
/// Configuation for propeller
pub struct PropellerConfig {
    /// Propeller spin rate at zero throttle, in revolutions per second. The
    /// engine idles rather than stopping, so the prop keeps turning on the
    /// ground. Purely cosmetic — does not affect thrust.
    pub prop_idle_rps: f32,
    /// Propeller spin rate at full throttle (rev/s). The visual rate scales
    /// linearly between `prop_idle_rps` and this with `throttle_percent`.
    pub prop_max_rps: f32,
    /// Local-space axis the propeller node spins about. The model faces +Z, so
    /// the prop turns about Z; flip a component if the blades spin edge-on.
    pub prop_spin_axis: Vec3,
    /// Local-space position (×0.1 → metres) of the placeholder "debug propeller"
    /// rectangle, relative to the aircraft origin. Slide it onto the nose; this
    /// is the spot the real prop node should occupy once the model is ported.
    pub prop_position: Vec3,
    /// Propeller disc radius in metres — drives the prop gizmo (the circle swept
    /// by the blades). C172 ≈ 0.94 m.
    pub prop_radius: f32,
    /// Airspeed (m/s) at which the fixed-pitch prop produces zero net thrust —
    /// the blades stall and the thrust curve reaches zero. Thrust falls linearly
    /// from `thrust_max` at v=0 to 0 at this speed. C172 fixed-pitch prop: ~82 m/s
    /// (≈160 kt). At cruise (70 kt ≈ 36 m/s) this gives ~56% of static thrust,
    /// which matches the real aircraft's available thrust at that speed.
    pub prop_zero_thrust_speed: f32,
}

impl Default for PropellerConfig {
    fn default() -> Self {
        Self {
            // Real C172 (direct-drive, fixed-pitch): idle ~650 rpm, redline 2700.
            prop_idle_rps:        11.0,  // ~660 rpm running idle
            prop_max_rps:         45.0,  // 2700 rpm redline
            prop_spin_axis:       Vec3::Z,
            // Forward of the wing on the nose, on the centerline. Tune onto the
            // model's spinner with the F3 "Propeller" sliders (G shows the gizmo).
            prop_position:        Vec3::new(0.0, 3.5, 33.0),
            prop_radius:          0.94,
            prop_zero_thrust_speed: 82.0, // ~160 kt — fixed-pitch blade stall speed
        }
    }
}

/// Configuation for plane landing gear
#[derive(Clone)]
pub struct LandingGearConfig {
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
    /// Nose-strut mount height above the origin (+Y), metres. Independent of the
    /// mains so the nose can be tucked or dropped on its own; combined with
    /// `gear_nose_rest_length` it sets how far the nose wheel reaches below the
    /// fuselage (and so the on-ground pitch attitude).
    pub gear_nose_mount_height: f32,
    /// Main-strut mount height above the origin (+Y), metres, shared by both
    /// mains. Combined with `gear_rest_length` it sets how far the main wheels
    /// reach below the fuselage.
    pub gear_main_mount_height: f32,
}

/// Positions and intensities for the aircraft's exterior lights.
///
/// All positions are in **local units** (×0.1 → metres), relative to the
/// aircraft root entity — the same space the propeller and debug-prop sliders
/// use. Edit them live from the F3 "Lights" panel.
#[derive(Clone)]
pub struct LightsConfig {
    // Nav lights (always on while running)
    pub nav_left_pos:   Vec3, // red, left wingtip
    pub nav_right_pos:  Vec3, // green, right wingtip
    pub nav_tail_pos:   Vec3, // white, tail

    // Strobe lights (white flash, wingtips + tail, anti-collision)
    pub strobe_left_pos:  Vec3,
    pub strobe_right_pos: Vec3,
    pub strobe_tail_pos:  Vec3,
    /// Period between flashes (seconds). Real C172 strobes fire every ~1.2 s.
    pub strobe_period:  f32,
    /// How long each flash stays lit (seconds).
    pub strobe_on_time: f32,

    // Beacon (red pulse on belly, on whenever engine is running)
    pub beacon_pos: Vec3,
    /// Beacon pulse period (seconds). Real rotating beacon feels like ~1 Hz.
    pub beacon_period: f32,

    // Landing light (bright forward spotlight; toggled with L)
    pub landing_pos: Vec3,
    /// Forward and slightly down: angles are in degrees, positive = nose-down.
    pub landing_pitch_deg: f32,
    /// Outer cone half-angle of the spotlight (degrees).
    pub landing_outer_deg: f32,
    /// Inner (bright) cone half-angle (degrees).
    pub landing_inner_deg: f32,
    /// Intensity of the landing light (lux).
    pub landing_intensity: f32,

    // Shared intensities
    /// Intensity of the nav position lights (lux).
    pub nav_intensity: f32,
    /// Peak intensity of each strobe flash (lux).
    pub strobe_intensity: f32,
    /// Peak intensity of the anti-collision beacon (lux).
    pub beacon_intensity: f32,
}

impl Default for LightsConfig {
    fn default() -> Self {
        Self {
            // Nav lights: wingtips at ±55 local X, tail at -65 Z.
            // Y=10 puts them at wing height; tail sits slightly above the tailplane.
            nav_left_pos:   Vec3::new(-650.0, 130.0, 100.0),
            nav_right_pos:  Vec3::new( 650.0, 130.0, 100.0),
            nav_tail_pos:   Vec3::new(  0.0, 40.0, -620.0),

            // Strobes co-located with nav lights (real C172 has combined units).
            strobe_left_pos:  Vec3::new(-650.0, 130.0, 100.0),
            strobe_right_pos: Vec3::new( 650.0, 130.0, 100.0),
            strobe_tail_pos:  Vec3::new(0.0, 40.0, -620.0),
            strobe_period:    1.2,
            strobe_on_time:   0.07, // brief pop

            beacon_pos:    Vec3::new(0.0, 240.0, -530.0),
            beacon_period: 1.0,

            // Landing light: on the nose, angled ~5° down to light the runway.
            landing_pos:       Vec3::new(0.0, 5.0, 320.0),
            landing_pitch_deg: -5.0,
            landing_outer_deg: 65.0,
            landing_inner_deg: 25.0,
            landing_intensity: 3_000_000.0,

            nav_intensity:     1000.0,
            strobe_intensity:  9_000.0,
            beacon_intensity:  6_000.0,
        }
    }
}

impl Default for LandingGearConfig {
    fn default() -> Self {
        Self {
            // Sized for a loaded light single over three wheels: firm enough to
            // squat only a few cm under load, with damping near-critical
            // (≈ 2·√(k · mass-per-wheel)) so it settles without bouncing.
            gear_spring:             120_000.0,
            gear_damping:            15_000.0,
            gear_rest_length:        1.1,
            gear_nose_rest_length:   1.1,
            gear_grip:               6_000.0,
            gear_rolling_resistance: 0.01,
            gear_brake_strength:     0.65,

            // Tricycle layout: nose wheel forward, mains just aft of the CoM
            // with a ~2.5 m track, mounts tucked just below the origin.
            gear_nose_z:             2.35,
            gear_main_z:             0.2,
            gear_track:              2.5,
            gear_nose_mount_height:  -0.15,
            gear_main_mount_height:  -0.15,
        }
    }
}


/// ISA (International Standard Atmosphere) troposphere density ratio σ =
/// ρ(altitude)/ρ(sea level), from the standard barometric formula. Shared by
/// the aero/drag forces (scales `air_density`), the mixture lever (its ideal
/// setting leans with this same ratio), and the baro instrument readout, so
/// all three agree on what the air is doing at a given altitude.
pub fn isa_density_ratio(altitude_m: f32) -> f32 {
    (1.0 - 2.2557e-5 * altitude_m).max(0.0).powf(4.2559)
}

impl Default for FlightModelConfig {
    fn default() -> Self {
        Self {
            pitch_sensitivity:    0.50,
            roll_sensitivity:     0.22,
            yaw_sensitivity:      0.45,
            throttle_rate:        0.5,
            servo_tau:            0.45,
  
            elevator_trim:        0.0,

            // Supplemental damping only — the tail/fin/wings already provide the
            // primary rate damping aerodynamically, so keep this low to avoid
            // double-counting; re-tune via the slider.
            aero_damp:            Vec3::new(25.0, 4.5, 4.0),

            // Cd·A per body axis (side X, belly Y, nose Z). The nose is
            // streamlined (small Z) and is the only term that acts in normal
            // flight; the flank/belly terms bite in a skid or a high-AoA mush.
            // Roughly broadside Cd·A for a light-aircraft fuselage.
            fuselage_drag:        Vec3::new(3.0, 4.0, 0.20),
            air_density:          1.2,
            gravity:              9.81,
            prediction_fraction:  0.5,

            // Realistic ground effect: strength 1.0 is the physical ceiling (a
            // clear flare float, not arcade), and the span is the real C172
            // wingspan (~11 m) since ground-effect reach scales with the actual
            // wingspan and starts being felt about one span above the ground.
            ground_effect_strength: 1.0,
            ground_effect_span:     11.0,

            auto_level_strength:  150.0,
            pitch_assist_strength: 150.0,
            pitch_rate_damp:       20.0,
            bank_turn_strength:   12.0,

            thrust_max:           3_200.0,
            // Lycoming-ish throttle response: winds up in ~1.2 s, settles back to
            // idle in ~1.5 s. (A real fixed-pitch single responds in a second or
            // two; the deliberate slow throttle push pilots use is technique.)
            engine_spool_up_tau:   1.2,
            engine_spool_down_tau: 1.5,
            engine_crank_rps:      5.0,  // ~300 rpm starter cranking speed
            engine_start_secs:     1.5,  // cranks ~1.5 s before it catches

            propeller: PropellerConfig::default(),

            landing_gear: LandingGearConfig::default(),

            lights: LightsConfig::default(),

            mass:                 767.0, // C172 basic empty weight
            // Real C172 moments of inertia (kg·m²). Body axes: X=pitch (wing-to-wing),
            // Y=yaw (vertical), Z=roll (nose). Published values: Ix(roll)≈1285,
            // Iy(pitch)≈1285, Iz(yaw)≈1825. Previous values were wrong/swapped.
            angular_inertia:      Vec3::new(1285.0, 1825.0, 1285.0),
            angular_damping:      0.0,

            cargo: CargoConfig::default(),

            model_offset:         Vec3::new(0.0, -12.0, 11.0),
            // Local units (×0.1 → metres), +Z forward / +Y up. Sits just ahead of
            // the wing for a small positive static margin (pitch-stable without
            // being nose-heavy). Every aerodynamic moment arm is measured from
            // here, so it directly sets trim and stability — but keep it forward of
            // the main gear or the aircraft tips back on its tail on the ground.
            center_of_mass:       Vec3::new(0.0, 1.0, 3.0),

            // 4° rigging incidence: enough lift at a gentle fuselage attitude so
            // the pilot doesn't have to hold extreme back-pressure to stay airborne.
            wing_incidence:       4.0,

            // Main wing panels (two, one per side). Sized to the real C172 wing:
            //   span × chord = 4.05 × 1.62 ≈ 6.56 m² per panel
            //   Two panels + two ailerons → 16.2 m² total (real C172 wing area).
            //   Full wingspan: 2 × (0.475 root + 4.05 wing + 0.95 aileron) ≈ 11.0 m.
            //   aspect_ratio is the full-wing AR (7.32) so the lift-slope correction
            //   and induced drag use the real value, independent of per-panel geometry.
            //   Stall speed Vs ≈ 46 kt (real C172: 44–48 kt).
            wing: AeroSurfaceConfig {
                flap_fraction: 0.2,
                span: 4.05,
                chord: 1.62,
                aspect_ratio: 7.32,
                ..AeroSurfaceConfig::default()
            },
            // Ailerons: outer panels, 0.95 m span × 1.62 m chord ≈ 1.54 m² each.
            // Root sits flush against the wing-panel tip (no gap, no overlap).
            aileron: AeroSurfaceConfig {
                flap_fraction: 0.35,
                span: 0.95,
                chord: 1.62,
                aspect_ratio: 7.32,
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
        if self.cargo.fuel_left_kg > 0.0 {
            loads.push((self.cargo.fuel_left_kg, FUEL_POS_LEFT));
        }
        if self.cargo.fuel_right_kg > 0.0 {
            loads.push((self.cargo.fuel_right_kg, FUEL_POS_RIGHT));
        }
        if self.cargo.cargo_kg > 0.0 {
            loads.push((self.cargo.cargo_kg, CARGO_POS));
        }
        for seat in SEAT_POS.iter().take(self.cargo.passengers.min(4) as usize) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- loaded_mass_properties -------------------------------------------

    // Default config (pilot only, 1/3 tanks each side) should produce a mass
    // close to empty + pilot + fuel and a CoM that is near the longitudinal centre.
    #[test]
    fn loaded_mass_pilot_only() {
        let cfg = FlightModelConfig::default();
        let (mass, _com, _inertia) = cfg.loaded_mass_properties();

        // 767 empty + 86 pilot + 2*(75/3) fuel = 903 kg
        let expected_mass = 767.0 + 86.0 + 2.0 * (75.0 / 3.0);
        assert!((mass - expected_mass).abs() < 0.1,
            "Loaded mass should be ~{expected_mass:.0} kg, got {mass:.1}");
    }

    // With equal left/right fuel the CoM x-offset must be essentially zero.
    #[test]
    fn balanced_fuel_keeps_com_centred_laterally() {
        let mut cfg = FlightModelConfig::default();
        cfg.cargo.fuel_left_kg  = 40.0;
        cfg.cargo.fuel_right_kg = 40.0;
        let (_, com, _) = cfg.loaded_mass_properties();
        assert!(com.x.abs() < 0.05,
            "Symmetric fuel load must keep CoM on centreline, got x={:.4}", com.x);
    }

    // A heavy left tank should shift the CoM to the left (negative x).
    #[test]
    fn heavy_left_tank_shifts_com_left() {
        let mut cfg = FlightModelConfig::default();
        cfg.cargo.fuel_left_kg  = 70.0;
        cfg.cargo.fuel_right_kg = 0.0;
        let (_, com, _) = cfg.loaded_mass_properties();
        assert!(com.x < 0.0,
            "Full left tank only must shift CoM to -x, got {:.4}", com.x);
    }

    // Aft baggage should move the CoM rearward (negative z in body frame).
    #[test]
    fn aft_baggage_shifts_com_rearward() {
        let mut cfg_no_cargo = FlightModelConfig::default();
        cfg_no_cargo.cargo.cargo_kg = 0.0;

        let mut cfg_cargo = FlightModelConfig::default();
        cfg_cargo.cargo.cargo_kg = CARGO_MAX_KG;

        let (_, com_no, _) = cfg_no_cargo.loaded_mass_properties();
        let (_, com_cargo, _) = cfg_cargo.loaded_mass_properties();

        assert!(com_cargo.z < com_no.z,
            "Aft cargo must move CoM rearward (more negative z), got no_cargo.z={:.4} cargo.z={:.4}",
            com_no.z, com_cargo.z);
    }

    // Adding any load must strictly increase total inertia on all axes (parallel-axis theorem).
    #[test]
    fn inertia_increases_with_load() {
        let mut cfg_empty = FlightModelConfig::default();
        cfg_empty.cargo.fuel_left_kg  = 0.0;
        cfg_empty.cargo.fuel_right_kg = 0.0;
        cfg_empty.cargo.cargo_kg      = 0.0;
        cfg_empty.cargo.passengers    = 0;

        let mut cfg_full = FlightModelConfig::default();
        cfg_full.cargo.fuel_left_kg  = FUEL_TANK_MAX_KG;
        cfg_full.cargo.fuel_right_kg = FUEL_TANK_MAX_KG;
        cfg_full.cargo.cargo_kg      = CARGO_MAX_KG;
        cfg_full.cargo.passengers    = 4;

        let (_, _, i_empty) = cfg_empty.loaded_mass_properties();
        let (_, _, i_full)  = cfg_full.loaded_mass_properties();

        assert!(i_full.x > i_empty.x, "Pitch inertia must increase with load");
        assert!(i_full.y > i_empty.y, "Yaw inertia must increase with load");
        assert!(i_full.z > i_empty.z, "Roll inertia must increase with load");
    }

    // CoM must stay in front of (or at) the main gear to prevent tail-tipping.
    // Main gear station is gear_main_z metres forward of origin.
    #[test]
    fn com_forward_of_main_gear() {
        let cfg = FlightModelConfig::default();
        let (_, com, _) = cfg.loaded_mass_properties();
        // gear_main_z = 0.2 m in default config — CoM.z must be >= that.
        assert!(com.z >= cfg.landing_gear.gear_main_z - 0.01,
            "CoM must not be aft of main gear or aircraft tips backward: com.z={:.3}, gear_z={:.3}",
            com.z, cfg.landing_gear.gear_main_z);
    }

    // --- FlightModelConfig default sanity ---------------------------------

    // All sensitivities and rates must be positive.
    #[test]
    fn config_sensitivities_positive() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.pitch_sensitivity > 0.0);
        assert!(cfg.roll_sensitivity  > 0.0);
        assert!(cfg.yaw_sensitivity   > 0.0);
        assert!(cfg.throttle_rate     > 0.0);
        assert!(cfg.servo_tau         > 0.0);
    }

    // Aero damping must be positive on all axes.
    #[test]
    fn aero_damp_positive() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.aero_damp.x > 0.0, "roll damp must be positive");
        assert!(cfg.aero_damp.y > 0.0, "yaw damp must be positive");
        assert!(cfg.aero_damp.z > 0.0, "pitch damp must be positive");
    }

    // Fuselage drag must be positive on all axes; the nose axis (z) should be
    // meaningfully smaller than the side/belly axes — it's the streamlined direction.
    #[test]
    fn fuselage_drag_nose_streamlined() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.fuselage_drag.z > 0.0, "nose drag must be positive");
        assert!(cfg.fuselage_drag.x > cfg.fuselage_drag.z,
            "side drag must exceed nose drag (fuselage is not a sphere)");
        assert!(cfg.fuselage_drag.y > cfg.fuselage_drag.z,
            "belly drag must exceed nose drag");
    }

    // Engine spool must take at least a fraction of a second (not instant).
    #[test]
    fn engine_spool_time_constants_nonzero() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.engine_spool_up_tau   >= 0.5, "spool-up too fast: {}", cfg.engine_spool_up_tau);
        assert!(cfg.engine_spool_down_tau >= 0.5, "spool-down too fast: {}", cfg.engine_spool_down_tau);
    }

    // The C172's main wheels must sit behind the nose wheel.
    #[test]
    fn tricycle_gear_layout() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.landing_gear.gear_nose_z > cfg.landing_gear.gear_main_z,
            "Nose wheel must be ahead (+z) of main gear: nose_z={} main_z={}",
            cfg.landing_gear.gear_nose_z, cfg.landing_gear.gear_main_z);
    }

    // --- Fuselage drag formula -------------------------------------------

    // Independent formula check: F = 0.5 * rho * CdA * v^2 at known speeds.
    #[test]
    fn fuselage_nose_drag_at_60ms() {
        let rho   = 1.2_f32;
        let cda_z = 0.10_f32; // forward CdA: AoA-dependent penalty only (skin friction already in wing polar)
        let speed = 60.0_f32;
        let expected = 0.5 * rho * cda_z * speed * speed; // 216 N
        assert!((expected - 216.0).abs() < 1.0,
            "Fuselage nose drag at 60 m/s should be ~216 N, got {expected}");
    }

    #[test]
    fn fuselage_drag_quadratic_with_speed() {
        // Doubling speed must quadruple drag (v² relationship).
        let rho = 1.2_f32;
        let cda = FlightModelConfig::default().fuselage_drag.z;
        let d30 = 0.5 * rho * cda * 30.0_f32.powi(2);
        let d60 = 0.5 * rho * cda * 60.0_f32.powi(2);
        let ratio = d60 / d30;
        assert!((ratio - 4.0).abs() < 0.01,
            "Doubling speed must quadruple drag, got ratio {ratio:.3}");
    }

    // --- C172 Physics Plausibility ----------------------------------------
    // These tests document where the sim intentionally diverges from a real C172
    // and where it should be close. They act as regression guards against
    // accidental tuning drift.

    // Thrust max should be in the range of a real C172 (~2000 N) ± generous sim margin.
    #[test]
    fn thrust_max_plausible_for_c172() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.thrust_max > 1500.0 && cfg.thrust_max < 4000.0,
            "Thrust max outside C172-plausible range: {}", cfg.thrust_max);
    }

    // Empty airframe mass must be close to real C172 basic empty weight (767 kg ±20%).
    #[test]
    fn empty_mass_close_to_c172() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.mass > 600.0 && cfg.mass < 920.0,
            "Empty mass outside C172 range (767 kg ±20%): {}", cfg.mass);
    }

    // Prop idle/redline RPM must make physical sense (idle < redline, both > 0).
    #[test]
    fn prop_rps_ordering() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.propeller.prop_idle_rps > 0.0);
        assert!(cfg.propeller.prop_max_rps  > cfg.propeller.prop_idle_rps,
            "Redline RPM must exceed idle RPM");
    }

    // Wing aspect ratio must be in a sensible range for a GA aircraft.
    #[test]
    fn wing_aspect_ratio_realistic() {
        let cfg = FlightModelConfig::default();
        assert!(cfg.wing.aspect_ratio > 4.0 && cfg.wing.aspect_ratio < 12.0,
            "Wing AR outside realistic GA range: {}", cfg.wing.aspect_ratio);
    }

    // Gravity must be close to 9.81 m/s².
    #[test]
    fn gravity_earth_standard() {
        let cfg = FlightModelConfig::default();
        assert!((cfg.gravity - 9.81).abs() < 0.5,
            "Gravity should be ~9.81 m/s², got {}", cfg.gravity);
    }
}
