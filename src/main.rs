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
use crate::debug_world::spawn_debug_world;
use crate::plane::{Airplane, move_airplane};

mod camera;
mod debug_world;
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
        ))
        .init_resource::<CameraMode>()
        .add_systems(Startup, (setup, spawn_debug_world))
        .add_systems(Update, (
            toggle_camera_mode,
            free_cam_control,
            track_cam_control,
            move_airplane,
            update_fps,
            fit_canvas,
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

    // Plane faces -Z (Bevy forward) so it flies along the -Z axis
    commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/scene.gltf#Scene0")),
        Transform::from_xyz(0.0, 0.5, 0.0).with_scale(Vec3::splat(0.1)),
        Airplane,
        PIXEL_LAYER,
    ));

    // Camera starts in Free mode; TrackCam state is also stored for when F is pressed.
    // TrackCam yaw=0 puts the orbit behind the plane (+Z), pitch lifts it slightly above.
    commands.spawn((
        Camera3d::default(),
        Camera { order: -1, ..default() },
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
}
