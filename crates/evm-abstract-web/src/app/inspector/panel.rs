//! Docked sizing preferences are independent of temporary viewport constraints
//! and of visibility. Maximizing never overwrites the user's docked height.

use egui::{Id, PanelState, Rect, Ui};

#[cfg(test)]
mod tests;

const DEFAULT_HEIGHT: f32 = 210.0;
const DEFAULT_VIEWPORT_FRACTION: f32 = 0.38;
const MIN_HEIGHT: f32 = 80.0;
const PANE_REMAINDER: f32 = 32.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Docked,
    Maximized,
}

pub(super) struct Panel {
    mode: Mode,
    // None retains the moderate automatic default, rather than treating the
    // initial size as a user request to fill a short viewport.
    preferred_height: Option<f32>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            mode: Mode::Docked,
            preferred_height: None,
        }
    }
}

impl Panel {
    pub(super) fn maximized(&self) -> bool {
        self.mode == Mode::Maximized
    }

    pub(super) fn toggle_maximized(&mut self) {
        self.mode = match self.mode {
            Mode::Docked => Mode::Maximized,
            Mode::Maximized => Mode::Docked,
        };
    }

    fn maximum_height(available: f32) -> f32 {
        // On short screens keep the title controls reachable; the other panes
        // yield their remainder before the inspector drops below its minimum.
        (available - PANE_REMAINDER)
            .max(MIN_HEIGHT)
            .min(available.max(0.0))
    }

    /// Seed egui's size from our preference every frame. egui persists a
    /// clamped size on viewport shrink, which must not replace that preference.
    pub(super) fn docked(&self, ui: &Ui, workspace_height: f32) -> egui::Panel {
        let available = ui.available_rect_before_wrap();
        let maximum = Self::maximum_height(available.height());
        let default_limit = workspace_height * DEFAULT_VIEWPORT_FRACTION;
        let height = self
            .preferred_height
            .unwrap_or_else(|| DEFAULT_HEIGHT.min(default_limit.max(MIN_HEIGHT)))
            .min(maximum);
        let id = Id::new("world_inspector");
        ui.ctx().data_mut(|data| {
            data.insert_persisted(
                id,
                PanelState {
                    outer_rect: Rect::from_min_max(
                        egui::pos2(available.left(), available.bottom() - height),
                        available.right_bottom(),
                    ),
                },
            );
        });
        let panel = egui::Panel::bottom(id)
            .resizable(true)
            .default_size(DEFAULT_HEIGHT)
            .frame(crate::palette::chrome_frame());
        if ui
            .read_response(id.with("__resize"))
            .is_some_and(|response| response.dragged() || response.drag_stopped())
        {
            panel.min_size(MIN_HEIGHT.min(maximum)).max_size(maximum)
        } else {
            // egui may grow a panel to fit minimum content sizes or rounded
            // frame bounds. Enforce the selected height when idle, while a
            // live resize handle gets the full result-workspace range.
            panel.exact_size(height)
        }
    }

    pub(super) fn remember_resize(&mut self, ui: &Ui, rect: Rect) {
        // Only a user drag changes the preference, including its release
        // frame. Content measurement and window resize are temporary clamps.
        if ui
            .read_response(Id::new("world_inspector").with("__resize"))
            .is_some_and(|response| response.dragged() || response.drag_stopped())
        {
            self.preferred_height = Some(rect.height().max(MIN_HEIGHT));
        }
    }
}
