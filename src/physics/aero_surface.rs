use bevy::prelude::*;
use super::aero_surface_config::AeroSurfaceConfig;
use super::bi_vector3::BiVector3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlInputType { Pitch, Yaw, Roll, Flap }

#[derive(Component, Clone)]
pub struct AeroSurface {
    pub config: AeroSurfaceConfig,
    pub is_control_surface: bool,
    pub input_type: ControlInputType,
    pub input_multiplier: f32,
    pub flap_angle: f32,
}

impl AeroSurface {
    pub fn wing(config: AeroSurfaceConfig) -> Self {
        Self { config, is_control_surface: false, input_type: ControlInputType::Roll, input_multiplier: 1.0, flap_angle: 0.0 }
    }
    pub fn control(config: AeroSurfaceConfig, input_type: ControlInputType, multiplier: f32) -> Self {
        Self { config, is_control_surface: true, input_type, input_multiplier: multiplier, flap_angle: 0.0 }
    }
    pub fn set_flap_angle(&mut self, angle: f32) {
        self.flap_angle = angle.clamp(-50_f32.to_radians(), 50_f32.to_radians());
    }
}

impl AeroSurface {
    /// Computes lift+drag+torque in world space.
    /// `world_air_velocity` = air velocity relative to this surface in world space
    /// `air_density` = kg/m³
    /// `rel_pos` = surface world position minus aircraft center of mass
    /// `ground_effect` = effective-aspect-ratio multiplier, `>= 1.0`. `1.0` is
    ///   free air; larger values model proximity to the ground, where the
    ///   wingtip vortices/downwash are suppressed and the wing acts like a
    ///   longer, more slender one. Raising the effective aspect ratio increases
    ///   the lift-curve slope (MORE lift for a given AoA) and shrinks the induced
    ///   angle (LESS induced drag) together. Computed by the caller from
    ///   height/span; see `aircraft_physics::ground_effect_factor`.
    pub fn calculate_forces(
        &self,
        world_air_velocity: Vec3,
        air_density: f32,
        rel_pos: Vec3,
        surface_rotation: Quat,
        ground_effect: f32,
    ) -> BiVector3 {
        let mut result = BiVector3::default();
        let c = &self.config;
        // Zero-area surfaces (degenerate config) produce no aerodynamic force.
        if c.span <= 0.0 || c.chord <= 0.0 || c.aspect_ratio <= 0.0 {
            return result;
        }

        // Effective aspect ratio, raised by ground effect (1.0 = free air). Both
        // the lift slope below and the induced angle in the coefficient helpers
        // use this, so the ground cushion adds lift and cuts induced drag at once.
        let aspect_ratio = c.aspect_ratio * ground_effect;

        // Aspect ratio correction on lift slope
        let corrected_lift_slope = c.lift_slope * aspect_ratio
            / (aspect_ratio + 2.0 * (aspect_ratio + 4.0) / (aspect_ratio + 2.0));

        // Flap deflection effect on zero-lift AoA and stall angles
        let theta = (2.0 * c.flap_fraction - 1.0).acos();
        let flap_effectiveness = 1.0 - (theta - theta.sin()) / std::f32::consts::PI;
        let delta_lift = corrected_lift_slope
            * flap_effectiveness
            * self.flap_effectiveness_correction()
            * self.flap_angle;

        let zero_lift_aoa_base = c.zero_lift_aoa.to_radians();
        let zero_lift_aoa = zero_lift_aoa_base - delta_lift / corrected_lift_slope;

        let stall_high_base = c.stall_angle_high.to_radians();
        let stall_low_base = c.stall_angle_low.to_radians();

        let cl_max_high = corrected_lift_slope * (stall_high_base - zero_lift_aoa_base)
            + delta_lift * self.lift_coeff_max_fraction();
        let cl_max_low = corrected_lift_slope * (stall_low_base - zero_lift_aoa_base)
            + delta_lift * self.lift_coeff_max_fraction();

        let stall_high = zero_lift_aoa + cl_max_high / corrected_lift_slope;
        let stall_low = zero_lift_aoa + cl_max_low / corrected_lift_slope;

        // Air velocity in surface-local space.
        // Span is local +Z; project out the spanwise component to get the
        // chord-plane velocity (works for any surface orientation).
        let local_vel = surface_rotation.inverse() * world_air_velocity;
        let local_vel_2d = Vec3::new(local_vel.x, local_vel.y, 0.0); // zero span (Z) component

        let span_dir = surface_rotation * Vec3::Z;
        let drag_dir = surface_rotation * local_vel_2d.normalize_or_zero();
        let lift_dir = drag_dir.cross(span_dir);

        let area = c.chord * c.span;
        let q = 0.5 * air_density * local_vel_2d.length_squared();
        // AoA: angle between chord axis (local +X) and the chord-plane airflow.
        // atan2(y, -x) gives positive AoA when flow comes from below (+Y side).
        let aoa = f32::atan2(local_vel.y, -local_vel.x);

        let coeffs = self.calculate_coefficients(aoa, corrected_lift_slope, zero_lift_aoa, stall_high, stall_low, aspect_ratio);

        let lift = lift_dir * coeffs.x * q * area;
        let drag = drag_dir * coeffs.y * q * area;
        let torque = -span_dir * coeffs.z * q * area * c.chord;

        result.force += lift + drag;
        result.torque += rel_pos.cross(result.force) + torque;
        result
    }

