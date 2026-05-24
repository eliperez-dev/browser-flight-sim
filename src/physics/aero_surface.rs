use bevy::prelude::*;
use super::aero_surface_config::AeroSurfaceConfig;
use super::bi_vector3::BiVector3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlInputType { Pitch, Yaw, Roll, Flap }

#[derive(Component)]
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
    pub fn calculate_forces(
        &self,
        world_air_velocity: Vec3,
        air_density: f32,
        rel_pos: Vec3,
        surface_rotation: Quat,
    ) -> BiVector3 {
        let mut result = BiVector3::default();
        let c = &self.config;

        // Aspect ratio correction on lift slope
        let corrected_lift_slope = c.lift_slope * c.aspect_ratio
            / (c.aspect_ratio + 2.0 * (c.aspect_ratio + 4.0) / (c.aspect_ratio + 2.0));

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

        let coeffs = self.calculate_coefficients(aoa, corrected_lift_slope, zero_lift_aoa, stall_high, stall_low);

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
    ) -> Vec3 {
        let padding_high = f32::to_radians(f32::lerp(
            15.0, 5.0, (self.flap_angle.to_degrees() + 50.0) / 100.0,
        ));
        let padding_low = f32::to_radians(f32::lerp(
            15.0, 5.0, (-self.flap_angle.to_degrees() + 50.0) / 100.0,
        ));
        let padded_high = stall_high + padding_high;
        let padded_low = stall_low - padding_low;

        if aoa < stall_high && aoa > stall_low {
            self.coeffs_low_aoa(aoa, lift_slope, zero_lift_aoa)
        } else if aoa > padded_high || aoa < padded_low {
            self.coeffs_stall(aoa, lift_slope, zero_lift_aoa, stall_high, stall_low)
        } else {
            let (low, stall, t) = if aoa > stall_high {
                let low = self.coeffs_low_aoa(stall_high, lift_slope, zero_lift_aoa);
                let stall = self.coeffs_stall(padded_high, lift_slope, zero_lift_aoa, stall_high, stall_low);
                let t = (aoa - stall_high) / (padded_high - stall_high);
                (low, stall, t)
            } else {
                let low = self.coeffs_low_aoa(stall_low, lift_slope, zero_lift_aoa);
                let stall = self.coeffs_stall(padded_low, lift_slope, zero_lift_aoa, stall_high, stall_low);
                let t = (aoa - stall_low) / (padded_low - stall_low);
                (low, stall, t)
            };
            low.lerp(stall, t)
        }
    }

    fn coeffs_low_aoa(&self, aoa: f32, lift_slope: f32, zero_lift_aoa: f32) -> Vec3 {
        let cl = lift_slope * (aoa - zero_lift_aoa);
        let induced_angle = cl / (std::f32::consts::PI * self.config.aspect_ratio);
        let eff = aoa - zero_lift_aoa - induced_angle;
        let ct = self.config.skin_friction * eff.cos();
        let cn = (cl + eff.sin() * ct) / eff.cos();
        let cd = cn * eff.sin() + ct * eff.cos();
        let cm = -cn * self.torque_coeff_proportion(eff);
        Vec3::new(cl, cd, cm)
    }

    fn coeffs_stall(&self, aoa: f32, lift_slope: f32, zero_lift_aoa: f32, stall_high: f32, stall_low: f32) -> Vec3 {
        let cl_low = if aoa > stall_high {
            lift_slope * (stall_high - zero_lift_aoa)
        } else {
            lift_slope * (stall_low - zero_lift_aoa)
        };
        let induced = cl_low / (std::f32::consts::PI * self.config.aspect_ratio);
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
