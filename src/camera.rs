use bevy::{camera::visibility::RenderLayers, prelude::*, window::WindowResized};

use crate::plane::Airplane;

// Resolution of pixel canvas.
// Default is 560 x 315
pub const PIXEL_WIDTH: u32 = 592;
pub const PIXEL_HEIGHT: u32 = 333;

pub const PIXEL_LAYER: RenderLayers = RenderLayers::layer(0);
pub const SCREEN_LAYER: RenderLayers = RenderLayers::layer(1);

#[derive(Resource, Default, PartialEq, Eq)]
pub enum CameraMode {
    Free,
    #[default]
    Orbit,
}

/// State for the free-look camera (yaw/pitch accumulated from arrow keys).
#[derive(Component)]
pub struct FreeCam {
    pub yaw: f32,
    pub pitch: f32,
}

/// State for the tracking camera (orbits around the plane with arrow keys).
#[derive(Component)]
pub struct TrackCam {
    pub yaw: f32,
    pub pitch: f32,
    /// Orbit-mode radius around the plane.
    pub distance: f32,
}

/// Marker for the outer 2D camera that upscales the pixel canvas to the screen.
#[derive(Component)]
pub struct OuterCamera;

pub fn fit_canvas(
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

/// Cycles Free → Orbit → Chase → Free with F.
pub fn toggle_camera_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    mut cam_query: Query<(&Transform, &mut FreeCam, &mut TrackCam)>,
) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    *mode = match *mode {
        CameraMode::Free  => CameraMode::Orbit,
        CameraMode::Orbit => {
            if let Ok((tf, mut free, _)) = cam_query.single_mut() {
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                free.yaw = yaw;
                free.pitch = pitch;
            }
            CameraMode::Free
        }
    };
}

/// Free-look camera — WASD/EQ move, arrow keys look.
/// Only active when mode is Free.
pub fn free_cam_control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    mut query: Query<(&mut Transform, &mut FreeCam)>,
) {
    if !matches!(*mode, CameraMode::Free) {
        return;
    }

    let move_speed: f32 = match keys.pressed(KeyCode::ShiftLeft) {
        false => 5.0,
        true => 500.0
    };
    const LOOK_SPEED: f32 = 1.5;

    let Ok((mut transform, mut cam)) = query.single_mut() else { return };
    let dt = time.delta_secs();

    // Accumulate yaw (left/right) and pitch (up/down) from arrow keys.
    // Yaw is unbounded so you can spin freely; pitch is clamped just under
    // ±90° to prevent the camera from flipping upside-down at the poles.
    if keys.pressed(KeyCode::ArrowLeft) { cam.yaw += LOOK_SPEED * dt; }
    if keys.pressed(KeyCode::ArrowRight) { cam.yaw -= LOOK_SPEED * dt; }
    if keys.pressed(KeyCode::ArrowUp) {
        cam.pitch = (cam.pitch + LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }
    if keys.pressed(KeyCode::ArrowDown) {
        cam.pitch = (cam.pitch - LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }

    // Rebuild the rotation from the accumulated angles every frame using
    // YXZ order: yaw around world-Y first, then pitch around the camera's
    // local-X. This avoids gimbal lock and keeps roll permanently zero.
    transform.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);

    // Derive movement axes from the freshly-updated rotation so WASD always
    // moves relative to where the camera is currently pointing.
    let forward = transform.forward();
    let right = transform.right();

    // WASD flies the camera along its local forward/right axes (no altitude change).
    // E/Q move straight up or down in world space regardless of camera tilt,
    // so vertical movement is always predictable.
    if keys.pressed(KeyCode::KeyW) { transform.translation += forward * move_speed * dt; }
    if keys.pressed(KeyCode::KeyS) { transform.translation -= forward * move_speed * dt; }
    if keys.pressed(KeyCode::KeyA) { transform.translation -= right * move_speed * dt; }
    if keys.pressed(KeyCode::KeyD) { transform.translation += right * move_speed * dt; }
    if keys.pressed(KeyCode::KeyE) { transform.translation += Vec3::Y * move_speed * dt; }
    if keys.pressed(KeyCode::KeyQ) { transform.translation -= Vec3::Y * move_speed * dt; }
}

/// Orbit / Chase camera — only active when mode is Orbit or Chase.
pub fn track_cam_control(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&mut Transform, &mut TrackCam), Without<Airplane>>,
) {
    if matches!(*mode, CameraMode::Free) {
        return;
    }

    const LOOK_SPEED: f32 = 1.5;

    let Ok(plane_tf) = plane_query.single() else { return };
    let Ok((mut cam_tf, mut track)) = cam_query.single_mut() else { return };

    let dt = time.delta_secs();

    match *mode {
        CameraMode::Orbit => {
            // [ / ] zoom the orbit radius.
            const ZOOM_SPEED: f32 = 100.0;
            if keys.pressed(KeyCode::BracketLeft)  { track.distance = (track.distance - ZOOM_SPEED * dt).clamp(3.0, 100.0); }
            if keys.pressed(KeyCode::BracketRight) { track.distance = (track.distance + ZOOM_SPEED * dt).clamp(3.0, 100.0); }

            // Arrow keys orbit freely around the plane.
            if keys.pressed(KeyCode::ArrowLeft)  { track.yaw += LOOK_SPEED * dt; }
            if keys.pressed(KeyCode::ArrowRight) { track.yaw -= LOOK_SPEED * dt; }
            // Allow the orbit to swing well below the plane (negative pitch) so the
            // camera can sit low and look up past the aircraft at the sky above it.
            // Clamp stops just short of straight up/down to avoid the look_at
            // gimbal flip at the poles.
            if keys.pressed(KeyCode::ArrowUp) {
                track.pitch = (track.pitch + LOOK_SPEED * dt).clamp(-1.4, 1.4);
            }
            if keys.pressed(KeyCode::ArrowDown) {
                track.pitch = (track.pitch - LOOK_SPEED * dt).clamp(-1.4, 1.4);
            }

            let offset = Quat::from_euler(EulerRot::YXZ, track.yaw, -track.pitch, 0.0)
                * Vec3::new(0.0, 0.0, track.distance);
            cam_tf.translation = plane_tf.translation + offset;
            cam_tf.look_at(plane_tf.translation, Vec3::Y);
        }
        CameraMode::Free => {}
    }
}
