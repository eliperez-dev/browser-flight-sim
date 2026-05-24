use bevy::{
    asset::AssetMetaCheck,
    camera::{visibility::RenderLayers, RenderTarget},
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    image::ImageSampler,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
    window::WindowResized,
};

use crate::camera::{FreeCam, OuterCamera, PIXEL_HEIGHT, PIXEL_WIDTH, camera_control};

mod camera;


// Layer 0: the 3D scene and its camera (renders into the low-res texture).
const PIXEL_LAYER: RenderLayers = RenderLayers::layer(0);
// Layer 1: the pixel canvas sprite and the outer 2D camera (renders to screen).
const SCREEN_LAYER: RenderLayers = RenderLayers::layer(1);


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
        .add_systems(Startup, setup)
        .add_systems(Update, (camera_control, update_fps, fit_canvas))
        .run();
}

/// Reads the smoothed FPS diagnostic and writes it into the FPS text entity.
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

/// Keeps the pixel canvas filling the window by adjusting the outer camera's
/// orthographic scale. Uses the smallest integer multiple that fits both axes,
/// so pixels are always upscaled by a whole number (1x, 2x, 3x …) and never
/// blurry from a fractional stretch.
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

/// One-shot startup system. Builds the pixel render target, spawns the scene,
/// and sets up both cameras.
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window>,
) {
    // The 3D scene renders into this small texture; the outer camera then
    // upscales it to fill the screen, giving the retro pixelated look.
    let canvas_size = Extent3d {
        width: PIXEL_WIDTH,
        height: PIXEL_HEIGHT,
        ..default()
    };
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
    // Nearest-neighbor sampling preserves hard pixel edges on upscale
    canvas.sampler = ImageSampler::nearest();
    let pixel_target = images.add(canvas);

    // Everything here is on PIXEL_LAYER so the outer 2D camera ignores it.
    commands.spawn((
        Mesh3d(meshes.add(Circle::new(4.0))),
        MeshMaterial3d(materials.add(Color::WHITE)),
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        PIXEL_LAYER,
    ));
    commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/scene.gltf#Scene0")),
        Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::new(0.1, 0.1, 0.1)),
        PIXEL_LAYER,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.75, 1.75, 1.75))),
        MeshMaterial3d(materials.add(Color::linear_rgb(1.0, 1.0, 1.0))),
        Transform::from_xyz(0.0, 1.0, 0.0),
        PIXEL_LAYER,
    ));
    commands.spawn((
        PointLight { shadows_enabled: false, ..default() },
        Transform::from_xyz(4.0, 8.0, 4.0),
        PIXEL_LAYER,
    ));

    // 3D camera renders the scene into the low-res texture at order -1,
    // so it always runs before the outer camera that composites to screen.
    commands.spawn((
        Camera3d::default(),
        Camera { order: -1, ..default() },
        RenderTarget::Image(pixel_target.clone().into()),
        Msaa::Off,
        PIXEL_LAYER,
        Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        FreeCam { yaw: -0.27, pitch: -0.45 },
    ));

    // The canvas sprite lives on SCREEN_LAYER — only the outer 2D camera sees it.
    // Its size in world-space is PIXEL_WIDTH × PIXEL_HEIGHT; the camera projection
    // scale makes that fill the window.
    commands.spawn((
        Sprite::from_image(pixel_target),
        SCREEN_LAYER,
    ));

    // Set the initial scale so the first frame looks correct before any resize event fires.
    let initial_scale = windows.single().ok().map(|w| {
        let h = w.width() / PIXEL_WIDTH as f32;
        let v = w.height() / PIXEL_HEIGHT as f32;
        1.0 / h.min(v).round()
    }).unwrap_or(0.25);

    // Outer 2D camera — composites the canvas sprite to the screen
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

    // FPS counter (UI pipeline — unaffected by RenderLayers)
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
