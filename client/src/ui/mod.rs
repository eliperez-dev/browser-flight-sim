pub mod camera_menu;
pub mod instrument_panel;
pub mod menu_bar;
pub mod plane_menu;
pub mod style;
pub mod world_menu;

use bevy::prelude::*;

/// System set that all individual window-drawing systems belong to.
/// `draw_menu_bar` runs `.before(UiSet)` so button clicks are seen immediately.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiSet;

pub use camera_menu::CameraMenuPlugin;
pub use instrument_panel::InstrumentPanelPlugin;
pub use menu_bar::MenuBarPlugin;
pub use plane_menu::PlaneMenuPlugin;
pub use style::StylePlugin;
pub use world_menu::WorldMenuPlugin;
