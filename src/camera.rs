use bevy::prelude::*;

/// Low-res render target dimensions. The 3D scene is rendered into a texture
/// this size, then upscaled to fill the screen, producing the pixelated look.
pub const PIXEL_WIDTH: u32 = 320;
pub const PIXEL_HEIGHT: u32 = 180;

/// Stores the accumulated yaw (horizontal) and pitch (vertical) angles for the
/// free-look camera.
#[derive(Component)]
pub struct FreeCam {
    pub yaw: f32,
    pub pitch: f32,
}

/// Marker for the outer 2D camera that upscales the pixel canvas to the screen.
/// Used by `fit_canvas` to find the right projection to adjust on resize.
#[derive(Component)]
pub struct OuterCamera;

/// Free-look camera controller.
///
/// Controls:
/// - Arrow keys: look (yaw left/right, pitch up/down)
/// - WASD: fly forward/back/strafe relative to camera facing
/// - E/Q: move up/down in world space
pub fn camera_control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &mut FreeCam)>,
) {
    const MOVE_SPEED: f32 = 5.0;
    const LOOK_SPEED: f32 = 1.5;

    let Ok((mut transform, mut cam)) = query.single_mut() else {
        return;
    };

    let dt = time.delta_secs();

    // Arrow keys rotate the camera
    if keys.pressed(KeyCode::ArrowLeft) {
        cam.yaw += LOOK_SPEED * dt;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        cam.yaw -= LOOK_SPEED * dt;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        // Clamp pitch to just under ±90° to avoid flipping
        cam.pitch = (cam.pitch + LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }
    if keys.pressed(KeyCode::ArrowDown) {
        cam.pitch = (cam.pitch - LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }

    // Rebuild rotation from stored angles each frame (YXZ = yaw then pitch, no roll)
    transform.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);

    let forward = transform.forward();
    let right = transform.right();

    // WASD moves relative to where the camera is pointing
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

    // E/Q moves straight up/down in world space regardless of camera tilt
    if keys.pressed(KeyCode::KeyE) {
        transform.translation += Vec3::Y * MOVE_SPEED * dt;
    }
    if keys.pressed(KeyCode::KeyQ) {
        transform.translation -= Vec3::Y * MOVE_SPEED * dt;
    }
}