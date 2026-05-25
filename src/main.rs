use avian3d::prelude::{
    AngularDamping, AngularInertia, AngularVelocity, CenterOfMass, Gravity,
    LinearVelocity, Mass, PhysicsPlugins, RigidBody, TransformInterpolation,
};
use bevy::{
    asset::AssetMetaCheck,
    camera::RenderTarget,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    image::ImageSampler,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
    window::WindowResized,
};

use crate::camera::{
    CameraMode, FreeCam, OuterCamera, TrackCam,
    PIXEL_HEIGHT, PIXEL_LAYER, PIXEL_WIDTH, SCREEN_LAYER,
    free_cam_control, track_cam_control, toggle_camera_mode,
};
use bevy_egui::PrimaryEguiContext;
use crate::debug_tools::debug_flight_menu::DebugFlightMenuPlugin;
use crate::debug_tools::debug_hud::{DebugHud, DebugHudText, render_debug_hud};
use crate::debug_tools::debug_world::spawn_debug_world;
use crate::fog::{FogEnabled, FogPlugin};
use crate::plane::{Airplane, PlaneState, PlaneVisual};
use crate::debug_tools::debug_gizmos::{GizmosVisible, draw_aero_gizmos, setup_gizmo_config, toggle_gizmos};
use crate::physics::aero_surface::{AeroSurface, ControlInputType};
use crate::physics::aircraft_physics::{AircraftRoot, apply_aero_forces};
use crate::physics::airplane_controller::airplane_controller;
use crate::physics::flight_config::FlightModelConfig;
use crate::physics::landing_gear::apply_landing_gear;

mod camera;
mod fog;
mod physics;
mod debug_tools;
mod plane;

#[derive(Component)]
struct FpsText;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(AssetPlugin {
                meta_check: AssetMetaCheck::Never,
                ..default()
            }),
            FrameTimeDiagnosticsPlugin::default(),
            FogPlugin,
            PhysicsPlugins::default(),
            DebugFlightMenuPlugin,
        ))
        // Initial gravity; kept in sync with cfg.gravity by
        // apply_config_to_entities so the debug slider drives the real force.
        // The same value is fed into our velocity predictor in aircraft_physics.
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .init_resource::<FlightModelConfig>()
        .init_resource::<CameraMode>()
        .init_resource::<DebugHud>()
        .init_resource::<GizmosVisible>()
        .add_systems(Startup, (setup, spawn_debug_world, setup_gizmo_config))
        .add_systems(Update, (
            // Controller and aero forces run in Update so they are in sync with
            // Avian's PhysicsSchedule, which also runs once per render frame
            // using the real frame delta rather than a fixed timestep.
            // Running them in FixedUpdate caused a rate mismatch that produced
            // the visible stepping / snapping artifacts.
            (airplane_controller, apply_aero_forces, apply_landing_gear).chain(),
            toggle_camera_mode,
            toggle_gizmos,
            free_cam_control,
            update_fps,
            fit_canvas,
            apply_config_to_entities,
            (populate_debug_hud, render_debug_hud).chain(),
        ))
        // PostUpdate: transform propagation has already run, so GlobalTransform
        // reflects the current frame position — no one-frame lag on gizmos.
        .add_systems(PostUpdate, (track_cam_control, draw_aero_gizmos))
        .run();
}

fn update_fps(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<FpsText>>,
) {
    let Ok(mut text) = query.single_mut() else { return };
    if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS)
        && let Some(value) = fps.smoothed() {
            **text = format!("FPS: {:.0}", value);
        }
}

fn populate_debug_hud(
    mut hud: ResMut<DebugHud>,
    mode: Res<CameraMode>,
    fog: Res<FogEnabled>,
    cam_query: Query<&Transform, With<FreeCam>>,
    plane_query: Query<(&Transform, &PlaneState, &AircraftRoot), With<Airplane>>,
    cfg: Res<FlightModelConfig>,
) {
    hud.entries.clear();

    hud.entries.push(("CAM", match &*mode {
        CameraMode::Free  => "FREE".into(),
        CameraMode::Orbit => "ORBIT".into(),
        CameraMode::Chase => "CHASE".into(),
    }));
    hud.entries.push(("FOG", if fog.0 { "ON  [1]" } else { "OFF [1]" }.into()));

    if let Ok(tf) = cam_query.single() {
        let p = tf.translation;
        hud.entries.push(("POS", format!("X={:.1}  Y={:.1}  Z={:.1}", p.x, p.y, p.z)));
    }

    if let Ok((tf, state, root)) = plane_query.single() {
        let (yaw, pitch, roll) = tf.rotation.to_euler(EulerRot::YXZ);
        hud.entries.push(("SPD",    format!("{:.1} m/s", state.speed)));
        hud.entries.push(("ALT",    format!("{:.1} m",   tf.translation.y)));
        hud.entries.push(("GND",    if state.on_ground { "ON GROUND" } else { "AIRBORNE" }.into()));
        hud.entries.push(("BRK",    if state.braking { "ON  [B]" } else { "OFF [B]" }.into()));
        hud.entries.push(("THR",    format!("{:.0}%",    root.throttle_percent * 100.0)));
        hud.entries.push(("FLAPS",  format!("{:.0} degrees",    root.flap_setting.to_degrees())));
        hud.entries.push(("THRUST", format!("{:.0} N",   cfg.thrust_max * root.throttle_percent)));
        hud.entries.push(("DRAG",   format!("{:.0} N",    state.drag)));
        hud.entries.push(("  SURF", format!("{:.0} N",    state.drag_surface)));
        hud.entries.push(("  FUSE", format!("{:.0} N",    state.drag_fuselage)));
        hud.entries.push(("LIFT",   format!("{:.0}%",    state.lift_pct * 100.0)));
        hud.entries.push(("PITCH",  format!("{:.1}",    pitch.to_degrees())));
        hud.entries.push(("ROLL",   format!("{:.1}",    roll.to_degrees())));
        hud.entries.push(("YAW",    format!("{:.1}",    yaw.to_degrees())));
    }
}

