use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{camera::PIXEL_LAYER, physics::{aero_surface::{AeroSurface, ControlInputType}, aircraft_physics::AircraftRoot, flight_config::FlightModelConfig}};

/// Marker for every flyable aircraft entity.
#[derive(Component)]
pub struct Airplane;

/// Marker for the child entity holding the visual GLTF scene, so its local
/// offset can be adjusted at runtime (see `model_offset` in the debug menu).
#[derive(Component)]
pub struct PlaneVisual;

/// Marker on the propeller scene root, so `spin_propeller` can rotate it
/// about the engine's spin axis each frame.
#[derive(Component)]
pub struct Propeller;

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
    /// True while at least one landing-gear strut is touching the ground.
    pub on_ground: bool,
    /// True while the wheel brakes (B) are applied.
    pub braking: bool,
    /// True once a hull point has struck the terrain (see `hull_collision.rs`).
    /// Sticky until the aircraft is respawned/reset.
    pub crashed: bool,
}

/// Wing dihedral (degrees per side). Shared between the geometric wingtip rise
/// and the panel rotation so the spawn and live-update paths can't drift apart.
pub const WING_DIHEDRAL_DEG: f32 = 1.5;

/// Local→world rotation for a main-wing / aileron panel.
///
/// Build order: incidence first (tilts the chord about the span axis), then
/// dihedral (tilts the whole panel about world Z). Doing it the other way
/// around corrupts the incidence axis — the dihedral tilt shifts it off -X,
/// giving the left and right panels slightly different incidence axes and
/// producing asymmetric lift when pitching.
///
/// `dihedral_sign`: +1.0 for left panels, -1.0 for right panels.
pub fn wing_panel_rotation(incidence_deg: f32, dihedral_sign: f32) -> Quat {
    // Ry(-90°) maps span (local Z) to world -X. Apply incidence about that
    // span axis before any dihedral so both panels share the same rotation axis.
    let wing_rot = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let incidence_rot = wing_rot * Quat::from_rotation_z(incidence_deg.to_radians());
    // Dihedral tilts the already-incidenced panel tip up/down about world Z.
    Quat::from_rotation_z(dihedral_sign * WING_DIHEDRAL_DEG.to_radians()) * incidence_rot
}

