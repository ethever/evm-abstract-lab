//! A new touch contact has a position, not a drag from the last mouse hover.
//! Preserve its actual press for the following input pass, after that position
//! is known. Mouse events and ongoing contact movement retain their ordering.

use egui::{Event, PointerButton, RawInput};

#[derive(Default)]
pub(super) struct ContactStart {
    pending: Option<Event>,
}

impl ContactStart {
    pub(super) fn prepare(&mut self, input: &mut RawInput, primary_touch_started: bool) -> bool {
        if let Some(press) = self.pending.take() {
            input.events.insert(0, press);
            return false;
        }
        if !primary_touch_started {
            return false;
        }
        let Some(index) = input.events.iter().position(|event| {
            matches!(
                event,
                Event::PointerButton {
                    button: PointerButton::Primary,
                    pressed: true,
                    ..
                }
            )
        }) else {
            return false;
        };
        let Event::PointerButton { pos, .. } = input.events[index] else {
            unreachable!()
        };
        let press = std::mem::replace(&mut input.events[index], Event::PointerMoved(pos));
        self.pending = Some(press);
        true
    }
}

#[cfg(test)]
mod tests;
