use bevy::{asset::AssetMetaCheck, prelude::*};

const MOVE_SPEED: f32 = 5.0;
const LOOK_SPEED: f32 = 1.5;

#[derive(Component)]
struct FreeCam {
    yaw: f32,
    pitch: f32,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            meta_check: AssetMetaCheck::Never,
            ..default()
        }))
        .add_systems(Startup, setup)
        .add_systems(Update, camera_control)
        .run();
}

fn camera_control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &mut FreeCam)>,
) {
    let Ok((mut transform, mut cam)) = query.single_mut() else {
        return;
    };

    let dt = time.delta_secs();

    // Arrow keys: pan (yaw left/right, pitch up/down)
    if keys.pressed(KeyCode::ArrowLeft) {
        cam.yaw += LOOK_SPEED * dt;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        cam.yaw -= LOOK_SPEED * dt;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        cam.pitch = (cam.pitch + LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }
    if keys.pressed(KeyCode::ArrowDown) {
        cam.pitch = (cam.pitch - LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }

    let rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
    transform.rotation = rotation;

    // WASD + EQ: move relative to camera orientation
    let forward = transform.forward();
    let right = transform.right();
    let up = Vec3::Y;

    if keys.pressed(KeyCode::KeyW) {
        transform.translation += forward * MOVE_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyS) {
        transform.translation -= forward * MOVE_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyA) {
        transform.translation -= right * MOVE_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyD) {
        transform.translation += right * MOVE_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyE) {
        transform.translation += up * MOVE_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyQ) {
        transform.translation -= up * MOVE_SPEED * dt;
    }
}

/// set up a simple 3D scene
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    // circular base
    commands.spawn((
        Mesh3d(meshes.add(Circle::new(4.0))),
        MeshMaterial3d(materials.add(Color::WHITE)),
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
    ));
    // airplane
    commands.spawn((
        SceneRoot(asset_server.load("low-poly-airplane/scene.gltf#Scene0")),
        Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::new(0.1, 0.1, 0.1)),
    ));

    // spawn 1 meter cube for reference
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(Color::linear_rgb(1.0, 1.0, 1.0))),
        Transform::from_xyz(0.0, 0.5, 0.0),
    ));
    
    // light
    commands.spawn((
        PointLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0),
    ));
    // camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        FreeCam { yaw: -0.27, pitch: -0.45 },
    ));
}
