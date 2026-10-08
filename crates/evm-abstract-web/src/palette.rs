//! A small semantic palette shared by the three custom-painted views.

use egui::Color32;
#[cfg(any(target_arch = "wasm32", test))]
use egui::{Context, Stroke, Visuals};

pub const BACKGROUND: Color32 = Color32::from_rgb(14, 19, 29);
pub const PANEL: Color32 = Color32::from_rgb(21, 29, 42);
pub const RAISED: Color32 = Color32::from_rgb(29, 40, 56);
pub const BORDER: Color32 = Color32::from_rgb(47, 62, 80);
pub const TEXT: Color32 = Color32::from_rgb(224, 233, 244);
pub const MUTED: Color32 = Color32::from_rgb(145, 164, 186);
pub const ACCENT: Color32 = Color32::from_rgb(99, 214, 202);
pub const BLUE: Color32 = Color32::from_rgb(127, 177, 255);
pub const PURPLE: Color32 = Color32::from_rgb(201, 165, 255);
pub const WARNING: Color32 = Color32::from_rgb(245, 193, 117);
pub const ERROR: Color32 = Color32::from_rgb(249, 130, 142);
pub const SELECTED: Color32 = Color32::from_rgb(33, 65, 75);

#[cfg(any(target_arch = "wasm32", test))]
pub fn configure(ctx: &Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BACKGROUND;
    visuals.faint_bg_color = RAISED;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.weak_bg_fill = RAISED;
    visuals.widgets.hovered.weak_bg_fill = SELECTED;
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(5.0, 3.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.menu_margin = egui::Margin::same(5);
        // Compact spacing, with the ordinary desktop text sizes preserved.
        style.spacing.window_margin = egui::Margin::same(6);
    });
}

/// Shared tight padding for navigation, input and status panels.
pub(crate) fn chrome_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BACKGROUND)
        .inner_margin(egui::Margin::symmetric(6, 2))
}
