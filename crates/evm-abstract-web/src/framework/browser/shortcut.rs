//! Synchronous ownership of one canvas shortcut. eframe 0.36.2 does not cancel
//! Ctrl+J's downloads action. Capturing it here cancels that action before DOM
//! dispatch returns; accepted presses are delivered once to the workspace.

use std::{cell::Cell, rc::Rc};

use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use web_sys::{Event, HtmlCanvasElement, KeyboardEvent};

use crate::app::{inspector_chord, inspector_shortcut_allowed};

/// The browser app owns these listeners and removes them before dropping their
/// Rust closures. No listener owns the app or survives its destruction.
pub(super) struct ShortcutBridge {
    canvas: HtmlCanvasElement,
    enabled: Rc<Cell<bool>>,
    pending: Rc<Cell<usize>>,
    keydown: Closure<dyn FnMut(KeyboardEvent)>,
    composition_start: Closure<dyn FnMut(Event)>,
    composition_end: Closure<dyn FnMut(Event)>,
    blur: Closure<dyn FnMut(Event)>,
}

impl ShortcutBridge {
    pub(super) fn install(canvas: HtmlCanvasElement, ctx: egui::Context) -> Result<Self, JsValue> {
        let document = canvas
            .owner_document()
            .ok_or_else(|| JsValue::from_str("The workspace document is unavailable"))?;
        let enabled = Rc::new(Cell::new(false));
        let pending = Rc::new(Cell::new(0_usize));
        let composing = Rc::new(Cell::new(false));
        let keydown = {
            let enabled = Rc::clone(&enabled);
            let pending = Rc::clone(&pending);
            let composing = Rc::clone(&composing);
            let canvas = canvas.clone();
            Closure::new(move |event: KeyboardEvent| {
                let modifiers = egui::Modifiers {
                    ctrl: event.ctrl_key(),
                    mac_cmd: event.meta_key(),
                    alt: event.alt_key(),
                    shift: event.shift_key(),
                    command: if ctx.os().is_mac() {
                        event.meta_key()
                    } else {
                        event.ctrl_key()
                    },
                };
                if !event.key().eq_ignore_ascii_case("j")
                    || !inspector_chord(modifiers, ctx.os().is_mac())
                    || event.is_composing()
                    || composing.get()
                    || event.default_prevented()
                    || document.hidden()
                    || !document
                        .active_element()
                        .is_some_and(|active| active == **canvas)
                    || !inspector_shortcut_allowed(&ctx, enabled.get())
                {
                    return;
                }
                event.prevent_default();
                // eframe would otherwise queue the same key too. Only this
                // owned chord is stopped; editing and unrelated keys proceed.
                event.stop_immediate_propagation();
                if !event.repeat() {
                    pending.set(pending.get().saturating_add(1));
                    ctx.request_repaint();
                }
            })
        };
        let composition_start = {
            let composing = Rc::clone(&composing);
            Closure::new(move |_: Event| composing.set(true))
        };
        let composition_end = {
            let composing = Rc::clone(&composing);
            Closure::new(move |_: Event| composing.set(false))
        };
        let blur = {
            let pending = Rc::clone(&pending);
            Closure::new(move |_: Event| {
                composing.set(false);
                pending.set(0);
            })
        };
        let bridge = Self {
            canvas,
            enabled,
            pending,
            keydown,
            composition_start,
            composition_end,
            blur,
        };
        // Construct the owner before registering. Drop also cleans up a
        // partially installed bridge if a later registration fails.
        bridge.canvas.add_event_listener_with_callback_and_bool(
            "keydown",
            bridge.keydown.as_ref().unchecked_ref(),
            true,
        )?;
        bridge.canvas.add_event_listener_with_callback_and_bool(
            "compositionstart",
            bridge.composition_start.as_ref().unchecked_ref(),
            true,
        )?;
        bridge.canvas.add_event_listener_with_callback_and_bool(
            "compositionend",
            bridge.composition_end.as_ref().unchecked_ref(),
            true,
        )?;
        bridge.canvas.add_event_listener_with_callback_and_bool(
            "blur",
            bridge.blur.as_ref().unchecked_ref(),
            true,
        )?;
        Ok(bridge)
    }

    pub(super) fn set_available(&self, available: bool) {
        self.enabled.set(available);
    }

    pub(super) fn take_presses(&self) -> usize {
        self.pending.replace(0)
    }
}

impl Drop for ShortcutBridge {
    fn drop(&mut self) {
        let _ = self.canvas.remove_event_listener_with_callback_and_bool(
            "keydown",
            self.keydown.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.canvas.remove_event_listener_with_callback_and_bool(
            "compositionstart",
            self.composition_start.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.canvas.remove_event_listener_with_callback_and_bool(
            "compositionend",
            self.composition_end.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.canvas.remove_event_listener_with_callback_and_bool(
            "blur",
            self.blur.as_ref().unchecked_ref(),
            true,
        );
    }
}
