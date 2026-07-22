use bevy::{camera::visibility::RenderLayers, input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel}, prelude::*, render::render_resource::Extent3d, window::WindowResized};
use bevy_egui::{EguiContextSettings, EguiContexts};

use crate::plane::Airplane;

/// Base pixel-art canvas resolution ("Low" — the original, only resolution
/// before render scale became configurable). "High" renders at exactly 2x
/// this in each dimension; see `RenderScale`.
pub const PIXEL_WIDTH: u32 = 720;
pub const PIXEL_HEIGHT: u32 = 405;

pub const PIXEL_LAYER: RenderLayers = RenderLayers::layer(0);
pub const SCREEN_LAYER: RenderLayers = RenderLayers::layer(1);

/// User-facing render resolution, set from the Graphics menu. `PIXEL_WIDTH`/
/// `PIXEL_HEIGHT` were fixed consts everywhere they were read (five call
/// sites across camera.rs/main.rs/waypoints.rs/player_labels.rs); this
/// resource replaces that, with `current_extent()` as the one place that
/// derives actual pixel dimensions so every caller stays in sync
/// automatically when the setting changes.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderScale {
    #[default]
    Low,
    Medium,
}

impl RenderScale {
    pub fn multiplier(self) -> f32 {
        match self {
            RenderScale::Low => 1.0,
            RenderScale::Medium => 1.5,
        }
    }

    pub fn width(self) -> u32 {
        (PIXEL_WIDTH as f32 * self.multiplier()) as u32
    }

    pub fn height(self) -> u32 {
        (PIXEL_HEIGHT as f32 * self.multiplier()) as u32
    }

    pub fn extent(self) -> Extent3d {
        Extent3d { width: self.width(), height: self.height(), ..default() }
    }
}

/// Handle to the pixel-art render target texture, stashed at startup so
/// `apply_render_scale` can find and resize it later — `setup` creates it
/// once via `Assets<Image>::add`, which is the only place a fresh handle is
/// normally available.
#[derive(Resource)]
pub struct PixelCanvas(pub Handle<Image>);

/// Resizes the actual GPU render-target texture when `RenderScale` changes
/// (a plain resource change alone doesn't touch the texture — `Image::resize`
/// has to be called explicitly). The 3D camera renders into this texture at
/// whatever size it currently is, so this is the one place that needs to run
/// for a resolution change to take visual effect; every other consumer of
/// the old PIXEL_WIDTH/HEIGHT consts just needs to read the current
/// `RenderScale` dimensions instead.
pub fn apply_render_scale(
    scale: Res<RenderScale>,
    canvas: Res<PixelCanvas>,
    mut images: ResMut<Assets<Image>>,
) {
    if !scale.is_changed() {
        return;
    }
    if let Some(image) = images.get_mut(&canvas.0) {
        image.resize(scale.extent());
    }
}

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

/// User-facing multiplier on egui's rendering scale, exposed as a slider in
/// the Camera menu. `1.0` is bevy_egui's own default (roughly "native
/// resolution", independent of the 3D canvas's own upscale factor).
///
/// An earlier version of this tried to lock `EguiContextSettings.scale_factor`
/// to the *same* multiplier driving the 3D canvas's upscale
/// (`h_scale.min(v_scale)`, often 2-8x depending on window size). That's a
/// different kind of number: bevy_egui's `scale_factor` is a DPI-style
/// logical-to-physical-pixel ratio meant to stay near 1.0, not a "grow with
/// the window" factor — multiplying it by the canvas upscale made the UI
/// balloon far past readable size on any window bigger than the raw 640x360
/// canvas, and made this slider's 0.5-2.0 range negligible against that much
/// larger driving term (hence "doesn't seem to listen"). Keeping this
/// resource as the *only* input to `scale_factor` fixes both.
#[derive(Resource)]
pub struct UiScale(pub f32);

impl Default for UiScale {
    fn default() -> Self {
        Self(1.15)
    }
}

