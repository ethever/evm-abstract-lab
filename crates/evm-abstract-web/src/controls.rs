//! Shared workbench controls. Actions activate, toggles invert a boolean, and
//! tabs choose an alternative without deselecting the current one. egui owns
//! pointer/keyboard interaction; tile tabs retain the IDs supplied by the tree.

use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Response, Sense, Stroke, StrokeKind, Style,
    TextStyle, TextWrapMode, Ui, Vec2, WidgetInfo, WidgetText, WidgetType, vec2,
    widget_style::{Classes, WidgetState},
};

use crate::{notation, palette};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Density {
    Regular,
    Compact,
    Inline,
}

impl Density {
    pub(crate) fn height(self) -> f32 {
        match self {
            Self::Regular => 20.0,
            Self::Compact => 18.0,
            Self::Inline => 15.0,
        }
    }

    fn font(self) -> FontId {
        FontId::proportional(13.0)
    }

    fn padding(self) -> Vec2 {
        match self {
            Self::Regular => vec2(6.0, 2.0),
            Self::Compact => vec2(6.0, 1.0),
            Self::Inline => vec2(6.0, 0.0),
        }
    }

    fn apply(self, style: &mut Style) {
        style.text_styles.insert(TextStyle::Button, self.font());
        style.spacing.button_padding = self.padding();
        style.spacing.interact_size.y = self.height();
    }

    /// egui also uses interact_size for row height and native popup triggers.
    /// Apply density to the row so compact controls do not reserve a taller
    /// invisible band, and native controls keep the same font and padding.
    pub(crate) fn scope<R>(self, ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
        let original = Arc::clone(ui.style());
        self.apply(ui.style_mut());
        let result = contents(ui);
        ui.set_style(original);
        result
    }
}

const FOCUS_STROKE: Stroke = Stroke {
    width: 2.0,
    color: palette::BLUE,
};

/// Preserve the native baseline; compact and inline rows opt into tighter
/// padding without reducing the text size of their controls or neighbors.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn configure(style: &mut Style) {
    Density::Regular.apply(style);
}

pub(crate) fn action(ui: &mut Ui, label: impl Into<WidgetText>, density: Density) -> Response {
    show(ui, None, Kind::Action, label.into(), density)
}

pub(crate) fn action_enabled(
    ui: &mut Ui,
    enabled: bool,
    label: impl Into<WidgetText>,
    density: Density,
) -> Response {
    ui.add_enabled_ui(enabled, |ui| action(ui, label, density))
        .inner
}

pub(crate) fn menu_button(
    ui: &mut Ui,
    label: impl Into<WidgetText>,
    density: Density,
    contents: impl FnOnce(&mut Ui),
) -> Response {
    let response = show(ui, None, Kind::Menu, label.into(), density);
    let _ = egui::Popup::menu(&response).show(contents);
    response
}

pub(crate) fn toggle(
    ui: &mut Ui,
    value: &mut bool,
    label: impl Into<WidgetText>,
    density: Density,
) -> Response {
    let mut response = show(ui, None, Kind::Toggle(*value), label.into(), density);
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
}

pub(crate) fn tab(
    ui: &mut Ui,
    selected: bool,
    label: impl Into<WidgetText>,
    density: Density,
) -> Response {
    show(ui, None, Kind::Tab(selected), label.into(), density)
}

pub(crate) fn tab_value<T: PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    alternative: T,
    label: impl Into<WidgetText>,
    density: Density,
) -> Response {
    let mut response = tab(ui, *current == alternative, label, density);
    if response.clicked() && *current != alternative {
        *current = alternative;
        response.mark_changed();
    }
    response
}

/// The tree reads this response to select and lay out its actual active tile.
/// These fixed workspace tabs have neither close buttons nor drag handles.
pub(crate) fn tile_tab(
    ui: &mut Ui,
    id: Id,
    selected: bool,
    label: WidgetText,
    density: Density,
) -> Response {
    show(ui, Some(id), Kind::Tab(selected), label, density)
}

#[derive(Clone, Copy)]
enum Kind {
    Action,
    Menu,
    Toggle(bool),
    Tab(bool),
}

impl Kind {
    fn selected(self) -> bool {
        matches!(self, Self::Toggle(true) | Self::Tab(true))
    }

    fn info(self, enabled: bool, label: &str) -> WidgetInfo {
        match self {
            Self::Action | Self::Menu => WidgetInfo::labeled(WidgetType::Button, enabled, label),
            Self::Toggle(selected) => {
                WidgetInfo::selected(WidgetType::Button, enabled, selected, label)
            }
            Self::Tab(selected) => {
                WidgetInfo::selected(WidgetType::RadioButton, enabled, selected, label)
            }
        }
    }
}

struct Appearance {
    fill: Color32,
    border: Stroke,
    text: Color32,
    corner_radius: CornerRadius,
}

fn appearance(ui: &Ui, response: &Response, kind: Kind) -> Appearance {
    let state = response.widget_state();
    let native = ui.style().button_style(&Classes::default(), state);
    let framed =
        !matches!(kind, Kind::Toggle(false) | Kind::Tab(false)) || state != WidgetState::Inactive;
    Appearance {
        // Selection changes only the fill. Native text and interaction strokes
        // stay independent of the separate keyboard focus ring.
        fill: if kind.selected() {
            ui.visuals().selection.bg_fill
        } else if framed {
            native.frame.fill
        } else {
            Color32::TRANSPARENT
        },
        border: if framed {
            native.frame.stroke
        } else {
            Stroke::NONE
        },
        text: native.text_style.color,
        corner_radius: native.frame.corner_radius,
    }
}

fn show(ui: &mut Ui, id: Option<Id>, kind: Kind, label: WidgetText, density: Density) -> Response {
    let font = density.font();
    let label = notation::widget(
        ui,
        label
            .text_style(TextStyle::Button)
            .size(font.size)
            .color(Color32::PLACEHOLDER),
    );
    let wrap = ui.style().wrap_mode.unwrap_or(TextWrapMode::Extend);
    let width = (ui.available_width() - 2.0 * density.padding().x).max(0.0);
    let galley = label.into_galley(ui, Some(wrap), width, font);
    let desired = (galley.size() + 2.0 * density.padding()).max(vec2(0.0, density.height()));
    let (auto_id, rect) = ui.allocate_space(desired);
    let response = ui.interact(rect, id.unwrap_or(auto_id), Sense::click());
    response.widget_info(|| kind.info(ui.is_enabled(), galley.text()));
    if ui.is_rect_visible(rect) {
        let appearance = appearance(ui, &response, kind);
        ui.painter().rect(
            rect,
            appearance.corner_radius,
            appearance.fill,
            appearance.border,
            StrokeKind::Inside,
        );
        if response.has_focus() && response.enabled() {
            // Keep focus independent of selection, inside the allocated bounds
            // so it remains visible even when the tab bar clips its edges.
            ui.painter().rect_stroke(
                rect.shrink(2.0),
                appearance.corner_radius,
                FOCUS_STROKE,
                StrokeKind::Inside,
            );
        }
        let at = Align2::CENTER_CENTER
            .align_size_within_rect(galley.size(), rect)
            .min;
        ui.painter().galley(at, galley, appearance.text);
    }
    response
}
