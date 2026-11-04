//! Stable stack projection for active and suspended literal continuations.
//!
//! Every stack-carried literal JUMPDEST participates, including the active
//! target: merging it can turn sequential calls sharing an outer continuation
//! into a false cycle. Computed values and memory never supply tags, even when
//! their numeric facts happen to be exact.

use super::Cell;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ContextKey {
    original: usize,
    height: usize,
    continuations: Vec<(usize, usize)>,
}

impl ContextKey {
    pub(super) fn new(original: usize, stack: &[Cell]) -> Self {
        Self {
            original,
            height: stack.len(),
            continuations: stack
                .iter()
                .enumerate()
                .filter_map(|(slot, cell)| cell.control.map(|pc| (slot, pc)))
                .collect(),
        }
    }
}
