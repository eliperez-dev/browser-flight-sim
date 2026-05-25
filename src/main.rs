use avian3d::prelude::{
    AngularDamping, AngularInertia, CenterOfMass, Gravity,
    Mass, PhysicsPlugins
};
use bevy::{
    asset::AssetMetaCheck,
    camera::RenderTarget,
    diagnostic::FrameTimeDiagnosticsPlugin,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
    window::WindowResized,
};

use crate::{camera::{
    CameraMode, FreeCam, OuterCamera, PIXEL_HEIGHT, PIXEL_LAYER, PIXEL_WIDTH, SCREEN_LAYER, TrackCam, free_cam_control, toggle_camera_mode, track_cam_control
}, debug_tools::debug_hud::{populate_debug_hud, update_fps}, plane::spawn_aircraft};
use bevy_egui::PrimaryEguiContext;
use crate::debug_tools::debug_flight_menu::DebugFlightMenuPlugin;
use crate::debug_tools::debug_hud::{DebugHud, DebugHudText, render_debug_hud};
use crate::debug_tools::debug_world::spawn_debug_world;
use crate::fog::FogPlugin;
use crate::plane::{Airplane, DebugPropeller, PlaneVisual, spin_propeller, tag_propeller};
use crate::debug_tools::debug_gizmos::{GizmosVisible, draw_aero_gizmos, setup_gizmo_config, toggle_gizmos};
use crate::physics::aero_surface::{AeroSurface, ControlInputType};
use crate::physics::aircraft_physics::apply_aero_forces;
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
            // Locate the propeller node once its scene loads, then spin it.
            (tag_propeller, spin_propeller).chain(),
            (populate_debug_hud, render_debug_hud).chain(),
        ))
        // PostUpdate: transform propagation has already run, so GlobalTransform
        // reflects the current frame position — no one-frame lag on gizmos.
        .add_systems(PostUpdate, (track_cam_control, draw_aero_gizmos))
        .run();
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
    mut visual_q: Query<&mut Transform, (With<PlaneVisual>, Without<DebugPropeller>)>,
    // Placeholder propeller — repositioned live from `prop_position`. Disjoint
    // from `visual_q` (both touch Transform) via the marker filters.
    mut debug_prop_q: Query<&mut Transform, (With<DebugPropeller>, Without<PlaneVisual>)>,
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
    // Move only the placeholder's translation; spin_propeller owns its rotation.
    for mut tf in &mut debug_prop_q {
        tf.translation = cfg.prop_position;
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



fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

    spawn_aircraft(&mut commands, &asset_server, &mut meshes, &mut materials, &cfg);

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