/// Pushes debug-menu values that live outside the config resource back onto
/// the world whenever the config changes: the visual mesh offset, the rigid-body
/// mass properties (mass, inertia, CoM, damping), Avian's gravity, and each
/// surface's full aero config.
///
/// Surfaces are matched to their config by control input type, mirroring how
/// `spawn_aircraft` built them, so editing `cfg.wing` updates both wing panels,
/// `cfg.aileron` updates both ailerons, and so on.
fn apply_config_to_entities(
    cfg: Res<FlightModelConfig>,
    mut visual_q: Query<&mut Transform, With<PlaneVisual>>,
    mut body_q: Query<
        (&mut CenterOfMass, &mut Mass, &mut AngularInertia, &mut AngularDamping),
        With<Airplane>,
    >,
    mut surface_q: Query<&mut AeroSurface>,
    mut gravity: ResMut<Gravity>,
) {
    if !cfg.is_changed() {
        return;
    }
    // Drive Avian's actual gravity from the config so the slider really changes
    // the force the aircraft feels (not just the predictor / HUD estimate).
    gravity.0 = Vec3::NEG_Y * cfg.gravity;
    for mut tf in &mut visual_q {
        tf.translation = cfg.model_offset;
    }
    // Empty airframe + current fuel/cargo/occupant load → effective mass,
    // CoM (metres), and inertia. Avian recomputes its solver mass properties
    // when these components change.
    let (mass_eff, com_eff, inertia_eff) = cfg.loaded_mass_properties();
    for (mut com, mut mass, mut inertia, mut damping) in &mut body_q {
        com.0 = com_eff;
        mass.0 = mass_eff;
        inertia.principal = inertia_eff;
        damping.0 = cfg.angular_damping;
    }
    for mut surface in &mut surface_q {
        let new_config = match (surface.is_control_surface, surface.input_type) {
            (true, ControlInputType::Flap)  => &cfg.wing,
            (true, ControlInputType::Roll)  => &cfg.aileron,
            (true, ControlInputType::Pitch) => &cfg.elevator,
            (true, ControlInputType::Yaw)   => &cfg.rudder,
            (false, _)                      => &cfg.body_lift,
        };
        surface.config = new_config.clone();
    }
}

fn fit_canvas(
    mut events: MessageReader<WindowResized>,
    mut projection: Single<&mut Projection, With<OuterCamera>>,
) {
    let Projection::Orthographic(projection) = &mut **projection else { return };
    for event in events.read() {
        let h_scale = event.width / PIXEL_WIDTH as f32;
        let v_scale = event.height / PIXEL_HEIGHT as f32;
        projection.scale = 1.0 / h_scale.min(v_scale);
    }
}

