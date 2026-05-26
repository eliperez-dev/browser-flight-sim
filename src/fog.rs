use bevy::prelude::*;

use crate::camera::FreeCam;

/// Tunable atmospheric fog, editable live from the F3 debug menu. Colours are
/// stored as linear `[r, g, b]` arrays so egui's colour pickers can edit them
/// directly; `apply_fog` rebuilds the camera's [`DistanceFog`] whenever this
/// changes.
#[derive(Resource, Clone)]
pub struct FogSettings {
    pub enabled: bool,
    /// Distance (m) at which terrain is fully fogged out.
    pub visibility: f32,
    /// Base fog colour.
    pub color: [f32; 3],
    /// Falloff extinction (near) and inscattering (far) colours.
    pub extinction_color: [f32; 3],
    pub inscattering_color: [f32; 3],
    /// Tint and tightness of the glow around the sun direction.
    pub directional_light_color: [f32; 3],
    pub directional_light_exponent: f32,
}

impl Default for FogSettings {
    fn default() -> Self {
        // Matches the original hand-tuned look.
        Self {
            enabled: true,
            visibility: 35000.0,
            color: [0.55, 0.68, 0.82],
            extinction_color: [0.35, 0.5, 0.66],
            inscattering_color: [0.8, 0.844, 1.0],
            directional_light_color: [1.0, 0.95, 0.85],
            directional_light_exponent: 4.0,
        }
    }
}

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FogSettings>()
            .add_systems(Update, (toggle_fog, apply_fog));
    }
}

fn build_fog(s: &FogSettings) -> DistanceFog {
    let rgb = |c: [f32; 3]| Color::srgb(c[0], c[1], c[2]);
    DistanceFog {
        color: rgb(s.color),
        directional_light_color: Color::srgba(
            s.directional_light_color[0],
            s.directional_light_color[1],
            s.directional_light_color[2],
            0.5,
        ),
        directional_light_exponent: s.directional_light_exponent,
        falloff: FogFalloff::from_visibility_colors(
            s.visibility.max(1.0),
            rgb(s.extinction_color),
            rgb(s.inscattering_color),
        ),
    }
}

/// Keeps the camera's [`DistanceFog`] in sync with [`FogSettings`]: (re)builds it
/// when the settings change or the camera doesn't have it yet, and removes it
/// when fog is disabled. Runs every frame but only touches the camera when
/// something actually needs to change.
fn apply_fog(
    settings: Res<FogSettings>,
    mut commands: Commands,
    cam_query: Query<(Entity, Option<&DistanceFog>), With<FreeCam>>,
) {
    let Ok((entity, has_fog)) = cam_query.single() else { return };
    if settings.enabled {
        if settings.is_changed() || has_fog.is_none() {
            commands.entity(entity).insert(build_fog(&settings));
        }
    } else if has_fog.is_some() {
        commands.entity(entity).remove::<DistanceFog>();
    }
}

// Press 1 to toggle fog on/off; `apply_fog` reacts to the flipped setting.
fn toggle_fog(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<FogSettings>) {
    if keys.just_pressed(KeyCode::Digit1) {
        settings.enabled = !settings.enabled;
    }
}
