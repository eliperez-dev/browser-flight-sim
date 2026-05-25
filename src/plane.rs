use bevy::prelude::*;

/// Marker for every flyable aircraft entity.
#[derive(Component)]
pub struct Airplane;

/// Marker for the child entity holding the visual GLTF scene, so its local
/// offset can be adjusted at runtime (see `model_offset` in the debug menu).
#[derive(Component)]
pub struct PlaneVisual;

/// Shared output written each frame by whichever physics model is active.
/// All other systems (HUD, camera, etc.) read from here instead of from
/// model-specific components, so they stay decoupled from the active model.
#[derive(Component, Default)]
pub struct PlaneState {
    pub speed: f32,
    pub thrust: f32,
    /// Total drag along the flight path (N) = surface + fuselage.
    pub drag: f32,
    /// Drag contributed by the aerodynamic surfaces (profile + induced + stall).
    pub drag_surface: f32,
    /// Drag contributed by the fuselage drag box (form drag).
    pub drag_fuselage: f32,
    /// Fraction of cruise lift — 0 = stalled, 1 = cruise, >1 = excess speed.
    pub lift_pct: f32,
}