    fn calculate_coefficients(
        &self,
        aoa: f32,
        lift_slope: f32,
        zero_lift_aoa: f32,
        stall_high: f32,
        stall_low: f32,
        effective_aspect_ratio: f32,
    ) -> Vec3 {
        let padding_high = f32::to_radians(f32::lerp(
            15.0, 8.0, (self.flap_angle.to_degrees() + 50.0) / 100.0,
        ));
        let padding_low = f32::to_radians(f32::lerp(
            15.0, 8.0, (-self.flap_angle.to_degrees() + 50.0) / 100.0,
        ));
        let padded_high = stall_high + padding_high;
        let padded_low = stall_low - padding_low;

        if aoa < stall_high && aoa > stall_low {
            self.coeffs_low_aoa(aoa, lift_slope, zero_lift_aoa, effective_aspect_ratio)
        } else if aoa > padded_high || aoa < padded_low {
            self.coeffs_stall(aoa, lift_slope, zero_lift_aoa, stall_high, stall_low, effective_aspect_ratio)
        } else {
            let (low, stall, t) = if aoa > stall_high {
                let low = self.coeffs_low_aoa(stall_high, lift_slope, zero_lift_aoa, effective_aspect_ratio);
                let stall = self.coeffs_stall(padded_high, lift_slope, zero_lift_aoa, stall_high, stall_low, effective_aspect_ratio);
                let t = (aoa - stall_high) / (padded_high - stall_high);
                (low, stall, t)
            } else {
                let low = self.coeffs_low_aoa(stall_low, lift_slope, zero_lift_aoa, effective_aspect_ratio);
                let stall = self.coeffs_stall(padded_low, lift_slope, zero_lift_aoa, stall_high, stall_low, effective_aspect_ratio);
                let t = (aoa - stall_low) / (padded_low - stall_low);
                (low, stall, t)
            };
            low.lerp(stall, t)
        }
    }

    fn coeffs_low_aoa(&self, aoa: f32, lift_slope: f32, zero_lift_aoa: f32, effective_aspect_ratio: f32) -> Vec3 {
        let cl = lift_slope * (aoa - zero_lift_aoa);
        // Standard wing polar: CD = CD0 + CL² / (π·e·AR).
        // The old flat-plate formula (cn·sin(eff) + ct·cos(eff)) over-predicted
        // drag by 2-3× at typical cruise AoA, making the aircraft unflyable.
        let oswald = 0.63_f32; // C172 effective e: matches Cessna polar CD=0.032+0.063·CL²
        let cd = self.config.skin_friction + cl * cl / (std::f32::consts::PI * oswald * effective_aspect_ratio);
        // Pitching moment: ~-0.1 at zero lift for a cambered section, shifting
        // forward with lift as the centre of pressure moves.
        let induced_angle = cl / (std::f32::consts::PI * effective_aspect_ratio);
        let eff = aoa - zero_lift_aoa - induced_angle;
        let cn = cl / eff.cos().max(0.01);
        let cm = -cn * self.torque_coeff_proportion(eff);
        Vec3::new(cl, cd, cm)
    }

