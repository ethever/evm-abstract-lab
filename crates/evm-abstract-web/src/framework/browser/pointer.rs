//! Observe the real device before eframe synchronously handles pointerdown.
//! Its first touchstart arrives later, after native labels choose drag senses.

use std::{cell::Cell, rc::Rc};

use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use web_sys::{HtmlCanvasElement, PointerEvent};

pub(super) struct PointerBridge {
    canvas: HtmlCanvasElement,
    pointerdown: Closure<dyn FnMut(PointerEvent)>,
    touch: Rc<Cell<bool>>,
    primary_start: Rc<Cell<bool>>,
    contact: super::super::contact::ContactStart,
}

impl PointerBridge {
    pub(super) fn install(canvas: HtmlCanvasElement) -> Result<Self, JsValue> {
        let touch = Rc::new(Cell::new(false));
        let current = Rc::clone(&touch);
        let primary_start = Rc::new(Cell::new(false));
        let started = Rc::clone(&primary_start);
        let pointerdown = Closure::new(move |event: PointerEvent| {
            current.set(event.pointer_type() == "touch");
            started.set(event.pointer_type() == "touch" && event.is_primary());
        });
        canvas.add_event_listener_with_callback_and_bool(
            "pointerdown",
            pointerdown.as_ref().unchecked_ref(),
            true,
        )?;
        Ok(Self {
            canvas,
            pointerdown,
            touch,
            primary_start,
            contact: Default::default(),
        })
    }

    pub(super) fn raw_input(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if self
            .contact
            .prepare(input, self.primary_start.replace(false))
        {
            ctx.request_repaint();
        }
    }

    pub(super) fn prepare(&self, ui: &mut egui::Ui) {
        if self.touch.get() {
            // Let the enclosing ScrollArea own the first real swipe. Native
            // labels otherwise infer mouse dragging before Touch metadata.
            // TextEdit and explicit copy actions keep their own interactions.
            ui.style_mut().interaction.selectable_labels = false;
        }
    }
}

impl Drop for PointerBridge {
    fn drop(&mut self) {
        let _ = self.canvas.remove_event_listener_with_callback_and_bool(
            "pointerdown",
            self.pointerdown.as_ref().unchecked_ref(),
            true,
        );
    }
}
