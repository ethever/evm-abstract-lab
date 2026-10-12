//! Shared workbench controls. Actions activate, toggles invert a boolean, and
//! tabs choose an alternative without deselecting the current one. egui owns
//! pointer/keyboard interaction; tile tabs retain the IDs supplied by the tree.

use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, FontId, Id, Response, Sense, Stroke, StrokeKind, Style,
    TextStyle, TextWrapMode, Ui, Vec2, WidgetInfo, WidgetText, WidgetType, vec2,
};

use crate::{notation, palette};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Density {
    Regular,
    Compact,
}

impl Density {
    pub(crate) fn height(self) -> f32 {
        match self {
            Self::Regular => 24.0,
            Self::Compact => 18.0,
        }
    }

    fn font(self) -> FontId {
        FontId::proportional(match self {
            Self::Regular => 14.0,
            Self::Compact => 12.0,
        })
    }

    fn padding(self) -> Vec2 {
        match self {
            Self::Regular => vec2(7.0, 3.0),
            Self::Compact => vec2(6.0, 2.0),
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

const CORNER_RADIUS: CornerRadius = CornerRadius::same(4);
const FOCUS_STROKE: Stroke = Stroke {
    width: 2.0,
    color: palette::BLUE,
};

/// Native controls inherit the workbench's compact baseline. Navigation and
/// form action rows opt into regular density without enlarging every panel.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn configure(style: &mut Style) {
    Density::Compact.apply(style);
    let visuals = &mut style.visuals;
    for (widget, fill, border) in [
        (
            &mut visuals.widgets.noninteractive,
            palette::PANEL,
            palette::BORDER,
        ),
        (
            &mut visuals.widgets.inactive,
            palette::RAISED,
            palette::BORDER,
        ),
        (
            &mut visuals.widgets.hovered,
            palette::CONTROL_HOVER,
            palette::MUTED,
        ),
        (
            &mut visuals.widgets.active,
            palette::CONTROL_PRESSED,
            palette::ACCENT,
        ),
        (
            &mut visuals.widgets.open,
            palette::SELECTED,
            palette::ACCENT,
        ),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0, border);
        widget.fg_stroke = Stroke::new(1.0, palette::TEXT);
        widget.corner_radius = CORNER_RADIUS;
        widget.expansion = 0.0;
    }
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
    let open = egui::Popup::is_id_open(ui.ctx(), ui.next_auto_id().with("popup"));
    let response = show(ui, None, Kind::Menu(open), label.into(), density);
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
    Menu(bool),
    Toggle(bool),
    Tab(bool),
}

impl Kind {
    fn selected(self) -> bool {
        matches!(
            self,
            Self::Menu(true) | Self::Toggle(true) | Self::Tab(true)
        )
    }

    fn info(self, enabled: bool, label: &str) -> WidgetInfo {
        match self {
            Self::Action | Self::Menu(_) => WidgetInfo::labeled(WidgetType::Button, enabled, label),
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
}

fn appearance(response: &Response, selected: bool) -> Appearance {
    if !response.enabled() {
        return Appearance {
            fill: palette::PANEL,
            border: Stroke::new(1.0, palette::BORDER),
            text: palette::MUTED,
        };
    }
    let pressed = response.is_pointer_button_down_on();
    let hovered = response.hovered() || response.highlighted();
    Appearance {
        fill: if pressed {
            palette::CONTROL_PRESSED
        } else if hovered {
            if selected {
                palette::SELECTED_HOVER
            } else {
                palette::CONTROL_HOVER
            }
        } else if selected {
            palette::SELECTED
        } else {
            palette::RAISED
        },
        border: Stroke::new(
            1.0,
            if selected || pressed {
                palette::ACCENT
            } else if hovered {
                palette::MUTED
            } else {
                palette::BORDER
            },
        ),
        text: if selected {
            palette::ACCENT
        } else {
            palette::TEXT
        },
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
        let appearance = appearance(&response, kind.selected());
        ui.painter().rect(
            rect,
            CORNER_RADIUS,
            appearance.fill,
            appearance.border,
            StrokeKind::Inside,
        );
        if response.has_focus() && response.enabled() {
            // Keep focus independent of selection, inside the allocated bounds
            // so it remains visible even when the tab bar clips its edges.
            ui.painter().rect_stroke(
                rect.shrink(2.0),
                CORNER_RADIUS,
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
