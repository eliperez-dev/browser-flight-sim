use bevy::{camera::visibility::RenderLayers, prelude::*, window::WindowResized};

use crate::plane::Airplane;

// Resolution of pixel canvas.
// Default is 560 x 315
pub const PIXEL_WIDTH: u32 = 640;
pub const PIXEL_HEIGHT: u32 = 360;

pub const PIXEL_LAYER: RenderLayers = RenderLayers::layer(0);
pub const SCREEN_LAYER: RenderLayers = RenderLayers::layer(1);

#[derive(Resource, Default, PartialEq, Eq)]
pub enum CameraMode {
    Free,
    /// Free-look, but the camera's position tracks the plane instead of
    /// staying fixed in world space — like Free, WASD/EQ move the camera and
    /// arrow keys pan, except movement changes an offset from the plane
    /// rather than an absolute world position, so the camera rides along.
    Chase,
    #[default]
    Orbit,
    /// Rigidly mounted to a point on the plane; index into [`FixedCameraMounts`].
    Fixed(usize),
}

/// One rigid mount point on the aircraft: a translation offset (metres, in the
/// plane's local space) and a yaw/pitch to aim the camera once mounted there.
#[derive(Clone, Copy)]
pub struct FixedCameraMount {
    pub name: &'static str,
    pub offset: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// The default fixed-camera mounts, editable at runtime from the debug menu.
#[derive(Resource, Clone)]
pub struct FixedCameraMounts {
    pub mounts: Vec<FixedCameraMount>,
}

impl Default for FixedCameraMounts {
    fn default() -> Self {
        Self {
            // The aircraft's flight direction is local +Z (see plane.rs), but a
            // Bevy camera looks down its own local -Z by default, so a mount
            // facing forward (down the direction of travel) needs yaw = PI,
            // and a mount facing backward (toward the fuselage) needs yaw = 0.
            mounts: vec![
                // Nose: just ahead of the prop hub, looking forward.
                FixedCameraMount { name: "Nose", offset: Vec3::new(0.0, 1.0, 1.41), yaw: std::f32::consts::PI, pitch: 0.0 },
                // Tail: behind the rudder, looking forward over the aircraft.
                FixedCameraMount { name: "Tail", offset: Vec3::new(0.0, 1.8, -12.0), yaw: std::f32::consts::PI, pitch: -0.13 },
                // Left wingtip, looking inward at the fuselage.
                FixedCameraMount { name: "Left Wing", offset: Vec3::new(-6.8, 1.00, 0.5), yaw: -1.5708, pitch: -0.20 },
                // Right wingtip, looking inward at the fuselage.
                FixedCameraMount { name: "Right Wing", offset: Vec3::new(3.7, 0.8, -1.0), yaw: 2.5, pitch: -0.25 },
            ],
        }
    }
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

/// State for the chase camera: free-look angles plus a world-space offset
/// from the plane. WASD/EQ move the offset (not an absolute position) and
/// arrow keys pan, so the camera rides along with the plane while still
/// letting you freely reposition and look around like Free cam.
#[derive(Component)]
pub struct ChaseCam {
    pub yaw: f32,
    pub pitch: f32,
    pub offset: Vec3,
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

/// F11 toggles real browser fullscreen on the canvas element. This leaves the
/// canvas's backing resolution untouched — `fit_canvas` (driven by the
/// resulting `WindowResized` event) handles rescaling the pixel-art output to
/// whatever size the fullscreen canvas ends up being.
pub fn toggle_fullscreen_hotkey(keys: Res<ButtonInput<KeyCode>>) {
    if !keys.just_pressed(KeyCode::F11) {
        return;
    }
    request_toggle_fullscreen();
}

/// Enters/exits browser fullscreen on the canvas. Shared by the F11 hotkey and
/// the Camera menu's Fullscreen button.
pub fn request_toggle_fullscreen() {
    #[cfg(target_arch = "wasm32")]
    web_fullscreen::toggle();
}

/// Whether the canvas is currently in browser fullscreen, for the menu button's
/// pressed state. Always false on native builds.
pub fn is_fullscreen() -> bool {
    #[cfg(target_arch = "wasm32")]
    return web_fullscreen::is_fullscreen();
    #[cfg(not(target_arch = "wasm32"))]
    false
}

#[cfg(target_arch = "wasm32")]
mod web_fullscreen {
    use wasm_bindgen::JsCast;

    pub fn toggle() {
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };
        if document.fullscreen_element().is_some() {
            document.exit_fullscreen();
            return;
        }
        let Some(canvas) = document.query_selector("canvas").ok().flatten() else { return };
        let canvas: web_sys::Element = canvas.unchecked_into();
        let _ = canvas.request_fullscreen();
    }

    pub fn is_fullscreen() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.fullscreen_element().is_some())
    }
}

/// Digit keys 1-9/0 map to fixed-camera mount index (1 → mount 0, 2 → mount 1,
/// ... 9 → mount 8, 0 → mount 9), instantly snapping into `Fixed(index)` for
/// whichever mounts exist. Extra keys beyond the mount count are ignored.
const FIXED_CAM_KEYS: [KeyCode; 10] = [
    KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5,
    KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9, KeyCode::Digit0,
];

/// Snaps directly into a fixed-camera mount when its number key is pressed.
pub fn fixed_cam_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mounts: Res<FixedCameraMounts>,
    mut mode: ResMut<CameraMode>,
) {
    for (index, key) in FIXED_CAM_KEYS.iter().enumerate() {
        if keys.just_pressed(*key) && index < mounts.mounts.len() {
            *mode = CameraMode::Fixed(index);
        }
    }
}

