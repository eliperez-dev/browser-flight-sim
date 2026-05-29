use bevy::prelude::*;

#[derive(Clone, Component)]
pub struct AeroSurfaceConfig {
    pub lift_slope: f32,
    pub skin_friction: f32,
    pub zero_lift_aoa: f32,   // degrees
    pub stall_angle_high: f32, // degrees
    pub stall_angle_low: f32,  // degrees
    pub chord: f32,
    pub flap_fraction: f32,
    pub span: f32,
    pub aspect_ratio: f32,
}

impl Default for AeroSurfaceConfig {
    fn default() -> Self {
        let chord = 1.57_f32;
        let span = 2.1_f32;
        Self {
            lift_slope: std::f32::consts::TAU,
            skin_friction: 0.02,
            zero_lift_aoa: -3.0,
            stall_angle_high: 17.0,
            stall_angle_low: -17.0,
            chord,
            flap_fraction: 0.0,
            span,
            aspect_ratio: span / chord,
        }
    }
}

impl AeroSurfaceConfig {
    pub fn stabilizer(span: f32, chord: f32) -> Self {
        Self {
            lift_slope: std::f32::consts::TAU,
            skin_friction: 0.02,
            zero_lift_aoa: 0.0,
            stall_angle_high: 15.0,
            stall_angle_low: -15.0,
            chord,
            flap_fraction: 0.35,
            span,
            aspect_ratio: span / chord,
        }
    }
}