/// Scales the pixel-art 3D canvas to fill the window (unrelated to egui,
/// which bevy_egui already renders at native window resolution by default —
/// see `UiScale` for the separate, simple multiplier that actually adjusts
/// egui's size).
pub fn fit_canvas(
    mut events: MessageReader<WindowResized>,
    scale: Res<RenderScale>,
    window: Single<&Window>,
    mut outer: Single<(&mut Projection, &mut Camera), With<OuterCamera>>,
) {
    // Recompute on resize, and also when RenderScale changes: the canvas's
    // native size just changed, so the same window size now maps to a
    // different upscale factor even with no resize event of its own.
    let resized = !events.is_empty();
    events.clear();
    if !resized && !scale.is_changed() {
        return;
    }

    let (projection, camera) = &mut *outer;
    let Projection::Orthographic(projection) = &mut **projection else { return };
    let h_scale = window.width() / scale.width() as f32;
    let v_scale = window.height() / scale.height() as f32;
    let canvas_upscale = h_scale.min(v_scale);
    projection.scale = 1.0 / canvas_upscale;

    // Constrain this camera's viewport to exactly the letterboxed/pillarboxed
    // rect the pixel-art canvas actually occupies on screen — without this,
    // egui (pinned to this camera via PrimaryEguiContext) renders across the
    // *entire* window regardless of aspect ratio, so its windows could drift
    // into or past the grey bars whenever the window/fullscreen aspect ratio
    // didn't match the canvas's own.
    let canvas_w = scale.width() as f32 * canvas_upscale;
    let canvas_h = scale.height() as f32 * canvas_upscale;
    let offset_x = ((window.width() - canvas_w) * 0.5).max(0.0);
    let offset_y = ((window.height() - canvas_h) * 0.5).max(0.0);
    let scale_factor = window.scale_factor();
    camera.viewport = Some(bevy::camera::Viewport {
        physical_position: UVec2::new(
            (offset_x * scale_factor) as u32,
            (offset_y * scale_factor) as u32,
        ),
        physical_size: UVec2::new(
            (canvas_w * scale_factor).max(1.0) as u32,
            (canvas_h * scale_factor).max(1.0) as u32,
        ),
        depth: 0.0..1.0,
    });
}

/// Applies `UiScale` to egui whenever the slider moves.
pub fn apply_ui_scale(
    ui_scale: Res<UiScale>,
    mut settings: Single<&mut EguiContextSettings, With<OuterCamera>>,
) {
    if !ui_scale.is_changed() {
        return;
    }
    settings.scale_factor = ui_scale.0;
}