/// Cycles Orbit → Chase → Free → Orbit with F. Each step seeds the next
/// mode's look angles (and Chase's offset) from wherever the camera currently
/// is, so switching in never snaps the view. Fixed cameras are only entered
/// from the Camera menu, not via this cycle.
pub fn toggle_camera_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CameraMode>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&Transform, &mut FreeCam, &mut ChaseCam, &mut TrackCam)>,
) {
    if !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    *mode = match *mode {
        CameraMode::Orbit | CameraMode::Fixed(_) => {
            if let (Ok((tf, _, mut chase, _)), Ok(plane_tf)) = (cam_query.single_mut(), plane_query.single()) {
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                chase.yaw = yaw;
                chase.pitch = pitch;
                chase.offset = tf.translation - plane_tf.translation;
            }
            CameraMode::Chase
        }
        CameraMode::Chase => {
            if let Ok((tf, mut free, _, _)) = cam_query.single_mut() {
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                free.yaw = yaw;
                free.pitch = pitch;
            }
            CameraMode::Free
        }
        CameraMode::Free => CameraMode::Orbit,
    };
}

/// Free-look camera — WASD/EQ move, arrow keys look.
/// Only active when mode is Free.
pub fn free_cam_control(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    mut query: Query<(&mut Transform, &mut FreeCam)>,
) {
    if !matches!(*mode, CameraMode::Free) {
        return;
    }

    let move_speed: f32 = match keys.pressed(KeyCode::ShiftLeft) {
        false => 5.0,
        true => match keys.pressed(KeyCode::ShiftRight) {
            false => 300.0,
            true => 2000.0,
        },
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

/// Chase camera — free-look, but WASD/EQ move an offset from the plane
/// instead of an absolute world position, so the camera rides along with the
/// aircraft while still panning and repositioning like Free cam.
/// Only active when mode is Chase.
pub fn chase_cam_control(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&mut Transform, &mut ChaseCam), Without<Airplane>>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        return;
    }

    let move_speed: f32 = match keys.pressed(KeyCode::ShiftLeft) {
        false => 5.0,
        true => match keys.pressed(KeyCode::ShiftRight) {
            false => 300.0,
            true => 2000.0,
        },
    };
    const LOOK_SPEED: f32 = 1.5;

    let Ok(plane_tf) = plane_query.single() else { return };
    let Ok((mut transform, mut cam)) = cam_query.single_mut() else { return };
    let dt = time.delta_secs();

    // Same look-angle accumulation as Free cam.
    if keys.pressed(KeyCode::ArrowLeft) { cam.yaw += LOOK_SPEED * dt; }
    if keys.pressed(KeyCode::ArrowRight) { cam.yaw -= LOOK_SPEED * dt; }
    if keys.pressed(KeyCode::ArrowUp) {
        cam.pitch = (cam.pitch + LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }
    if keys.pressed(KeyCode::ArrowDown) {
        cam.pitch = (cam.pitch - LOOK_SPEED * dt).clamp(-1.5, 1.5);
    }
    transform.rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);

    // WASD/EQ move the plane-relative offset along the camera's own axes,
    // same as Free cam — but it's the offset that accumulates, not world
    // position, so the camera tracks the plane every frame below.
    let forward = transform.forward();
    let right = transform.right();
    if keys.pressed(KeyCode::KeyW) { cam.offset += forward * move_speed * dt; }
    if keys.pressed(KeyCode::KeyS) { cam.offset -= forward * move_speed * dt; }
    if keys.pressed(KeyCode::KeyA) { cam.offset -= right * move_speed * dt; }
    if keys.pressed(KeyCode::KeyD) { cam.offset += right * move_speed * dt; }
    if keys.pressed(KeyCode::KeyE) { cam.offset += Vec3::Y * move_speed * dt; }
    if keys.pressed(KeyCode::KeyQ) { cam.offset -= Vec3::Y * move_speed * dt; }

    transform.translation = plane_tf.translation + cam.offset;
}

/// Orbit camera — only active when mode is Orbit.
pub fn track_cam_control(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&mut Transform, &mut TrackCam), Without<Airplane>>,
) {
    if !matches!(*mode, CameraMode::Orbit) {
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
        CameraMode::Free | CameraMode::Chase | CameraMode::Fixed(_) => {}
    }
}

/// Fixed camera — rigidly mounted to a point on the plane (nose, tail, wingtips,
/// ...). Only active when mode is `Fixed`. The mount offset is in metres,
/// world-aligned to the plane's own rotation (not the local ×0.1-scaled mesh
/// space used for wing/aileron attachment points).
pub fn fixed_cam_control(
    mode: Res<CameraMode>,
    mounts: Res<FixedCameraMounts>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<&mut Transform, (With<FreeCam>, Without<Airplane>)>,
) {
    let CameraMode::Fixed(index) = *mode else { return };
    let Some(mount) = mounts.mounts.get(index) else { return };
    let Ok(plane_tf) = plane_query.single() else { return };
    let Ok(mut cam_tf) = cam_query.single_mut() else { return };

    cam_tf.translation = plane_tf.translation + plane_tf.rotation * mount.offset;
    cam_tf.rotation = plane_tf.rotation
        * Quat::from_euler(EulerRot::YXZ, mount.yaw, mount.pitch, 0.0);
}
