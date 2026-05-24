use bevy::prelude::*;

use crate::camera::CameraMode;
use crate::plane::{Airplane, PlaneState};

/// Velocity-tracks-nose arcade flight model.
/// Attach alongside [`PlaneState`] and [`Airplane`] to make an entity flyable.
#[derive(Component)]
pub struct SimplePlanePhysics {
    pub velocity: Vec3,
    pub throttle: f32,
}

impl Default for SimplePlanePhysics {
    fn default() -> Self {
        Self {
            velocity: Vec3::new(0.0, 0.0, 20.0), // +Z = visual nose direction
            throttle: 0.5,
        }
    }
}

const GRAVITY: f32       = 9.8;
const MAX_THRUST: f32    = 20.0;  // m/s² at full throttle
const DRAG_COEFF: f32    = 0.4;   // at cruise + 50% throttle, drag == thrust
const CRUISE_SPEED: f32  = 25.0;  // m/s; reference for lift and authority scaling

const VEL_TURN_RATE: f32 = 6.0;
const PITCH_RATE: f32    = 0.8;   // rad/s max, scales down with airspeed
const ROLL_RATE: f32     = 1.2;   // rad/s max, scales down with airspeed
const YAW_RATE: f32      = 0.5;   // rad/s max, scales down with airspeed
const THROTTLE_RATE: f32 = 0.5;   // fraction/s

/// Controls (Track mode only):
///   = / -      throttle up / down
///   W / S      pitch down / up
///   A / D      roll left / right
///   Q / E      yaw left / right
///   Arrow keys orbit the chase camera (handled in camera.rs)
pub fn simple_plane_physics(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CameraMode>,
    mut query: Query<(&mut Transform, &mut SimplePlanePhysics, &mut PlaneState), With<Airplane>>,
) {
    let Ok((mut transform, mut physics, mut state)) = query.single_mut() else { return };
    let dt = time.delta_secs();

    // Throttle works in both modes.
    if keys.pressed(KeyCode::Equal) {
        physics.throttle = (physics.throttle + THROTTLE_RATE * dt).min(1.0);
    }
    if keys.pressed(KeyCode::Minus) {
        physics.throttle = (physics.throttle - THROTTLE_RATE * dt).max(0.0);
    }

    // The plane model's nose faces +Z (Bevy's back = -forward).
    let nose: Vec3 = *(-transform.forward());
    let speed       = physics.velocity.length();
    let current_dir = if speed > 0.001 { physics.velocity / speed } else { nose };

    // Flight controls only active in Track mode so they don't fight the free camera's WASD.
    if matches!(*mode, CameraMode::Track) {
        // Authority scales with airspeed: sluggish at stall, full at cruise, capped there.
        let authority = (speed / CRUISE_SPEED).clamp(0.0, 1.0);
        if keys.pressed(KeyCode::KeyW) { transform.rotate_local_x(PITCH_RATE * authority * dt); }
        if keys.pressed(KeyCode::KeyS) { transform.rotate_local_x(- PITCH_RATE * authority * dt); }
        if keys.pressed(KeyCode::KeyA) { transform.rotate_local_z( ROLL_RATE  * authority * dt); }
        if keys.pressed(KeyCode::KeyD) { transform.rotate_local_z(-ROLL_RATE  * authority * dt); }
        if keys.pressed(KeyCode::KeyQ) { transform.rotate_local_y( YAW_RATE   * authority * dt); }
        if keys.pressed(KeyCode::KeyE) { transform.rotate_local_y(-YAW_RATE   * authority * dt); }
    }

    let thrust_accel = physics.throttle * MAX_THRUST;
    let drag_accel   = DRAG_COEFF * speed;
    physics.velocity += nose * thrust_accel * dt;
    physics.velocity -= current_dir * drag_accel * dt;

    // Velocity direction lerps toward the nose — this is the implicit lift.
    // Weak at low speed (stall → gravity wins), strong at cruise (plane goes where pointed).
    // Capped at cruise strength so overspeed doesn't compound agility.
    let speed2   = physics.velocity.length();
    let lift_pct = (speed2 / CRUISE_SPEED).clamp(0.0, 1.5);
    let lerp_t   = (VEL_TURN_RATE * lift_pct.min(1.0) * dt).min(1.0);
    let new_dir  = current_dir.lerp(nose, lerp_t).normalize_or_zero();
    if new_dir != Vec3::ZERO {
        physics.velocity = new_dir * speed2;
    }

    physics.velocity += Vec3::NEG_Y * GRAVITY * dt;
    transform.translation += physics.velocity * dt;

    // Ground clamp.
    if transform.translation.y < 0.5 {
        transform.translation.y = 0.5;
        physics.velocity.y = physics.velocity.y.max(0.0);
    }

    state.speed    = physics.velocity.length();
    state.thrust   = thrust_accel;
    state.drag     = drag_accel;
    state.lift_pct = lift_pct;
}