/// F11 or J toggles real browser fullscreen on the canvas element. This
/// leaves the canvas's backing resolution untouched — `fit_canvas` (driven
/// by the resulting `WindowResized` event) handles rescaling the pixel-art
/// output to whatever size the fullscreen canvas ends up being.
pub fn toggle_fullscreen_hotkey(keys: Res<ButtonInput<KeyCode>>) {
    if !keys.just_pressed(KeyCode::F11) && !keys.just_pressed(KeyCode::KeyJ) {
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
/// Pressing the same key again while already on that mount toggles back to
/// Orbit instead of re-snapping in place.
pub fn fixed_cam_hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    mounts: Res<FixedCameraMounts>,
    mut mode: ResMut<CameraMode>,
) {
    for (index, key) in FIXED_CAM_KEYS.iter().enumerate() {
        if keys.just_pressed(*key) && index < mounts.mounts.len() {
            *mode = if *mode == CameraMode::Fixed(index) {
                CameraMode::Orbit
            } else {
                CameraMode::Fixed(index)
            };
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
/// Free/Chase cam movement speed tiers: unmodified, one Shift, both Shifts,
/// and Caps Lock — the last a deliberate "faster than both Shifts" tier
/// (1.5x the both-Shifts speed) for covering long distances quickly without
/// needing to hold two keys down.
fn free_chase_move_speed(keys: &ButtonInput<KeyCode>) -> f32 {
    const BOTH_SHIFTS_SPEED: f32 = 2000.0;
    if keys.pressed(KeyCode::CapsLock) {
        return BOTH_SHIFTS_SPEED * 1.5;
    }
    match keys.pressed(KeyCode::ShiftLeft) {
        false => 5.0,
        true => match keys.pressed(KeyCode::ShiftRight) {
            false => 300.0,
            true => BOTH_SHIFTS_SPEED,
        },
    }
}

/// Click-and-drag look shared by Free and Chase cam: left-drag adds to
/// yaw/pitch the same way the arrow keys do, unless the drag started over an
/// egui panel (so UI clicks don't spin the camera).
const MOUSE_DRAG_SPEED: f32 = 0.005;

fn apply_mouse_look(
    mouse_buttons: &ButtonInput<MouseButton>,
    mouse_motion: &mut MessageReader<MouseMotion>,
    contexts: &mut EguiContexts,
    yaw: &mut f32,
    pitch: &mut f32,
) {
    let wants_pointer = contexts
        .ctx_mut()
        .map(|ctx| ctx.wants_pointer_input())
        .unwrap_or(false);
    if mouse_buttons.pressed(MouseButton::Left) && !wants_pointer {
        for ev in mouse_motion.read() {
            *yaw -= ev.delta.x * MOUSE_DRAG_SPEED;
            *pitch = (*pitch - ev.delta.y * MOUSE_DRAG_SPEED).clamp(-1.5, 1.5);
        }
    } else {
        mouse_motion.clear();
    }
}

pub fn free_cam_control(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut contexts: EguiContexts,
    mode: Res<CameraMode>,
    mut query: Query<(&mut Transform, &mut FreeCam)>,
) {
    if !matches!(*mode, CameraMode::Free) {
        mouse_motion.clear();
        return;
    }

    let move_speed: f32 = free_chase_move_speed(&keys);
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

    let cam = &mut *cam;
    apply_mouse_look(&mouse_buttons, &mut mouse_motion, &mut contexts, &mut cam.yaw, &mut cam.pitch);

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
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut contexts: EguiContexts,
    mode: Res<CameraMode>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&mut Transform, &mut ChaseCam), Without<Airplane>>,
) {
    if !matches!(*mode, CameraMode::Chase) {
        mouse_motion.clear();
        return;
    }

    let move_speed: f32 = free_chase_move_speed(&keys);
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

    let cam = &mut *cam;
    apply_mouse_look(&mouse_buttons, &mut mouse_motion, &mut contexts, &mut cam.yaw, &mut cam.pitch);

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
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut mouse_wheel: MessageReader<MouseWheel>,
    mut contexts: EguiContexts,
    mode: Res<CameraMode>,
    plane_query: Query<&Transform, With<Airplane>>,
    mut cam_query: Query<(&mut Transform, &mut TrackCam), Without<Airplane>>,
) {
    if !matches!(*mode, CameraMode::Orbit) {
        mouse_motion.clear();
        mouse_wheel.clear();
        return;
    }

    const LOOK_SPEED: f32 = 1.5;
    // Pixel-unit scroll (trackpads) reports much finer-grained deltas than
    // line-unit (mouse wheel notches), so scale each differently to land on
    // a similar felt zoom speed either way.
    const ZOOM_LINE_STEP: f32 = 4.0;
    const ZOOM_PIXEL_STEP: f32 = 0.15;

    let Ok(plane_tf) = plane_query.single() else { return };
    let Ok((mut cam_tf, mut track)) = cam_query.single_mut() else { return };

    let dt = time.delta_secs();

    match *mode {
        CameraMode::Orbit => {
            // [ / ] zoom the orbit radius.
            const ZOOM_SPEED: f32 = 100.0;
            if keys.pressed(KeyCode::BracketLeft)  { track.distance = (track.distance - ZOOM_SPEED * dt).clamp(3.0, 100.0); }
            if keys.pressed(KeyCode::BracketRight) { track.distance = (track.distance + ZOOM_SPEED * dt).clamp(3.0, 100.0); }

            // Scroll wheel also zooms, unless the scroll is over an egui panel.
            let wants_pointer_scroll = contexts
                .ctx_mut()
                .map(|ctx| ctx.wants_pointer_input())
                .unwrap_or(false);
            if !wants_pointer_scroll {
                for ev in mouse_wheel.read() {
                    let step = match ev.unit {
                        MouseScrollUnit::Line => ZOOM_LINE_STEP,
                        MouseScrollUnit::Pixel => ZOOM_PIXEL_STEP,
                    };
                    track.distance = (track.distance - ev.y * step).clamp(3.0, 100.0);
                }
            } else {
                mouse_wheel.clear();
            }

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

            // Click-and-drag also orbits, unless the drag started over an egui panel.
            let wants_pointer = contexts
                .ctx_mut()
                .map(|ctx| ctx.wants_pointer_input())
                .unwrap_or(false);
            if mouse_buttons.pressed(MouseButton::Left) && !wants_pointer {
                for ev in mouse_motion.read() {
                    track.yaw -= ev.delta.x * MOUSE_DRAG_SPEED;
                    track.pitch = (track.pitch - ev.delta.y * MOUSE_DRAG_SPEED).clamp(-1.4, 1.4);
                }
            } else {
                mouse_motion.clear();
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
