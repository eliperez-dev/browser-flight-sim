//! Top-of-screen menu bar — a single horizontal strip of toggle buttons, one
//! per in-game window. Clicking a button opens / closes that window; the button
//! is visually highlighted while the window is open. Windows also write back
//! here when the user closes them via the egui × button, so the bar stays in
//! sync automatically.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

/// The four states the Gizmos button cycles through on each click (or `G`
/// press): fully off; whole-aircraft vectors only (velocity, thrust, gravity,
/// CoM, aerodynamic center — no per-surface panels); wireframe outlines (the
/// original behaviour, vectors + per-surface panels/force/AoA); and filled
/// surfaces — which additionally hides the aircraft mesh so the solid
/// aero-surface panels are readable without the model occluding/cluttering them.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmosMode {
    #[default]
    Off,
    Vectors,
    Outline,
    Filled,
}

impl GizmosMode {
    pub fn next(self) -> Self {
        match self {
            GizmosMode::Off => GizmosMode::Vectors,
            GizmosMode::Vectors => GizmosMode::Outline,
            GizmosMode::Outline => GizmosMode::Filled,
            GizmosMode::Filled => GizmosMode::Off,
        }
    }

    fn label(self) -> &'static str {
        match self {
            GizmosMode::Off => "Gizmos",
            GizmosMode::Vectors => "Gizmos: Vectors",
            GizmosMode::Outline => "Gizmos: Outline",
            GizmosMode::Filled => "Gizmos: Filled",
        }
    }
}

/// Tracks which in-game windows are currently open. All systems that draw a
/// toggleable window read and write their own field here.
#[derive(Resource)]
pub struct MenuBar {
    pub flight_model: bool,
    pub map: bool,
    pub handbook: bool,
    pub world: bool,
    pub my_plane: bool,
    pub gizmos: GizmosMode,
    pub camera: bool,
    pub multiplayer: bool,
    pub graphics: bool,
}

impl Default for MenuBar {
    fn default() -> Self {
        Self {
            flight_model: false,
            map: false,
            // Handbook opens by default so new players see controls immediately.
            handbook: true,
            world: false,
            my_plane: false,
            gizmos: GizmosMode::Off,
            camera: false,
            multiplayer: false,
            graphics: false,
        }
    }
}

pub struct MenuBarPlugin;

impl Plugin for MenuBarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuBar>()
            .add_systems(EguiPrimaryContextPass, draw_menu_bar.before(crate::ui::UiSet));
    }
}

/// Draws the menu bar as a floating area centred at the top of the screen.
/// Using `Area` instead of `TopBottomPanel` means it only takes the space it
/// needs (no full-width black bar) and renders at `Foreground` order so it
/// always sits above in-world waypoint labels.
pub fn draw_menu_bar(
    mut bar: ResMut<MenuBar>,
    mut contexts: EguiContexts,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // Measure the screen width this frame so we can centre the area.
    let screen_w = ctx.content_rect().width();

    egui::Area::new(egui::Id::new("menu_bar"))
        .order(egui::Order::Tooltip)
        // Anchor top-centre; we'll offset left by half the measured width after
        // the first frame. On frame 0 it snaps; after that it stays centred.
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 4.0))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(13, 17, 23, 230))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 74, 122)))
                .corner_radius(egui::CornerRadius::from(5u8))
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_max_width(screen_w);
                    ui.horizontal(|ui| {
                        menu_button(ui, "Map", &mut bar.map);
                        menu_button(ui, "Handbook", &mut bar.handbook);
                        menu_button(ui, "Weather", &mut bar.world);
                        menu_button(ui, "My Plane", &mut bar.my_plane);
                        menu_button(ui, "Camera", &mut bar.camera);
                        menu_button(ui, "Graphics", &mut bar.graphics);
                        menu_button(ui, "Multiplayer", &mut bar.multiplayer);
                        ui.separator();
                        gizmos_button(ui, &mut bar.gizmos);
                        ui.separator();
                        menu_button(ui, "Dev Tools", &mut bar.flight_model);
                    });
                });
        });

    Ok(())
}

// Accent blue (matches style.rs BG_ACTIVE)
const ACCENT:     egui::Color32 = egui::Color32::from_rgb(59, 130, 246);
const ACCENT_BG:  egui::Color32 = egui::Color32::from_rgba_premultiplied(18, 40, 80, 220);
const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(139, 154, 181);

/// A single toggle button for the bar. Blue-lit when open, muted when closed.
fn menu_button(ui: &mut egui::Ui, label: &str, open: &mut bool) {
    let rich = egui::RichText::new(label)
        .size(13.0)
        .color(if *open { ACCENT } else { TEXT_MUTED });

    let btn = egui::Button::new(rich)
        .fill(if *open { ACCENT_BG } else { egui::Color32::TRANSPARENT })
        .stroke(if *open {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 90, 160))
        } else {
            egui::Stroke::NONE
        })
        .corner_radius(egui::CornerRadius::from(3u8));

    if ui.add(btn).clicked() {
        *open = !*open;
    }
}

/// The Gizmos button: cycles Off -> Outline -> Filled -> Off on click, same
/// blue-lit/muted styling as `menu_button` but lit for either non-Off state
/// and labelled with the current mode instead of a static caption.
fn gizmos_button(ui: &mut egui::Ui, mode: &mut GizmosMode) {
    let active = *mode != GizmosMode::Off;
    let rich = egui::RichText::new(mode.label())
        .size(13.0)
        .color(if active { ACCENT } else { TEXT_MUTED });

    let btn = egui::Button::new(rich)
        .fill(if active { ACCENT_BG } else { egui::Color32::TRANSPARENT })
        .stroke(if active {
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 90, 160))
        } else {
            egui::Stroke::NONE
        })
        .corner_radius(egui::CornerRadius::from(3u8));

    if ui.add(btn).clicked() {
        *mode = mode.next();
    }
}
