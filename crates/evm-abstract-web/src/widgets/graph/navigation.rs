//! Graph camera input, separate from content and layout.

use super::Graph;
use egui::{Event, Key, Response, Ui, Vec2};

impl Graph {
    pub(super) fn fit_shortcut(&mut self, ui: &Ui) {
        if ui.ctx().text_edit_focused() {
            return;
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
            self.restore_automatic();
        }
    }

    pub(super) fn navigate(&mut self, ui: &mut Ui, response: &Response) {
        let canvas = response.rect;
        if response.dragged() {
            self.automatic = false;
            self.pan += response.drag_delta();
        }
        // Nodes are interactive children of the canvas: a gesture over one
        // must still reach the camera even when the background isn't hovered.
        if response.contains_pointer() {
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
                    self.automatic = false;
                    self.pan += scroll;
                }
            }
        }
        if response.double_clicked() {
            self.restore_automatic();
        }
    }
}

#[cfg(test)]
mod tests;
