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
use crate::debug_hud::{DebugHud, DebugHudText, render_debug_hud};
use crate::debug_world::spawn_debug_world;
use crate::fog::{FogEnabled, FogPlugin};
use crate::plane::{Airplane, PlaneState};
use crate::physics::simple::{SimplePlanePhysics, simple_plane_physics};

mod camera;
mod debug_hud;
mod debug_world;
mod fog;
mod physics;
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
        ))
        .init_resource::<CameraMode>()
        .init_resource::<DebugHud>()
        .add_systems(Startup, (setup, spawn_debug_world))
        .add_systems(Update, (
            toggle_camera_mode,
            free_cam_control,
            update_fps,
            fit_canvas,
            // physics must settle before the tracking camera reads the plane transform
            (simple_plane_physics, track_cam_control).chain(),
            // populate must run before render so render always sees the current frame's data
            (populate_debug_hud, render_debug_hud).chain(),
        ))
        .run();
}

fn update_fps(
    diagnostics: Res<DiagnosticsStore>,
    mut query: Query<&mut Text, With<FpsText>>,
) {
    let Ok(mut text) = query.single_mut() else { return };
    if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS) {
        if let Some(value) = fps.smoothed() {
            **text = format!("FPS: {:.0}", value);
        }
    }
}

/// Clears and repopulates the debug overlay each frame.
/// Add new entries here as more flight-physics values become available.
fn populate_debug_hud(
    mut hud: ResMut<DebugHud>,
    mode: Res<CameraMode>,
    fog: Res<FogEnabled>,
    cam_query: Query<&Transform, With<FreeCam>>,
    plane_query: Query<(&Transform, &PlaneState, Option<&SimplePlanePhysics>), With<Airplane>>,
) {
    hud.entries.clear();

    // Camera mode
    hud.entries.push(("CAM", match &*mode {
        CameraMode::Free  => "FREE".into(),
        CameraMode::Track => "TRACK".into(),
    }));

    hud.entries.push(("FOG", if fog.0 { "ON  [1]" } else { "OFF [1]" }.into()));

    // Camera world position, rounded to one decimal place
    if let Ok(tf) = cam_query.single() {
        let p = tf.translation;
        hud.entries.push(("POS", format!("X={:.1}  Y={:.1}  Z={:.1}", p.x, p.y, p.z)));
    }

    // Flight physics — read from PlaneState so the HUD works with any model
    if let Ok((tf, state, simple)) = plane_query.single() {
        let (yaw, pitch, roll) = tf.rotation.to_euler(EulerRot::YXZ);
        hud.entries.push(("SPD",    format!("{:.1} m/s",  state.speed)));
        hud.entries.push(("ALT",    format!("{:.1} m",    tf.translation.y)));
        if let Some(s) = simple {
            hud.entries.push(("THR", format!("{:.0}%", s.throttle * 100.0)));
        }
        hud.entries.push(("THRUST", format!("{:.1} m/s²", state.thrust)));
        hud.entries.push(("DRAG",   format!("{:.1} m/s²", state.drag)));
        hud.entries.push(("LIFT",   format!("{:.0}%",     state.lift_pct * 100.0)));
        hud.entries.push(("PITCH",  format!("{:.1} deg",  pitch.to_degrees())));
        hud.entries.push(("ROLL",   format!("{:.1} deg",  roll.to_degrees())));
        hud.entries.push(("YAW",    format!("{:.1} deg",  yaw.to_degrees())));
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
        projection.scale = 1.0 / h_scale.min(v_scale).round();
    }
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
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

    commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/scene.gltf#Scene0")),
        Transform::from_xyz(0.0, 0.5, 0.0).with_scale(Vec3::splat(0.1)),
        Airplane,
        PlaneState::default(),
        SimplePlanePhysics::default(),
        PIXEL_LAYER,
    ));

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
        TrackCam { yaw: 0.0, pitch: 0.3, distance: 15.0 },
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
    ));

    // FPS keeps its own text so it can use the diagnostics smoothing independently
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

    // Single text entity for the generic debug overlay; content is driven by DebugHud entries
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
