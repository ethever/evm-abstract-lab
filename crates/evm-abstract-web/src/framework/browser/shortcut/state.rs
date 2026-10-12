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
        if !self.available.get() || !focused || composing || self.composing[target as usize].get() {
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
