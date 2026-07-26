use avian3d::prelude::*;
use bevy::prelude::*;

use crate::{camera::PIXEL_LAYER, physics::{aero_surface::{AeroSurface, ControlInputType}, aircraft_physics::AircraftRoot, flight_config::FlightModelConfig}};

/// Marker for every flyable aircraft entity.
#[derive(Component)]
pub struct Airplane;

/// Origin-runway spawn point, shared by the initial `setup()` spawn in
/// main.rs and the "Reset Plane" button (ui::plane_menu) so both place the
/// aircraft identically. `SPAWN_Y` is added to the live terrain height at
/// (SPAWN_X, SPAWN_Z) — see `spawn_position` below — since the runway isn't
/// flat across the whole map.
pub const SPAWN_X: f32 = 0.0;
pub const SPAWN_Z: f32 = -900.0;
/// Gear standoff (rest_length - mount_height = 1.1 - (-0.15) = 1.25 m) added
/// on top of the terrain height so the struts just touch down and settle
/// onto their springs instead of spawning embedded in the ground.
pub const GEAR_STANDOFF: f32 = 1.25;

/// Computes the world-space spawn position on the origin runway, given the
/// current terrain height there.
pub fn spawn_position(world_gen: &crate::terrain::WorldGenerator) -> Vec3 {
    let spawn_y = world_gen.get_terrain_height(SPAWN_X, SPAWN_Z) + GEAR_STANDOFF;
    Vec3::new(SPAWN_X, spawn_y, SPAWN_Z)
}

/// Puts the aircraft back on the runway at the spawn position/orientation,
/// zeroing velocities and transient flight state. Shared by the "Reset Plane
/// to Runway" UI buttons, the crash-recovery hotkey, and the terrain-regen
/// auto-recovery, so all three reset paths can't drift apart.
pub fn reset_to_runway(
    transform: &mut Transform,
    lin_vel: &mut avian3d::prelude::LinearVelocity,
    ang_vel: &mut avian3d::prelude::AngularVelocity,
    state: &mut PlaneState,
    root: &mut crate::physics::aircraft_physics::AircraftRoot,
    world_gen: &crate::terrain::WorldGenerator,
) {
    *transform = Transform::from_translation(spawn_position(world_gen)).with_scale(Vec3::splat(0.1));
    lin_vel.0 = Vec3::ZERO;
    ang_vel.0 = Vec3::ZERO;
    state.crashed = false;
    root.throttle_percent = 0.0;
}

/// Marker for the child entity holding the visual GLTF scene, so its local
/// offset can be adjusted at runtime (see `model_offset` in the debug menu).
#[derive(Component)]
pub struct PlaneVisual;

/// Marker on the propeller scene root, so `spin_propeller` can rotate it
/// about the engine's spin axis each frame.
#[derive(Component)]
pub struct Propeller;

/// Marker on the fixed vertical stabilizer (the fin the rudder hinges to).
/// Distinguishes it from the `body_lift` panels, which are also non-control
/// `AeroSurface::wing`s but use a different config slot in
/// `apply_config_to_entities`.
#[derive(Component)]
pub struct VerticalFin;

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
    /// Manually toggled by the cockpit PARK switch — holds the brakes on
    /// regardless of throttle/speed, in addition to the automatic
    /// throttle-idle-and-stopped parking brake in `apply_landing_gear`.
    pub parking_brake_set: bool,
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
/// `dihedral_sign`: +1.0 for left panels, -1.0 for right panels — the tip
/// (the end away from the fuselage) always rises for either sign.
pub fn wing_panel_rotation(incidence_deg: f32, dihedral_sign: f32) -> Quat {
    // Ry(-90°) maps span (local Z) to world -X. Apply incidence about that
    // span axis before any dihedral so both panels share the same rotation axis.
    let wing_rot = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let incidence_rot = wing_rot * Quat::from_rotation_z(incidence_deg.to_radians());
    // Dihedral tilts the already-incidenced panel tip up/down about world Z.
    // The left panel's span axis points toward world -X (see Ry(-90°) above),
    // and Rz(+angle) rotates -X toward -Y (standard right-hand rotation about
    // +Z) — so a *positive* Rz angle on the left panel would push its tip
    // DOWN, not up. Negating `dihedral_sign` here keeps the sign flip local
    // to this function so every call site can keep the intuitive "+1.0 =
    // left, -1.0 = right" convention while both tips actually rise.
    Quat::from_rotation_z(-dihedral_sign * WING_DIHEDRAL_DEG.to_radians()) * incidence_rot
}