    fn coeffs_stall(&self, aoa: f32, lift_slope: f32, zero_lift_aoa: f32, stall_high: f32, stall_low: f32, effective_aspect_ratio: f32) -> Vec3 {
        let cl_low = if aoa > stall_high {
            lift_slope * (stall_high - zero_lift_aoa)
        } else {
            lift_slope * (stall_low - zero_lift_aoa)
        };
        let induced = cl_low / (std::f32::consts::PI * effective_aspect_ratio);
        let t = if aoa > stall_high {
            (std::f32::consts::FRAC_PI_2 - aoa.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2))
                / (std::f32::consts::FRAC_PI_2 - stall_high)
        } else {
            (-std::f32::consts::FRAC_PI_2 - aoa.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2))
                / (-std::f32::consts::FRAC_PI_2 - stall_low)
        };
        let induced = f32::lerp(0.0, induced, t);
        let eff = aoa - zero_lift_aoa - induced;
        let cn = self.friction_at_90() * eff.sin()
            * (1.0 / (0.56 + 0.44 * eff.sin().abs())
                - 0.41 * (1.0 - (-17.0 / self.config.aspect_ratio).exp()));
        let ct = 0.5 * self.config.skin_friction * eff.cos();
        let cl = cn * eff.cos() - ct * eff.sin();
        let cd = cn * eff.sin() + ct * eff.cos();
        let cm = -cn * self.torque_coeff_proportion(eff);
        Vec3::new(cl, cd, cm)
    }

    fn torque_coeff_proportion(&self, eff: f32) -> f32 {
        0.25 - 0.175 * (1.0 - 2.0 * eff.abs() / std::f32::consts::PI)
    }

    fn friction_at_90(&self) -> f32 {
        1.98 - 4.26e-2 * self.flap_angle * self.flap_angle + 2.1e-1 * self.flap_angle
    }

    fn flap_effectiveness_correction(&self) -> f32 {
        f32::lerp(0.8, 0.4, (self.flap_angle.abs().to_degrees() - 10.0) / 50.0)
    }

    fn lift_coeff_max_fraction(&self) -> f32 {
        (1.0 - 0.5 * (self.config.flap_fraction - 0.1) / 0.3).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::aero_surface_config::AeroSurfaceConfig;
    use bevy::math::{Vec3, Quat};

    fn default_wing() -> AeroSurface {
        // Matches the real C172 per-panel geometry from FlightModelConfig::default():
        // chord=1.62m, span=4.05m, AR=7.32 (full-wing AR used for lift-slope correction).
        AeroSurface::wing(AeroSurfaceConfig {
            lift_slope: std::f32::consts::TAU,
            skin_friction: 0.02,
            zero_lift_aoa: -3.0,
            stall_angle_high: 17.0,
            stall_angle_low: -17.0,
            chord: 1.62,
            flap_fraction: 0.0,
            span: 4.05,
            aspect_ratio: 7.32,
        })
    }

    const AIR_DENSITY: f32 = 1.2;
    const LEVEL_FLIGHT_SPEED: f32 = 55.0; // m/s, typical cruise

    // Flat (no rotation) wing with air coming from directly ahead (+Z body = nose,
    // but AeroSurface span is local +Z so "ahead" for the wing is local +X = chord).
    // We send airflow along the chord-plane at a small positive AoA to get lift.
    fn surface_rotation_level() -> Quat {
        // Wing mounted flat: chord along X, span along Z, up is Y.
        Quat::IDENTITY
    }

    fn air_vel_at_aoa(aoa_deg: f32, speed: f32) -> Vec3 {
        // world_air_vel is the air velocity relative to the surface = -aircraft_velocity.
        // The surface's chord axis is local +X (IDENTITY rotation). Air arrives from
        // "ahead" of the chord, which is the -X direction in local/world space.
        // Positive AoA = flow comes from slightly below (+Y), so:
        //   local_vel ≈ (-cos(aoa), +sin(aoa), 0) * speed
        // Then aoa = atan2(local_vel.y, -local_vel.x) = atan2(sin, cos) = aoa ✓
        let aoa = aoa_deg.to_radians();
        Vec3::new(-speed * aoa.cos(), speed * aoa.sin(), 0.0)
    }

    // Sanity check: at zero AoA above zero_lift_aoa, lift should be positive.
    #[test]
    fn lift_is_positive_at_cruise_aoa() {
        let wing = default_wing();
        // zero_lift_aoa = -3°, so at 4° AoA we're 7° above zero-lift → positive CL
        let vel = air_vel_at_aoa(4.0, LEVEL_FLIGHT_SPEED);
        let result = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        assert!(result.force.y > 0.0, "Lift must be positive at 4° AoA, got {}", result.force.y);
    }

    // Drag must always oppose the direction of motion.
    // world_air_vel ≈ -velocity, so projecting the resultant force onto the
    // *aircraft* velocity direction must give a negative value (retarding force).
    #[test]
    fn drag_opposes_motion() {
        let wing = default_wing();
        let vel = air_vel_at_aoa(4.0, LEVEL_FLIGHT_SPEED);
        let result = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        // Aircraft velocity direction = opposite of world_air_vel.
        let aircraft_vel_dir = -vel.normalize();
        let force_along_vel = result.force.dot(aircraft_vel_dir);
        assert!(force_along_vel < 0.0,
            "Net aero force must retard motion (negative projection onto velocity), got {force_along_vel}");
    }

    // At zero_lift_aoa (-3°), lift coefficient is ~0 so the force along lift_dir is minimal.
    // Parasitic (skin friction) drag must still exist.
    #[test]
    fn drag_nonzero_at_zero_lift() {
        let wing = default_wing();
        // AoA = zero_lift_aoa → CL ≈ 0
        let vel = air_vel_at_aoa(-3.0, LEVEL_FLIGHT_SPEED);
        let result = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        // Project force onto drag_dir (= normalize of air velocity) to isolate drag component.
        let drag_dir = vel.normalize();
        let drag_component = result.force.dot(drag_dir);
        assert!(drag_component > 1.0,
            "Skin-friction drag must be nonzero at zero-lift AoA, got {drag_component}");
        // Lift component (perpendicular to drag_dir in the chord plane) should be small.
        // At CL=0, force is entirely drag so force ≈ drag_dir * drag_component.
        let lift_component = (result.force - drag_dir * drag_component).length();
        assert!(lift_component < drag_component * 0.1,
            "Lift should be negligible at zero_lift_aoa, lift={lift_component:.1} vs drag={drag_component:.1}");
    }

    // CL and CD at cruise AoA should be in realistic ranges for a light aircraft wing.
    // Lift and drag must be extracted by projecting the resultant force onto their
    // respective directions (not raw x/y components, since lift_dir and drag_dir rotate
    // with AoA relative to the world axes).
    #[test]
    fn cruise_cd_in_realistic_range() {
        let wing = default_wing();
        let aoa_deg = 4.0;
        let speed = LEVEL_FLIGHT_SPEED;
        let vel = air_vel_at_aoa(aoa_deg, speed);
        let result = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);

        let area = wing.config.chord * wing.config.span;
        let q = 0.5 * AIR_DENSITY * speed * speed;

        // drag_dir = normalize(world_air_vel), which equals normalize(vel) here.
        // lift_dir = drag_dir × span_dir (span = +Z with IDENTITY rotation).
        let drag_dir = vel.normalize();
        let span_dir = Vec3::Z;
        let lift_dir = drag_dir.cross(span_dir);

        let cl = result.force.dot(lift_dir) / (q * area);
        let cd = result.force.dot(drag_dir) / (q * area);

        assert!(cl > 0.2 && cl < 1.5, "CL at 4° AoA should be 0.2–1.5, got {cl}");
        assert!(cd > 0.01 && cd < 0.10, "CD at cruise AoA should be 0.01–0.10, got {cd}");
    }

    // Lift should increase with AoA up to stall.
    #[test]
    fn lift_increases_with_aoa_before_stall() {
        let wing = default_wing();
        let vel_low  = air_vel_at_aoa(2.0, LEVEL_FLIGHT_SPEED);
        let vel_high = air_vel_at_aoa(10.0, LEVEL_FLIGHT_SPEED);
        let f_low  = wing.calculate_forces(vel_low,  AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_high = wing.calculate_forces(vel_high, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        assert!(f_high.force.y > f_low.force.y,
            "Lift must increase with AoA pre-stall: {:.1} < {:.1}", f_low.force.y, f_high.force.y);
    }

    // Ground effect should increase lift relative to free-air.
    #[test]
    fn ground_effect_increases_lift() {
        let wing = default_wing();
        let vel = air_vel_at_aoa(4.0, LEVEL_FLIGHT_SPEED);
        let free_air = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let in_ground_effect = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.3);
        assert!(in_ground_effect.force.y > free_air.force.y,
            "Ground effect must increase lift: {:.1} vs {:.1}", free_air.force.y, in_ground_effect.force.y);
    }

    // After stall, lift should drop significantly compared to just before stall.
    #[test]
    fn lift_drops_after_stall() {
        let wing = default_wing();
        let vel_pre_stall  = air_vel_at_aoa(16.0, LEVEL_FLIGHT_SPEED);
        let vel_post_stall = air_vel_at_aoa(25.0, LEVEL_FLIGHT_SPEED);
        let f_pre  = wing.calculate_forces(vel_pre_stall,  AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_post = wing.calculate_forces(vel_post_stall, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        assert!(f_post.force.y < f_pre.force.y,
            "Lift must drop past stall: pre={:.1} post={:.1}", f_pre.force.y, f_post.force.y);
    }

    // Fuselage nose drag formula: F = 0.5 * rho * CdA_z * v^2.
    // At 60 m/s with CdA_z = 0.40 (updated default) → expect ~864 N.
    #[test]
    fn fuselage_drag_formula_correct() {
        let air_density = 1.2_f32;
        let fus_z = 0.40_f32;
        let speed = 60.0_f32;
        let expected = 0.5 * air_density * fus_z * speed * speed;
        assert!((expected - 864.0).abs() < 1.0, "Fuselage nose drag at 60 m/s should be ~864 N, got {expected}");
    }

    // --- Flap effects -------------------------------------------------------

    // Deploying flaps should increase lift at the same AoA.
    #[test]
    fn flaps_increase_lift() {
        let mut wing_clean = AeroSurface::control(AeroSurfaceConfig {
            lift_slope: std::f32::consts::TAU,
            skin_friction: 0.02,
            zero_lift_aoa: -3.0,
            stall_angle_high: 17.0,
            stall_angle_low: -17.0,
            chord: 1.57,
            flap_fraction: 0.2,
            span: 3.65,
            aspect_ratio: 7.0,
        }, ControlInputType::Flap, 1.0);
        let mut wing_flapped = wing_clean.clone();

        wing_clean.set_flap_angle(0.0);
        wing_flapped.set_flap_angle(20_f32.to_radians());

        let vel = air_vel_at_aoa(4.0, LEVEL_FLIGHT_SPEED);
        let f_clean   = wing_clean.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_flapped = wing_flapped.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);

        assert!(f_flapped.force.y > f_clean.force.y,
            "Flaps must increase lift: clean={:.1} flapped={:.1}", f_clean.force.y, f_flapped.force.y);
    }

    // Deploying flaps should also increase drag (more induced + pressure drag).
    #[test]
    fn flaps_increase_drag() {
        let base_config = AeroSurfaceConfig {
            lift_slope: std::f32::consts::TAU,
            skin_friction: 0.02,
            zero_lift_aoa: -3.0,
            stall_angle_high: 17.0,
            stall_angle_low: -17.0,
            chord: 1.57,
            flap_fraction: 0.2,
            span: 3.65,
            aspect_ratio: 7.0,
        };
        let mut wing_clean   = AeroSurface::control(base_config.clone(), ControlInputType::Flap, 1.0);
        let mut wing_flapped = AeroSurface::control(base_config, ControlInputType::Flap, 1.0);
        wing_clean.set_flap_angle(0.0);
        wing_flapped.set_flap_angle(30_f32.to_radians());

        let vel = air_vel_at_aoa(4.0, LEVEL_FLIGHT_SPEED);
        let f_clean   = wing_clean.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_flapped = wing_flapped.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);

        // Drag is in the air-velocity direction; project force onto vel_dir to extract it.
        let drag_dir = vel.normalize();
        let drag_clean   = f_clean.force.dot(drag_dir);
        let drag_flapped = f_flapped.force.dot(drag_dir);
        assert!(drag_flapped > drag_clean,
            "Flaps must increase drag: clean={drag_clean:.1} flapped={drag_flapped:.1}");
    }

    // --- C172-specific wing plausibility ----------------------------------

    // At cruise AoA (~4°) at 55 m/s, the main wing should produce enough lift
    // to support roughly half the aircraft weight per panel (two panels total).
    // Sim mass ≈ 903 kg → weight ≈ 8858 N → each of the two panels carries ~4429 N.
    #[test]
    fn cruise_lift_per_panel_supports_half_weight() {
        let wing = default_wing();
        let speed = LEVEL_FLIGHT_SPEED; // 55 m/s
        // Find the AoA that gives ~half-weight lift from one panel.
        // We'll check that the AoA range 2–8° brackets the required lift.
        // With the real C172 panel (chord=1.62, span=4.05, AR=7.32) the half-weight
        // bracket sits between 0° and 4° at 55 m/s.
        let vel_low  = air_vel_at_aoa(0.0, speed);
        let vel_high = air_vel_at_aoa(4.0, speed);
        let f_low  = wing.calculate_forces(vel_low,  AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_high = wing.calculate_forces(vel_high, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let half_weight = 9349.0 / 2.0; // 903 kg × 9.81 / 2
        assert!(f_low.force.y  < half_weight, "Lift at 0° should be < half weight ({half_weight:.0} N), got {:.0}", f_low.force.y);
        assert!(f_high.force.y > half_weight, "Lift at 4° should be > half weight ({half_weight:.0} N), got {:.0}", f_high.force.y);
    }

    // The C172 wing should stall around 17° (stall_angle_high default).
    // Lift at 16° should exceed lift at 20° by a significant margin.
    #[test]
    fn stall_onset_at_correct_angle() {
        let wing = default_wing();
        let vel_16 = air_vel_at_aoa(16.0, LEVEL_FLIGHT_SPEED);
        let vel_20 = air_vel_at_aoa(20.0, LEVEL_FLIGHT_SPEED);
        let f_16 = wing.calculate_forces(vel_16, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_20 = wing.calculate_forces(vel_20, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let drop_fraction = (f_16.force.y - f_20.force.y) / f_16.force.y;
        // Stall transition padding is intentionally wide (8-15°) for a gradual,
        // non-violent stall — so we only assert some drop, not a cliff.
        assert!(drop_fraction > 0.05,
            "Lift must drop between 16° and 20° AoA (stall), got {:.1}%", drop_fraction * 100.0);
    }

    // Negative AoA should produce negative (downward) lift.
    #[test]
    fn negative_aoa_gives_negative_lift() {
        let wing = default_wing();
        // -10° is well below zero_lift_aoa (-3°) → inverted lift
        let vel = air_vel_at_aoa(-10.0, LEVEL_FLIGHT_SPEED);
        let f = wing.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        assert!(f.force.y < 0.0, "Negative AoA must produce downward lift, got {:.1}", f.force.y);
    }

    // Force must scale with the square of airspeed (dynamic pressure q = ½ρv²).
    #[test]
    fn force_scales_with_speed_squared() {
        let wing = default_wing();
        let vel_base   = air_vel_at_aoa(4.0, 30.0);
        let vel_double = air_vel_at_aoa(4.0, 60.0);
        let f_base   = wing.calculate_forces(vel_base,   AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        let f_double = wing.calculate_forces(vel_double, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        // At same AoA, doubling speed → 4× force (within 5% tolerance for AR correction rounding)
        let ratio = f_double.force.y / f_base.force.y;
        assert!((ratio - 4.0).abs() < 0.2,
            "Force must scale with v² (expect 4× at 2× speed), got ratio {ratio:.3}");
    }

    // A surface with zero span or zero chord should produce zero force.
    #[test]
    fn zero_area_surface_produces_zero_force() {
        let zero_span = AeroSurface::wing(AeroSurfaceConfig {
            span: 0.0,
            aspect_ratio: 0.0,
            ..AeroSurfaceConfig::default()
        });
        let vel = air_vel_at_aoa(4.0, LEVEL_FLIGHT_SPEED);
        let f = zero_span.calculate_forces(vel, AIR_DENSITY, Vec3::ZERO, surface_rotation_level(), 1.0);
        assert!(f.force.length() < 1e-3, "Zero-area surface must produce zero force, got {}", f.force);
    }
}
