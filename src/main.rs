use avian3d::prelude::{
    AngularDamping, AngularInertia, CenterOfMass, Gravity,
    Mass, Physics, PhysicsPlugins, PhysicsSchedule, PhysicsStepSystems, PhysicsTime
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
    transform::TransformSystems,
};

use crate::{camera::{
    CameraMode, ChaseCam, FixedCameraMounts, FreeCam, OuterCamera, PIXEL_HEIGHT, PIXEL_LAYER, PIXEL_WIDTH, SCREEN_LAYER, TrackCam, chase_cam_control, fit_canvas, fixed_cam_control, fixed_cam_hotkeys, free_cam_control, toggle_camera_mode, toggle_fullscreen_hotkey, track_cam_control
}, debug_tools::debug_hud::{populate_debug_hud, update_fps}, plane::spawn_aircraft};
use crate::lights::{AircraftLightsPlugin, LightTimers, spawn_aircraft_lights};
use bevy_egui::{EguiPostUpdateSet, PrimaryEguiContext};
use crate::debug_tools::debug_flight_menu::DebugFlightMenuPlugin;
use crate::debug_tools::debug_hud::DebugHud;
use crate::fog::FogPlugin;
use crate::water::WaterPlugin;
use crate::plane::{Airplane, PlaneVisual, Propeller, spin_propeller, wing_panel_rotation};
use crate::debug_tools::debug_gizmos::{GizmosVisible, draw_aero_gizmos, draw_light_gizmos, setup_gizmo_config, toggle_gizmos};
use crate::physics::aero_surface::{AeroSurface, ControlInputType};
use crate::physics::aircraft_physics::apply_aero_forces;
use crate::physics::airplane_controller::{airplane_controller, flight_assist};
use crate::physics::flight_config::FlightModelConfig;
use crate::physics::hull_collision::{detect_hull_collision, react_to_crash, reset_on_crash_key};
use crate::physics::landing_gear::apply_landing_gear;

mod airport_names;
mod camera;
mod fog;
mod lights;
mod map;
mod physics;
mod debug_tools;
mod pilot_handbook;
mod plane;
mod sky;
mod terrain;
mod ui;
mod water;
mod waypoints;

use crate::terrain::{TerrainCamera, TerrainPlugin, WorldGenerator};

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
            TerrainPlugin,
            WaterPlugin,
            crate::map::MapPlugin,
            crate::pilot_handbook::PilotHandbookPlugin,
            crate::sky::SkyPlugin,
            AircraftLightsPlugin,
            crate::waypoints::WaypointsPlugin,
        ))
        .add_plugins((
            crate::ui::MenuBarPlugin,
            crate::ui::StylePlugin,
            crate::ui::WorldMenuPlugin,
            crate::ui::PlaneMenuPlugin,
            crate::ui::InstrumentPanelPlugin,
            crate::ui::CameraMenuPlugin,
        ))
        // Cap the virtual-time step so a stutter frame never gives the physics
        // integrator a huge dt that over-compresses the spring-damper struts.
        // 33 ms ≈ 30 fps minimum; anything slower is clamped, trading real-time
        // accuracy for numerical stability (springs can't blow up on lag spikes).
        // Physics runs inside FixedPostUpdate at a fixed 60 Hz timestep.
        // FixedUpdate accumulates real elapsed time and fires as many ticks as
        // needed per frame, so physics always runs at real-time speed regardless
        // of framerate. TransformInterpolation on the aircraft smooths rendering.
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        // Initial gravity; kept in sync with cfg.gravity by
        // apply_config_to_entities so the debug slider drives the real force.
        // The same value is fed into our velocity predictor in aircraft_physics.
        .insert_resource(Gravity(Vec3::NEG_Y * 9.81))
        .init_resource::<FlightModelConfig>()
        .init_resource::<CameraMode>()
        .init_resource::<FixedCameraMounts>()
        .init_resource::<DebugHud>()
        .init_resource::<GizmosVisible>()
        .add_systems(Startup, (setup, setup_gizmo_config))
        .add_systems(
            // Run force systems in PhysicsSchedule before BroadPhase so they
            // execute at the same fixed timestep as Avian's integrator.
            // This means physics advances at a constant rate regardless of
            // framerate — no slowdown at low fps. TransformInterpolation on
            // the aircraft entity handles smooth rendering between steps.
            PhysicsSchedule,
            (airplane_controller, flight_assist, apply_aero_forces, apply_landing_gear, detect_hull_collision)
                .chain()
                .after(PhysicsStepSystems::First)
                .before(PhysicsStepSystems::BroadPhase)
                .run_if(|t: Res<Time<Physics>>| !t.is_paused()),
        )
        .add_systems(Update, (
            toggle_camera_mode,
            toggle_fullscreen_hotkey,
            fixed_cam_hotkeys,
            react_to_crash,
            reset_on_crash_key,
            toggle_gizmos,
            toggle_pause,
            free_cam_control,
            update_fps,
            fit_canvas,
            apply_config_to_entities,
            spin_propeller,
            populate_debug_hud,
        ))
        // Camera runs before TransformPropagate so its GlobalTransform is current
        // by the time EguiPrimaryContextPass projects world positions to screen.
        // EguiPostUpdateSet::EndPass (which runs EguiPrimaryContextPass) is also
        // ordered after TransformPropagate to guarantee the label projection matches
        // the same-frame GlobalTransform that the 3D stalk uses.
        .add_systems(PostUpdate, (track_cam_control, chase_cam_control, fixed_cam_control).before(TransformSystems::Propagate))
        .add_systems(PostUpdate, (draw_aero_gizmos, draw_light_gizmos).after(TransformSystems::Propagate))
        .configure_sets(PostUpdate, EguiPostUpdateSet::EndPass.after(TransformSystems::Propagate))
        .run();
}


