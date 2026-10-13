//! Exact Inspect chord ownership shared by native egui input and the browser
//! event bridge. Ordinary text edits share the workbench chord; popups, modals
//! and IME retain keyboard priority.

use egui::{Context, Modifiers};
#[cfg(not(target_arch = "wasm32"))]
use egui::{Event, ImeEvent, Key};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;

pub(crate) fn matches(modifiers: Modifiers, mac: bool) -> bool {
    !modifiers.alt
        && !modifiers.shift
        && if mac {
            modifiers.mac_cmd && !modifiers.ctrl
        } else {
            modifiers.ctrl && !modifiers.mac_cmd
        }
}

pub(crate) fn allowed(ctx: &Context, available: bool) -> bool {
    available
        && !egui::Popup::is_any_open(ctx)
        && ctx.memory(|memory| memory.top_modal_layer().is_none())
}

pub(super) fn hint(ctx: &Context) -> &'static str {
    if ctx.os().is_mac() {
        "Show/hide Inspect (Cmd+J)"
    } else {
        "Show/hide Inspect (Ctrl+J)"
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub(super) struct Shortcut {
    composing: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Shortcut {
    pub(super) fn consume(&mut self, ctx: &Context, available: bool) -> usize {
        let allowed = allowed(ctx, available);
        let mac = ctx.os().is_mac();
        ctx.input_mut(|input| {
            if !input.focused {
                self.composing = false;
            }
            let mut presses = 0;
            input.events.retain(|event| {
                match event {
                    Event::Ime(ImeEvent::Preedit { text, .. }) => {
                        self.composing = !text.is_empty();
                    }
                    Event::Ime(ImeEvent::Commit(_)) => self.composing = false,
                    Event::Key {
                        key: Key::J,
                        pressed: true,
                        repeat,
                        modifiers,
                        ..
                    } if allowed
                        && input.focused
                        && !self.composing
                        && matches(*modifiers, mac) =>
                    {
                        presses += usize::from(!repeat);
                        return false;
                    }
                    _ => {}
                }
                true
            });
            presses
        })
    }
}
