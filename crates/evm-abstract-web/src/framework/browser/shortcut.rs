//! Synchronous shortcut ownership on the canvas and its exact eframe text agent.
//! eframe 0.36.2 does not cancel Ctrl+J's downloads action. Capturing the chord
//! cancels that action before DOM dispatch returns and delivers each press once.

mod state;

use std::rc::Rc;

use wasm_bindgen::{JsCast as _, JsValue, closure::Closure};
use web_sys::{Element, Event, HtmlCanvasElement, HtmlInputElement, KeyboardEvent};

use crate::app::{inspector_chord, inspector_shortcut_allowed};

use state::{State, Target};

/// The app owns both listener sets; neither closures nor delivery state own the
/// app. Dropping either set removes every callback before releasing its closure.
pub(super) struct ShortcutBridge {
    state: Rc<State>,
    ctx: egui::Context,
    _canvas: Listeners,
    text_agent: Option<Listeners>,
}

impl ShortcutBridge {
    pub(super) fn install(canvas: HtmlCanvasElement, ctx: egui::Context) -> Result<Self, JsValue> {
        let state = Rc::new(State::default());
        let listeners = Listeners::install(
            canvas.into(),
            Target::Canvas,
            ctx.clone(),
            Rc::clone(&state),
        )?;
        Ok(Self {
            state,
            ctx,
            _canvas: listeners,
            text_agent: None,
        })
    }

    pub(super) fn attach_text_agent(&mut self, agent: HtmlInputElement) -> Result<(), JsValue> {
        self.text_agent = Some(Listeners::install(
            agent.into(),
            Target::TextAgent,
            self.ctx.clone(),
            Rc::clone(&self.state),
        )?);
        Ok(())
    }

    pub(super) fn set_available(&self, available: bool) {
        self.state.set_available(available);
    }

    pub(super) fn take_presses(&self) -> usize {
        self.state.take_presses()
    }
}

struct Listeners {
    element: Element,
    keydown: Closure<dyn FnMut(KeyboardEvent)>,
    composition_start: Closure<dyn FnMut(Event)>,
    composition_end: Closure<dyn FnMut(Event)>,
    blur: Closure<dyn FnMut(Event)>,
}

impl Listeners {
    fn install(
        element: Element,
        target: Target,
        ctx: egui::Context,
        state: Rc<State>,
    ) -> Result<Self, JsValue> {
        let document = element
            .owner_document()
            .ok_or_else(|| JsValue::from_str("The workspace document is unavailable"))?;
        let keydown = {
            let element = element.clone();
            let state = Rc::clone(&state);
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
                // A Ctrl+Enter used to commit IME text must never reach the
                // form as submission. Keep the browser's composition default;
                // only stop the framework's keyboard listener for this chord.
                if event.key() == "Enter"
                    && modifiers.ctrl
                    && !modifiers.alt
                    && !modifiers.shift
                    && !modifiers.mac_cmd
                    && state.blocks_submit(
                        target,
                        event.is_composing() || event.key_code() == 229,
                        event.repeat(),
                    )
                {
                    event.stop_immediate_propagation();
                    return;
                }
                if !event.key().eq_ignore_ascii_case("j")
                    || !inspector_chord(modifiers, ctx.os().is_mac())
                    || event.default_prevented()
                    || !inspector_shortcut_allowed(&ctx, true)
                {
                    return;
                }
                let focused = !document.hidden()
                    && document
                        .active_element()
                        .is_some_and(|active| active == element);
                if !state.keydown(target, focused, event.is_composing(), event.repeat()) {
                    return;
                }
                event.prevent_default();
                // Both owned targets use capture; stopping here keeps eframe's
                // bubble listener from delivering the same chord a second time.
                event.stop_immediate_propagation();
                if !event.repeat() {
                    ctx.request_repaint();
                }
            })
        };
        let composition_start = {
            let state = Rc::clone(&state);
            Closure::new(move |_: Event| state.composition(target, true))
        };
        let composition_end = {
            let state = Rc::clone(&state);
            Closure::new(move |_: Event| state.composition(target, false))
        };
        let blur = Closure::new(move |_: Event| state.blur(target));
        let listeners = Self {
            element,
            keydown,
            composition_start,
            composition_end,
            blur,
        };
        // The owner exists before registration, so Drop also removes callbacks
        // already registered when any later registration fails.
        listeners
            .element
            .add_event_listener_with_callback_and_bool(
                "keydown",
                listeners.keydown.as_ref().unchecked_ref(),
                true,
            )?;
        listeners
            .element
            .add_event_listener_with_callback_and_bool(
                "compositionstart",
                listeners.composition_start.as_ref().unchecked_ref(),
                true,
            )?;
        listeners
            .element
            .add_event_listener_with_callback_and_bool(
                "compositionend",
                listeners.composition_end.as_ref().unchecked_ref(),
                true,
            )?;
        listeners
            .element
            .add_event_listener_with_callback_and_bool(
                "blur",
                listeners.blur.as_ref().unchecked_ref(),
                true,
            )?;
        Ok(listeners)
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        let _ = self.element.remove_event_listener_with_callback_and_bool(
            "keydown",
            self.keydown.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.element.remove_event_listener_with_callback_and_bool(
            "compositionstart",
            self.composition_start.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.element.remove_event_listener_with_callback_and_bool(
            "compositionend",
            self.composition_end.as_ref().unchecked_ref(),
            true,
        );
        let _ = self.element.remove_event_listener_with_callback_and_bool(
            "blur",
            self.blur.as_ref().unchecked_ref(),
            true,
        );
    }
}
