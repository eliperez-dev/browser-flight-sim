use bevy::prelude::*;

/// Marker for every flyable aircraft entity.
#[derive(Component)]
pub struct Airplane;

/// Shared output written each frame by whichever physics model is active.
/// All other systems (HUD, camera, etc.) read from here instead of from
/// model-specific components, so they stay decoupled from the active model.
#[derive(Component, Default)]
pub struct PlaneState {
    pub speed: f32,
    pub thrust: f32,
    pub drag: f32,
    /// Fraction of cruise lift — 0 = stalled, 1 = cruise, >1 = excess speed.
    pub lift_pct: f32,
}
