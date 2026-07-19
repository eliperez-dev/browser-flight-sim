pub mod menu_bar;
pub mod style;

use bevy::prelude::*;

/// System set that all individual window-drawing systems belong to.
/// `draw_menu_bar` runs `.before(UiSet)` so button clicks are seen immediately.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiSet;

pub use menu_bar::MenuBarPlugin;
pub use style::StylePlugin;