/// Cessna 172 approximate surface layout.
/// All local positions are in LOCAL space of the aircraft entity (scale 0.1),
/// so local x=55 → world x=5.5 m, giving a realistic moment arm.
pub fn spawn_aircraft(
    commands: &mut Commands,
    asset_server: &AssetServer,
    cfg: &FlightModelConfig,
    spawn_pos: Vec3,
) -> Entity {
    // All surface geometry lives in `FlightModelConfig` so the debug menu can
    // tune it live; `apply_config_to_entities` keeps the spawned surfaces in
    // sync afterwards.
    let wing_config = cfg.wing.clone();
    let stab_h = cfg.elevator.clone();
    let stab_v = cfg.rudder.clone();

    // Children spawned separately so we can get their entity IDs
    // Horizontal surfaces (wings, elevator) need local Z = world X so the span axis
    // is perpendicular to the flight direction (+Z).  Without this rotation the
    // aero code zeroes out the entire forward-flight velocity component as "span",
    // leaving q = 0 and generating no lift.
    let wing_rot = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);

    // Rudder needs local Z = world Y (vertical span).
    // Compose: first Ry(-90°) then Rz(-90°) gives the correct orientation.
    let rudder_rot = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)
        * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);

    // C172 dihedral: 1.5° per side. Both wingtips sit above the root.
    // Rz(+dihedral) tilts the surface so sideslip in a bank creates asymmetric AoA
    // on the two wings, generating a natural restoring roll moment.
    let dihedral = WING_DIHEDRAL_DEG.to_radians();
    // Wing panel centre sits at X=25 local (2.5 m) so the root falls at 0.475 m
    // from the centreline (just outboard of the fuselage) and the tip at 4.525 m.
    // The aileron root is flush against that tip (no gap, no overlap).
    // Combined: wing (4.05 m) + aileron (0.95 m) = 5.0 m per side → 11.0 m span.
    const WING_X: f32 = 25.0;
    let dihedral_h = WING_X * dihedral.tan();
    // Left panels: Rz(+dihedral) raises their -X tip (outboard).
    // Right panels: Rz(-dihedral) raises their +X tip (outboard).
    let rot_l = wing_panel_rotation(cfg.wing_incidence,  1.0);
    let rot_r = wing_panel_rotation(cfg.wing_incidence, -1.0);

    // Visual wings sit ~1 m (10 local units) above the entity origin after the mesh offset.
    const WING_Y: f32 = 10.0;

    // Chordwise station of the wing lift point (local units, ×0.1 → metres).
    const WING_Z: f32 = 5.0;

    let left_wing = commands.spawn((
        AeroSurface::control(wing_config.clone(), ControlInputType::Flap, 1.0),
        Transform::from_xyz(-WING_X, WING_Y + dihedral_h, WING_Z).with_rotation(rot_l),
    )).id();

    let right_wing = commands.spawn((
        AeroSurface::control(wing_config.clone(), ControlInputType::Flap, 1.0),
        Transform::from_xyz(WING_X, WING_Y + dihedral_h, WING_Z).with_rotation(rot_r),
    )).id();

    // Aileron centre at X=50 local (5.0 m): root at 4.525 m (flush with wing tip),
    // tip at 5.475 m. Full semispan ≈ 5.475 m → wingspan ≈ 10.95 m ≈ 11 m.
    let aileron_config = cfg.aileron.clone();

    let aileron_dihedral_h = 50.0 * dihedral.tan();
    let aileron_l = commands.spawn((
        AeroSurface::control(aileron_config.clone(), ControlInputType::Roll, -1.0),
        Transform::from_xyz(-50.0, WING_Y + aileron_dihedral_h, WING_Z).with_rotation(rot_l),
    )).id();

    let aileron_r = commands.spawn((
        AeroSurface::control(aileron_config, ControlInputType::Roll, 1.0),
        Transform::from_xyz(50.0, WING_Y + aileron_dihedral_h, WING_Z).with_rotation(rot_r),
    )).id();

    // Body lift surfaces (non-control)
    let body_left = commands.spawn((
        AeroSurface::wing(cfg.body_lift.clone()),
        Transform::from_xyz(-10.0, WING_Y, 5.0).with_rotation(wing_rot),
    )).id();

    let body_right = commands.spawn((
        AeroSurface::wing(cfg.body_lift.clone()),
        Transform::from_xyz(10.0, WING_Y, 5.0).with_rotation(wing_rot),
    )).id();

    let elevator = commands.spawn((
        AeroSurface::control(stab_h, ControlInputType::Pitch, 1.0),
        Transform::from_xyz(0.0, WING_Y - 3.0, -58.0).with_rotation(wing_rot),
    )).id();

    let rudder = commands.spawn((
        AeroSurface::control(stab_v, ControlInputType::Yaw, 1.0),
        Transform::from_xyz(0.0, 10.0, -58.0).with_rotation(rudder_rot),
    )).id();

    // Visual mesh is a separate child so it can be offset independently of the physics origin.
    // The model's Y origin is at the belly; shifting it down -10 local units (-1 m world)
    // aligns the fuselage center with the CoM and the simulated wing positions.
    // Body and propeller are exported as separate GLTFs from the same source model, sharing
    // the same coordinate space, so both mount at the same local offset.
    let visual = commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/body.glb#Scene0")),
        Transform::from_xyz(0.0, -10.0, 0.0),
        PlaneVisual,
        PIXEL_LAYER,
    )).id();

    // Propeller hub, in the same local space as the body (glb metres × 10 = local
    // units), offset by the same -10 Y as the body so the two models still line up.
    // The propeller mesh's own origin is not at its hub — its vertices sit ~1.5 m
    // up and 2.3 m forward of the model origin in the glb's coordinates — so
    // `Propeller` is a pivot at the hub, and the scene mesh underneath it is
    // shifted back by that same amount, keeping it visually in place while making
    // `spin_propeller`'s rotation happen about the hub instead of the mesh origin.
    const PROP_HUB_GLB: Vec3 = Vec3::new(-0.0015, 1.526, 2.317);
    let prop_hub_local = PROP_HUB_GLB * 10.0 + Vec3::new(0.0, -10.0, 0.0);
    let propeller_mesh = commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/propeller.glb#Scene0")),
        Transform::from_translation(-PROP_HUB_GLB * 10.0),
        PIXEL_LAYER,
    )).id();
    let propeller = commands.spawn((
        Transform::from_translation(prop_hub_local),
        Propeller,
        Name::new("propeller"),
    )).add_children(&[propeller_mesh]).id();

    let (mass_eff, com_eff, inertia_eff) = cfg.loaded_mass_properties();
    commands.spawn((
        // Spawn on the runway: `spawn_pos` is positioned by the caller at the
        // origin strip's surface plus the gear standoff (rest_length - mount_height
        // = 1.1 - (-0.15) = 1.25 m), so the struts just touch the ground and settle
        // onto their springs. It sits near the -Z threshold (runway spans
        // z = -1000..1000) so the full length is ahead for the takeoff roll in +Z.
        Transform::from_translation(spawn_pos).with_scale(Vec3::splat(0.1)),
        Visibility::default(),
        Airplane,
        PlaneState::default(),
        // Spawn with the throttle closed (default is full thrust).
        AircraftRoot { throttle_percent: 0.0, ..default() },
        // Avian rigid body. Mass, inertia, and CoM are the empty airframe plus
        // the current loadout (apply_config_to_entities keeps them in sync as
        // the debug menu tunes them). Principal moments are about the BODY axes
        // (X=pitch, Y=yaw, Z=roll); CoM is in metres.
        RigidBody::Dynamic,
        Mass(mass_eff),
        AngularInertia::new(inertia_eff),
        // Forward launch velocity — kept for reference.
        // LinearVelocity(Vec3::new(0.0, 0.0, 65.0)),
        LinearVelocity(Vec3::ZERO),
        AngularVelocity(Vec3::ZERO),
        AngularDamping(cfg.angular_damping),
        CenterOfMass(com_eff),
        TransformInterpolation,
        PIXEL_LAYER,
    ))
    .add_children(&[visual, propeller, left_wing, right_wing, aileron_l, aileron_r, body_left, body_right, elevator, rudder])
    .id()
}

/// Spins every tagged propeller about the configured local axis at the engine's
/// live, spooled RPM (`AircraftRoot::engine_rps`), so the visual winds up and
/// down with the engine. Purely visual; the flight model is untouched.
pub fn spin_propeller(
    time: Res<Time>,
    cfg: Res<FlightModelConfig>,
    root_q: Query<&AircraftRoot, With<Airplane>>,
    mut prop_q: Query<&mut Transform, With<Propeller>>,
) {
    let Ok(root) = root_q.single() else { return };
    let angle = root.engine_rps * std::f32::consts::TAU * time.delta_secs();
    let axis = cfg.propeller.prop_spin_axis.normalize_or(Vec3::Z);
    let delta = Quat::from_axis_angle(axis, angle);
    for mut transform in &mut prop_q {
        transform.rotation *= delta;
    }
}
