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
}
