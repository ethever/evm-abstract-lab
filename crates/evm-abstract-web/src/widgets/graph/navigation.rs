//! Graph camera input, separate from content and layout.

use super::Graph;
use egui::{Event, Key, Modifiers, MouseWheelUnit, RawInput, Response, TouchPhase, Ui, Vec2};

impl Graph {
    pub(crate) fn prepare_input(&mut self, input: &mut RawInput) {
        if std::mem::take(&mut self.cancel_wheel) {
            // Cancel through egui's public input protocol before processing any
            // fresh wheel events. Merely ignoring a smooth delta leaves its old
            // remainder queued, which would contaminate the next gesture.
            input.events.insert(
                0,
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: Vec2::ZERO,
                    phase: TouchPhase::Cancel,
                    modifiers: Modifiers::NONE,
                },
            );
        }
    }

    pub(super) fn request_fit(&mut self, ui: &Ui, cancel_wheel: bool) {
        self.queue_fit();
        // A shortcut also works while the pointer is over a neighboring code
        // pane. Do not cancel that pane's independent scrolling in that case.
        self.cancel_wheel |= cancel_wheel;
        ui.ctx().request_repaint();
    }

    pub(super) fn fit_shortcut(&mut self, ui: &Ui) -> bool {
        if !ui.is_enabled() || ui.ctx().text_edit_focused() || egui::Popup::is_any_open(ui.ctx()) {
            return false;
        }
        let pressed = ui.input_mut(|input| {
            let mut pressed = false;
            // Match each event's modifiers: egui's consume_key also accepts
            // Alt, which would intercept browser/menu shortcuts such as Alt+F.
            input.events.retain(|event| {
                let fits = matches!(event, Event::Key {
                    key: Key::F, pressed: true, modifiers, ..
                } if !modifiers.alt && !modifiers.ctrl && !modifiers.command && !modifiers.mac_cmd);
                pressed |= fits;
                !fits
            });
            pressed
        });
        if pressed {
            let pointer_in_graph = ui.rect_contains_pointer(ui.available_rect_before_wrap());
            self.request_fit(ui, pointer_in_graph);
        }
        pressed
    }

    pub(super) fn navigate(&mut self, ui: &mut Ui, response: &Response) {
        if !ui.is_enabled() || egui::Popup::is_any_open(ui.ctx()) {
            return;
        }
        let canvas = response.rect;
        if response.dragged() {
            self.fitted = false;
            self.pan += response.drag_delta();
        }
        // Nodes are interactive children of the canvas: a gesture over one
        // must still reach the camera even when the background isn't hovered.
        // An explicit fit ends the previous camera gesture. Its remaining
        // smoothed deltas must not leave Fit without fresh navigation input.
        if response.contains_pointer()
            && (!self.fitted
                || ui.input(|input| {
                    input.events.iter().any(|event| {
                        matches!(
                            event,
                            Event::MouseWheel { .. } | Event::Zoom(_) | Event::Touch { .. }
                        )
                    })
                }))
        {
            let (scroll, zoom, pointer) = ui.input(|input| {
                (
                    input.smooth_scroll_delta(),
                    input.zoom_delta(),
                    input.pointer.hover_pos(),
                )
            });
            if let Some(pointer) = pointer {
                // eframe maps browser pinch gestures to Zoom; egui also maps
                // modified wheel events to zoom_delta and removes their pan.
                self.zoom_at(pointer - canvas.min, zoom);
                if scroll.is_finite() && scroll != Vec2::ZERO {
                    self.fitted = false;
                    self.pan += scroll;
                }
            }
        }
        if response.double_clicked() {
            self.request_fit(ui, true);
        }
    }
}

#[cfg(test)]
mod tests;
