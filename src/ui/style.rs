//! Global egui style — "compact dark glass" theme applied once at startup.
//!
//! Palette:
//!   bg deep:   #0d1117   navy-black panel fill
//!   bg mid:    #161d2e   slightly lighter window fill
//!   bg widget: #1e2a42   widget background (sliders, checkboxes)
//!   accent:    #3b82f6   blue (active / selected)
//!   accent dim:#1d4ed8   blue hover
//!   border:    #2d4a7a   subtle blue-grey stroke
//!   text:      #d1d9e6   cool white text
//!   text dim:  #8b9ab5   muted label colour

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

// ── Palette ─────────────────────────────────────────────────────────────────
const BG_DEEP:    egui::Color32 = egui::Color32::from_rgb(13,  17,  23);
const BG_MID:     egui::Color32 = egui::Color32::from_rgb(20,  27,  43);
const BG_WIDGET:  egui::Color32 = egui::Color32::from_rgb(28,  40,  64);
const BG_HOVER:   egui::Color32 = egui::Color32::from_rgb(35,  52,  84);
const BG_ACTIVE:  egui::Color32 = egui::Color32::from_rgb(59, 130, 246);
const BORDER:     egui::Color32 = egui::Color32::from_rgb(45,  74, 122);
const BORDER_HOV: egui::Color32 = egui::Color32::from_rgb(80, 130, 210);
const TEXT:       egui::Color32 = egui::Color32::from_rgb(209, 217, 230);
const TEXT_DIM:   egui::Color32 = egui::Color32::from_rgb(139, 154, 181);
const SHADOW_COL: egui::Color32 = egui::Color32::from_rgba_premultiplied(0, 5, 15, 180);

const ROUND: u8 = 4;
const ROUND_SM: u8 = 2;

// ── System ───────────────────────────────────────────────────────────────────

/// Runs every frame in `EguiPrimaryContextPass` (before windows draw) and
/// applies the style. Running every frame rather than once at Startup is
/// necessary because bevy_egui resets the context between frames.
pub fn apply_style(mut contexts: EguiContexts) {
    let Ok(ctx) = contexts.ctx_mut() else { return };
    ctx.set_style(build_style());
}

fn build_style() -> egui::Style {
    let mut style = egui::Style::default();

    // ── Spacing ──────────────────────────────────────────────────────────────
    style.spacing.item_spacing        = egui::vec2(6.0, 4.0);
    style.spacing.window_margin       = egui::Margin::same(10);
    style.spacing.button_padding      = egui::vec2(8.0, 4.0);
    style.spacing.indent              = 14.0;
    style.spacing.scroll.bar_width    = 6.0;

    // ── Visuals ───────────────────────────────────────────────────────────────
    let mut v = egui::Visuals::dark();

    v.window_fill        = BG_MID;
    v.panel_fill         = BG_DEEP;
    v.faint_bg_color     = BG_WIDGET;
    v.extreme_bg_color   = BG_DEEP;
    v.window_stroke      = egui::Stroke::new(1.0, BORDER);
    v.window_shadow      = egui::Shadow {
        offset: [0, 4],
        blur:   12,
        spread: 2,
        color:  SHADOW_COL,
    };

    // Noninteractive (labels, separators, disabled)
    v.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill:       BG_MID,
        weak_bg_fill:  egui::Color32::TRANSPARENT,
        bg_stroke:     egui::Stroke::new(1.0, BORDER),
        corner_radius: ROUND.into(),
        fg_stroke:     egui::Stroke::new(1.0, TEXT_DIM),
        expansion:     0.0,
    };

    // Inactive (buttons, sliders at rest)
    v.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill:       BG_WIDGET,
        weak_bg_fill:  egui::Color32::TRANSPARENT,
        bg_stroke:     egui::Stroke::new(1.0, BORDER),
        corner_radius: ROUND.into(),
        fg_stroke:     egui::Stroke::new(1.0, TEXT),
        expansion:     0.0,
    };

    // Hovered
    v.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill:       BG_HOVER,
        weak_bg_fill:  BG_HOVER,
        bg_stroke:     egui::Stroke::new(1.0, BORDER_HOV),
        corner_radius: ROUND.into(),
        fg_stroke:     egui::Stroke::new(1.5, TEXT),
        expansion:     1.0,
    };

    // Active (held / clicking)
    v.widgets.active = egui::style::WidgetVisuals {
        bg_fill:       BG_ACTIVE,
        weak_bg_fill:  BG_ACTIVE,
        bg_stroke:     egui::Stroke::new(1.0, BORDER_HOV),
        corner_radius: ROUND.into(),
        fg_stroke:     egui::Stroke::new(1.5, egui::Color32::WHITE),
        expansion:     1.0,
    };

    // Open (combo-boxes, collapsing headers when expanded)
    v.widgets.open = egui::style::WidgetVisuals {
        bg_fill:       BG_HOVER,
        weak_bg_fill:  BG_HOVER,
        bg_stroke:     egui::Stroke::new(1.0, BORDER_HOV),
        corner_radius: ROUND_SM.into(),
        fg_stroke:     egui::Stroke::new(1.0, TEXT),
        expansion:     0.0,
    };

    // Selection (selectable_value highlight)
    v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(59, 130, 246, 80);
    v.selection.stroke  = egui::Stroke::new(1.0, BG_ACTIVE);

    v.override_text_color = Some(TEXT);

    // Slider grab and text cursor use accent blue
    v.widgets.inactive.bg_fill = BG_WIDGET;

    style.visuals = v;
    style
}

pub struct StylePlugin;

impl Plugin for StylePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, apply_style.before(crate::ui::UiSet));
    }
}
