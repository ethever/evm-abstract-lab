//! DOM-independent delivery state for the two owned keyboard targets.

use std::cell::Cell;

#[derive(Clone, Copy)]
pub(super) enum Target {
    Canvas = 0,
    TextAgent = 1,
}

#[derive(Default)]
pub(super) struct State {
    available: Cell<bool>,
    composing: [Cell<bool>; 2],
    pending: Cell<usize>,
}

impl State {
    pub(super) fn blocks_submit(&self, target: Target, composing: bool, repeat: bool) -> bool {
        composing || repeat || self.composing[target as usize].get()
    }

    pub(super) fn set_available(&self, available: bool) {
        self.available.set(available);
    }

    pub(super) fn composition(&self, target: Target, composing: bool) {
        self.composing[target as usize].set(composing);
    }

    pub(super) fn blur(&self, target: Target) {
        self.composition(target, false);
        // eframe moves focus between canvas and text agent after clicks and
        // repaint. Already accepted presses must survive those transitions.
    }

    /// Returns whether to cancel DOM dispatch. Repeats still cancel the browser
    /// action, but only the initial keydown is delivered to the workbench.
    pub(super) fn keydown(
        &self,
        target: Target,
        focused: bool,
        composing: bool,
        repeat: bool,
    ) -> bool {
        if !self.available.get() || !focused || self.blocks_submit(target, composing, false) {
            return false;
        }
        if !repeat {
            self.pending.set(self.pending.get().saturating_add(1));
        }
        true
    }

    pub(super) fn take_presses(&self) -> usize {
        self.pending.replace(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{State, Target};

    #[test]
    fn submit_guard_preserves_inspect_delivery_and_clears_composition_on_blur() {
        let state = State::default();
        state.set_available(true);
        for target in [Target::Canvas, Target::TextAgent] {
            assert!(!state.blocks_submit(target, false, false));
            assert!(state.blocks_submit(target, true, false));
            assert!(state.blocks_submit(target, false, true));
            state.composition(target, true);
            assert!(state.blocks_submit(target, false, false));
            assert!(!state.keydown(target, true, false, false));
            state.blur(target);
            assert!(!state.blocks_submit(target, false, false));
            assert!(state.keydown(target, true, false, false));
        }
        assert_eq!(state.take_presses(), 2);
    }
}