fn toggle_pause(
    keys: Res<ButtonInput<KeyCode>>,
    mut physics_time: ResMut<Time<Physics>>,
    mut virtual_time: ResMut<Time<Virtual>>,
) {
    if keys.just_pressed(KeyCode::KeyP) {
        if physics_time.is_paused() {
            physics_time.unpause();
            virtual_time.unpause();
        } else {
            physics_time.pause();
            virtual_time.pause();
        }
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
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn apply_config_to_entities(
    cfg: Res<FlightModelConfig>,
    mut visual_q: Query<&mut Transform, (With<PlaneVisual>, Without<Propeller>)>,
    // Propeller — repositioned live from `prop_position`. Disjoint from
    // `visual_q` (both touch Transform) via the marker filters.
    mut prop_q: Query<&mut Transform, (With<Propeller>, Without<PlaneVisual>)>,
    mut body_q: Query<
        (&mut CenterOfMass, &mut Mass, &mut AngularInertia, &mut AngularDamping),
        With<Airplane>,
    >,
    // Surfaces also carry their own Transform (the mounted orientation); the
    // Without filters keep this disjoint from the visual/propeller Transform
    // queries above so Bevy can run them together.
    mut surface_q: Query<(&mut AeroSurface, &mut Transform, Has<crate::plane::VerticalFin>), (Without<PlaneVisual>, Without<Propeller>)>,
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
    // Move only the propeller's translation; spin_propeller owns its rotation.
    for mut tf in &mut prop_q {
        tf.translation = cfg.propeller.prop_position;
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
    for (mut surface, mut tf, is_vertical_fin) in &mut surface_q {
        let new_config = match (surface.is_control_surface, surface.input_type) {
            (true, ControlInputType::Flap)  => &cfg.wing,
            (true, ControlInputType::Roll)  => &cfg.aileron,
            (true, ControlInputType::Pitch) => &cfg.elevator,
            (true, ControlInputType::Yaw)   => &cfg.rudder,
            (false, _) if is_vertical_fin   => &cfg.vertical_fin,
            (false, _)                      => &cfg.body_lift,
        };
        surface.config = new_config.clone();
        // Re-apply the wing rigging incidence live to the main-wing (Flap) and
        // aileron (Roll) panels. Right-side panels (X > 0) use a negative dihedral
        // sign so their outboard (+X) tip rises, matching the left-side geometry.
        if matches!(surface.input_type, ControlInputType::Flap | ControlInputType::Roll) {
            let dihedral_sign = if tf.translation.x >= 0.0 { -1.0 } else { 1.0 };
            tf.rotation = wing_panel_rotation(cfg.wing_incidence, dihedral_sign);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
    cfg: Res<FlightModelConfig>,
    world_gen: Res<WorldGenerator>,
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

    // Spawn the aircraft on the origin runway, which now sits at the natural
    // terrain height. `get_terrain_height` returns the runway surface here (the
    // spawn point is on the pavement); add the gear standoff so the struts settle.
    let aircraft = spawn_aircraft(
        &mut commands,
        &asset_server,
        &cfg,
        crate::plane::spawn_position(world_gen.as_ref()),
    );
    let light_children = spawn_aircraft_lights(&mut commands, &cfg);
    commands.entity(aircraft)
        .insert(LightTimers::default())
        .add_children(&light_children);

    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.45, 0.65, 0.9)),
            ..default()
        },
        // Depth precision is set by the near/far ratio. Geometry past the fog
        // horizon (15 km, see `fog::FogSettings::visibility`) is never visible, so
        // the far plane sits just beyond it rather than at some huge value — a
        // tighter range gives the depth buffer far more resolution at altitude,
        // which is what stops the runway slab and water plane z-fighting their
        // near-coplanar terrain when viewed from high up. `near` is held at the
        // closest the orbit cam can zoom (~3 m) without clipping the aircraft.
        Projection::Perspective(PerspectiveProjection::default()),
        RenderTarget::Image(pixel_target.clone().into()),
        Msaa::Off,
        PIXEL_LAYER,
        TerrainCamera,
        Transform::from_xyz(0.0, 8.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        FreeCam { yaw: 0.0, pitch: -0.3 },
        TrackCam { yaw: 0.0, pitch: 0.3, distance: 15.0 },
        ChaseCam { yaw: 0.0, pitch: -0.3, offset: Vec3::new(0.0, 8.0, 20.0) },
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
            right: Val::Px(8.0),
            ..default()
        },
    ));

}