/// Cessna 172 approximate surface layout.
/// All local positions are in LOCAL space of the aircraft entity (scale 0.1),
/// so local x=55 → world x=5.5 m, giving a realistic moment arm.
fn spawn_aircraft(commands: &mut Commands, asset_server: &AssetServer, cfg: &FlightModelConfig) -> Entity {
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
    let dihedral = 1.5_f32.to_radians();
    // Main wing panel is the inboard section (~2.0 m semispan center); the
    // ailerons sit just outboard of it. Mounting the panel center at local 20
    // keeps the wingtip near 5.5 m (≈11 m total span) instead of overlapping
    // the aileron panels, and gives a realistic dihedral roll moment arm.
    const WING_X: f32 = 20.0;
    let dihedral_h = WING_X * dihedral.tan();
    let dihedral_rot = Quat::from_rotation_z(dihedral) * wing_rot;

    // Visual wings sit ~1 m (10 local units) above the entity origin after the mesh offset.
    const WING_Y: f32 = 10.0;

    // Inboard wing panels carry the flaps (flap_fraction 0.2), so they're Flap
    // control surfaces; ailerons are separate smaller panels outboard. At flaps
    // 0 the deflection is zero, so they behave exactly like fixed lift surfaces.
    let left_wing = commands.spawn((
        AeroSurface::control(wing_config.clone(), ControlInputType::Flap, 1.0),
        Transform::from_xyz(-WING_X, WING_Y + dihedral_h, 0.0).with_rotation(dihedral_rot),
    )).id();

    let right_wing = commands.spawn((
        AeroSurface::control(wing_config.clone(), ControlInputType::Flap, 1.0),
        Transform::from_xyz(WING_X, WING_Y + dihedral_h, 0.0).with_rotation(dihedral_rot),
    )).id();

    // Ailerons: outer 28% of wing span, 35% chord flap — C172 ~1.5m span each
    let aileron_config = cfg.aileron.clone();

    // Outer ~30% of semispan (3.65+1.5=5.15 m total semispan, aileron centered at ~4.4 m = local 44)
    let aileron_dihedral_h = 44.0 * dihedral.tan();
    let aileron_l = commands.spawn((
        AeroSurface::control(aileron_config.clone(), ControlInputType::Roll, -1.0),
        Transform::from_xyz(-44.0, WING_Y + aileron_dihedral_h, 0.0).with_rotation(dihedral_rot),
    )).id();

    let aileron_r = commands.spawn((
        AeroSurface::control(aileron_config, ControlInputType::Roll, 1.0),
        Transform::from_xyz(44.0, WING_Y + aileron_dihedral_h, 0.0).with_rotation(dihedral_rot),
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
    let visual = commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/scene.gltf#Scene0")),
        Transform::from_xyz(0.0, -10.0, 0.0),
        PlaneVisual,
        PIXEL_LAYER,
    )).id();

    let (mass_eff, com_eff, inertia_eff) = cfg.loaded_mass_properties();
    commands.spawn((
        // Airborne spawn (250 m up, 65 m/s forward) — kept for reference.
        // Transform::from_xyz(0.0, 250.0, 0.0).with_scale(Vec3::splat(0.1)),
        // Sit on the runway: gear rest_length - mount_height = 1.1 - (-0.15) = 1.25 m,
        // so the struts just touch the ground and settle onto their springs.
        // Near the -Z threshold (runway spans z = -1000..1000) so the full
        // length is ahead for the takeoff roll in +Z.
        Transform::from_xyz(0.0, 1.25, -900.0).with_scale(Vec3::splat(0.1)),
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
    .add_children(&[visual, left_wing, right_wing, aileron_l, aileron_r, body_left, body_right, elevator, rudder])
    .id()
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
    cfg: Res<FlightModelConfig>,
) {
    let canvas_size = Extent3d { width: PIXEL_WIDTH, height: PIXEL_HEIGHT, ..default() };
    let mut canvas = Image {
        texture_descriptor: TextureDescriptor {
            label: None,
            size: canvas_size,
            dimension: TextureDimension::D2,
            format: TextureFormat::Bgra8UnormSrgb,
            mip_level_count: 1,
            sample_count: 1,
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        },
        ..default()
    };
    canvas.resize(canvas_size);
    canvas.sampler = ImageSampler::nearest();
    let pixel_target = images.add(canvas);

    spawn_aircraft(&mut commands, &asset_server, &cfg);

    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.45, 0.65, 0.9)),
            ..default()
        },
        RenderTarget::Image(pixel_target.clone().into()),
        Msaa::Off,
        PIXEL_LAYER,
        Transform::from_xyz(0.0, 8.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        FreeCam { yaw: 0.0, pitch: -0.3 },
        TrackCam { yaw: 0.0, pitch: 0.3, distance: 15.0, chase_distance: 6.0 },
    ));

    commands.spawn((Sprite::from_image(pixel_target), SCREEN_LAYER));

    let initial_scale = windows.single().ok().map(|w| {
        let h = w.width() / PIXEL_WIDTH as f32;
        let v = w.height() / PIXEL_HEIGHT as f32;
        1.0 / h.min(v).round()
    }).unwrap_or(0.25);

    commands.spawn((
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scale: initial_scale,
            ..OrthographicProjection::default_2d()
        }),
        Msaa::Off,
        SCREEN_LAYER,
        OuterCamera,
        // Pins the egui primary context to this camera so the debug panel
        // renders at window resolution rather than into the pixel render target.
        PrimaryEguiContext,
    ));

    commands.spawn((
        Text::new("FPS: --"),
        FpsText,
        TextColor(Color::linear_rgb(1.0, 1.0, 0.0)),
        TextFont { font_size: 16.0, ..default() },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));

    commands.spawn((
        Text::new(""),
        DebugHudText,
        TextColor(Color::linear_rgb(1.0, 1.0, 1.0)),
        TextFont { font_size: 16.0, ..default() },
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(28.0),
            left: Val::Px(8.0),
            ..default()
        },
    ));
}
