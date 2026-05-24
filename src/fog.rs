use bevy::prelude::*;

use crate::camera::FreeCam;

#[derive(Resource)]
pub struct FogEnabled(pub bool);

impl Default for FogEnabled {
    fn default() -> Self {
        Self(true)
    }
}

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FogEnabled>()
            .add_systems(PostStartup, setup_fog)
            .add_systems(Update, toggle_fog);
    }
}

fn make_fog() -> DistanceFog {
    DistanceFog {
        color: Color::srgba(0.55, 0.68, 0.82, 1.0),
        directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.5),
        directional_light_exponent: 30.0,
        falloff: FogFalloff::from_visibility_colors(
            4000.0,
            Color::srgb(0.35, 0.5, 0.66),
            Color::srgb(0.8, 0.844, 1.0),
        ),
    }
}

fn setup_fog(mut commands: Commands, cam_query: Query<Entity, With<FreeCam>>) {
    let Ok(entity) = cam_query.single() else { return };
    commands.entity(entity).insert(make_fog());
}

// Press 1 to insert/remove DistanceFog on the camera — reliable on/off for atmospheric falloff.
fn toggle_fog(
    keys: Res<ButtonInput<KeyCode>>,
    mut enabled: ResMut<FogEnabled>,
    mut commands: Commands,
    cam_query: Query<Entity, With<FreeCam>>,
) {
    if !keys.just_pressed(KeyCode::Digit1) {
        return;
    }
    enabled.0 = !enabled.0;
    let Ok(entity) = cam_query.single() else { return };
    if enabled.0 {
        commands.entity(entity).insert(make_fog());
    } else {
        commands.entity(entity).remove::<DistanceFog>();
    }
}
