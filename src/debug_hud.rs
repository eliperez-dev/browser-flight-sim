use bevy::prelude::*;

/// Shared debug overlay. Any system can push entries into `entries` each frame;
/// `render_debug_hud` turns the vec into the on-screen text.
/// Entries are cleared at the start of each populate pass, so the content
/// always reflects the current frame — no stale values accumulate.
#[derive(Resource, Default)]
pub struct DebugHud {
    /// Each entry is a (label, value) pair rendered as "LABEL: value".
    pub entries: Vec<(&'static str, String)>,
}

/// Marker for the single text entity that displays the debug overlay.
#[derive(Component)]
pub struct DebugHudText;

/// Reads the current entries in `DebugHud` and writes them to the overlay text.
/// Must run after whatever system populated `DebugHud` that frame.
pub fn render_debug_hud(
    hud: Res<DebugHud>,
    mut query: Query<&mut Text, With<DebugHudText>>,
) {
    let Ok(mut text) = query.single_mut() else { return };

    **text = hud.entries
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n");
}
