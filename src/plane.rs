use bevy::prelude::*;

#[derive(Component)]
pub struct Airplane;

const SPEED: f32 = 1.0;

pub fn move_airplane(time: Res<Time>, mut query: Query<&mut Transform, With<Airplane>>) {
    let Ok(mut transform) = query.single_mut() else { return };

    // In Bevy's right-handed coordinate system, an unrotated entity's local
    // forward axis is -Z. Subtracting that is the same as adding +Z, so the
    // plane flies in the +Z direction (toward the camera's starting position).
    // Once real flight physics rotate the transform, forward() will track the
    // actual nose direction automatically — no change needed here.
    let forward = transform.forward();
    transform.translation -= forward * SPEED * time.delta_secs();
}