/// Identifies which of the seven mounted `AeroSurface`s a translation/rotation
/// is being computed for — lets `surface_rigging` be the single source of
/// truth for placement, shared by the initial spawn in `spawn_aircraft` and
/// the live resync in `apply_config_to_entities`, so the two can't drift
/// apart the way the old hardcoded `WING_Y`/`WING_Z` constants did.
pub enum SurfaceSlot {
    WingLeft,
    WingRight,
    AileronLeft,
    AileronRight,
    BodyLiftLeft,
    BodyLiftRight,
    Elevator,
    Rudder,
    VerticalFin,
}

/// Local translation + rotation for a mounted surface, root-chained off its
/// physical neighbour instead of computed as an independent formula per
/// surface.
///
/// The previous version placed each panel's *center* from its own formula
/// (mount X station, a flat `X * tan(dihedral)` rise, `WING_Z`/hardcoded tail
/// Z). That put the wing tip and aileron root ~0.13 m apart instead of
/// touching: rotating a panel about its own center by the dihedral angle
/// displaces its span-edge by `half_span * sin(dihedral)` beyond the
/// translation term, and the old formula only accounted for the translation.
/// Two independently-"close" numbers is not the same as two edges that
/// actually meet.
///
/// Building outward from a fixed root — `tip = root + span_dir * span` —
/// makes edge-matching exact by construction: the next panel's root **is**
/// the previous panel's tip, not a separately-computed number that has to
/// coincidentally agree.
pub fn surface_rigging(cfg: &FlightModelConfig, slot: SurfaceSlot) -> (Vec3, Quat) {
    use SurfaceSlot::*;

    // Horizontal surfaces (wing, aileron, body-lift, elevator) need local Z =
    // world X so the span axis is perpendicular to the flight direction.
    let wing_rot = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    // Rudder/fin need local Z = world Y (vertical span).
    let rudder_rot = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)
        * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);

    // Vertical rise (2 m) from the mesh origin to the wing-root mount, matching
    // the original geometry when the mesh offset was (0, -10, 0).
    const WING_ABOVE_MESH: f32 = 20.0;
    let wing_y = cfg.model_offset.y + WING_ABOVE_MESH;
    // Chordwise station of the wing lift point (local units, ×0.1 → metres).
    const WING_Z: f32 = 5.0;
    // Wing-root mount: just outboard of the fuselage, at wing height, no
    // dihedral rise (dihedral only rises panels outboard of this point).
    // Local units (metres × 10) — see `WING_ABOVE_MESH` above for the convention.
    let wing_root_x = 4.75;

    // Spans in local units (config is metres; ×10 → local, matching the
    // aircraft root's 0.1 scale).
    let wing_span = cfg.wing.span * 10.0;
    let aileron_span = cfg.aileron.span * 10.0;

    let rot_l = wing_panel_rotation(cfg.wing_incidence, 1.0);
    let rot_r = wing_panel_rotation(cfg.wing_incidence, -1.0);
    // `Ry(-90°)` (baked into every `wing_panel_rotation` result) always maps
    // local +Z to world -X, for *both* sides — dihedral_sign only tilts the
    // rise by a small angle, it doesn't flip which world direction the span
    // axis points. So `rot * Vec3::Z` points toward -X for both panels:
    // that's already outboard for the left side, but needs negating to read
    // as outboard (+X) for the right side.
    let outboard_l = rot_l * Vec3::Z;
    let outboard_r = -(rot_r * Vec3::Z);

    let root_l = Vec3::new(-wing_root_x, wing_y, WING_Z);
    let root_r = Vec3::new(wing_root_x, wing_y, WING_Z);
    let wing_center_l = root_l + outboard_l * (wing_span * 0.5);
    let wing_tip_l = root_l + outboard_l * wing_span;
    let wing_center_r = root_r + outboard_r * (wing_span * 0.5);
    let wing_tip_r = root_r + outboard_r * wing_span;
    let aileron_center_l = wing_tip_l + outboard_l * (aileron_span * 0.5);
    let aileron_center_r = wing_tip_r + outboard_r * (aileron_span * 0.5);

    // Tail cone: elevator roots on the fuselage centerline; the vertical fin
    // roots directly on top of the elevator's mount (not at `wing_y`, which
    // is the *main* wing's height and unrelated to the tail) and rises by its
    // own half-span so its root edge — not its center — lands on the tailplane.
    let tail_root = Vec3::new(0.0, wing_y - 3.0, -58.0);
    let fin_root = Vec3::new(0.0, wing_y - 3.0, -50.0);
    let fin_span = cfg.vertical_fin.span * 10.0;
    let rudder_span = cfg.rudder.span * 10.0;
    let fin_up = rudder_rot * Vec3::Z;
    let fin_center = fin_root + fin_up * (fin_span * 0.5);
    let rudder_center = tail_root + fin_up * (rudder_span * 0.5);

    match slot {
        WingLeft       => (wing_center_l, rot_l),
        WingRight      => (wing_center_r, rot_r),
        AileronLeft    => (aileron_center_l, rot_l),
        AileronRight   => (aileron_center_r, rot_r),
        BodyLiftLeft   => (Vec3::new(-10.0, wing_y, WING_Z), wing_rot),
        BodyLiftRight  => (Vec3::new(10.0, wing_y, WING_Z), wing_rot),
        Elevator       => (tail_root, wing_rot),
        Rudder         => (rudder_center, rudder_rot),
        VerticalFin    => (fin_center, rudder_rot),
    }
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
    let fin_config = cfg.vertical_fin.clone();

    // Children spawned separately so we can get their entity IDs. Every
    // surface's position + rotation comes from `surface_rigging`, which
    // chains each panel off its physical neighbour (wing root → wing tip →
    // aileron root → aileron tip, elevator root → fin root → fin tip) so
    // edges touch exactly instead of being independently-computed numbers
    // that only approximately agree.
    let aileron_config = cfg.aileron.clone();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::WingLeft);
    let left_wing = commands.spawn((
        AeroSurface::control(wing_config.clone(), ControlInputType::Flap, 1.0),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::WingRight);
    let right_wing = commands.spawn((
        AeroSurface::control(wing_config.clone(), ControlInputType::Flap, 1.0),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::AileronLeft);
    let aileron_l = commands.spawn((
        AeroSurface::control(aileron_config.clone(), ControlInputType::Roll, -1.0),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::AileronRight);
    let aileron_r = commands.spawn((
        AeroSurface::control(aileron_config, ControlInputType::Roll, 1.0),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    // Body lift surfaces (non-control)
    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::BodyLiftLeft);
    let body_left = commands.spawn((
        AeroSurface::wing(cfg.body_lift.clone()),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::BodyLiftRight);
    let body_right = commands.spawn((
        AeroSurface::wing(cfg.body_lift.clone()),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::Elevator);
    let elevator = commands.spawn((
        AeroSurface::control(stab_h, ControlInputType::Pitch, 1.0),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::Rudder);
    let rudder = commands.spawn((
        AeroSurface::control(stab_v, ControlInputType::Yaw, 1.0),
        Transform::from_translation(pos).with_rotation(rot),
    )).id();

    // Fixed vertical fin, just forward of the rudder — the non-control tail
    // fin the rudder hinges to. Provides yaw weathervaning and sideslip drag
    // even with no rudder input, matching the real C172's fixed-fin +
    // hinged-rudder split (the rudder alone has no fixed portion).
    let (pos, rot) = surface_rigging(cfg, SurfaceSlot::VerticalFin);
    let vertical_fin = commands.spawn((
        AeroSurface::wing(fin_config),
        VerticalFin,
        Transform::from_translation(pos).with_rotation(rot),
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
        // Not a child of `visual`, so it needs its own tag to be hidden
        // alongside the body mesh in filled-gizmos mode.
        PlaneVisual,
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
    .add_children(&[visual, propeller, left_wing, right_wing, aileron_l, aileron_r, body_left, body_right, elevator, rudder, vertical_fin])
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

#[cfg(test)]
mod rigging_tests {
    use super::*;

    // Half-extent of a panel along its own span axis, in local units
    // (config span is metres; ×10 → local, ×0.5 → half).
    fn half_span_local(span_m: f32) -> f32 {
        span_m * 10.0 * 0.5
    }

    /// The wing tip and the aileron root are meant to be the same physical
    /// edge (see the spawn-time comment: "root sits flush against the wing
    /// tip, no gap, no overlap"). Verify that's actually true to sub-mm
    /// precision instead of trusting two independently-computed numbers to
    /// coincidentally agree.
    #[test]
    fn wing_tip_touches_aileron_root() {
        let cfg = FlightModelConfig::default();
        for (wing_slot, aileron_slot) in [
            (SurfaceSlot::WingLeft, SurfaceSlot::AileronLeft),
            (SurfaceSlot::WingRight, SurfaceSlot::AileronRight),
        ] {
            let (wing_pos, wing_rot) = surface_rigging(&cfg, wing_slot);
            let (aileron_pos, _) = surface_rigging(&cfg, aileron_slot);

            // Wing tip = wing center + half the wing's own span, walked along
            // the same outboard direction the chain used to place the aileron.
            let outboard = wing_rot * Vec3::Z; // sign varies by side; only used relatively below
            let wing_tip_candidate_a = wing_pos + outboard * half_span_local(cfg.wing.span);
            let wing_tip_candidate_b = wing_pos - outboard * half_span_local(cfg.wing.span);
            // The aileron root is the aileron center minus half its own span,
            // walked back toward the wing (i.e. toward whichever candidate is closer).
            let aileron_root_candidate_a = aileron_pos - outboard * half_span_local(cfg.aileron.span);
            let aileron_root_candidate_b = aileron_pos + outboard * half_span_local(cfg.aileron.span);

            let d_aa = wing_tip_candidate_a.distance(aileron_root_candidate_a);
            let d_ab = wing_tip_candidate_a.distance(aileron_root_candidate_b);
            let d_ba = wing_tip_candidate_b.distance(aileron_root_candidate_a);
            let d_bb = wing_tip_candidate_b.distance(aileron_root_candidate_b);
            let min_gap = d_aa.min(d_ab).min(d_ba).min(d_bb);

            assert!(
                min_gap < 0.01,
                "wing tip and aileron root should coincide (gap {min_gap} local units)"
            );
        }
    }

    /// Dihedral means both wingtips rise above their root — not just "away
    /// from centerline" but strictly higher in Y. A sign error in the
    /// dihedral rotation (rotating the wrong way) makes the tip sink instead,
    /// which reads visually as the wing "yawing into the ground" outboard.
    #[test]
    fn wingtips_rise_above_root() {
        let cfg = FlightModelConfig::default();
        for slot in [SurfaceSlot::WingLeft, SurfaceSlot::WingRight] {
            let (center_pos, _) = surface_rigging(&cfg, slot);
            // Root Y is always `wing_y` (no dihedral rise); center is
            // `wing_root + outboard*(span/2)` which must have risen above that.
            let wing_y = cfg.model_offset.y + 20.0;
            assert!(
                center_pos.y > wing_y,
                "wing panel center ({}) should sit above the root height ({wing_y})",
                center_pos.y,
            );
        }
    }

    /// Left and right sides must be mirror images: same rise, same span
    /// reach, opposite X sign. A one-sided sign bug (fixed on the left,
    /// still wrong on the right, or vice versa) would break this while each
    /// side individually might look plausible in isolation.
    #[test]
    fn left_and_right_wings_are_mirrored() {
        let cfg = FlightModelConfig::default();
        let (left, _) = surface_rigging(&cfg, SurfaceSlot::WingLeft);
        let (right, _) = surface_rigging(&cfg, SurfaceSlot::WingRight);
        assert!((left.y - right.y).abs() < 1e-3, "left/right wing height should match: {} vs {}", left.y, right.y);
        assert!((left.x + right.x).abs() < 1e-3, "left/right wing X should be mirrored: {} vs {}", left.x, right.x);
        assert!((left.z - right.z).abs() < 1e-3, "left/right wing Z should match: {} vs {}", left.z, right.z);
    }
}
