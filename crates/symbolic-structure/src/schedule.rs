//! Iterative dependency scheduling; memoization and admission remain caller
//! policies so pointer, structural and contextual identities cannot be mixed.

#[cfg(test)]
mod tests;

/// An iterative postorder stack, retaining parents until their dependencies
/// have been processed. Siblings are entered in the supplied order.
pub struct Postorder<N> {
    pending: Vec<(N, bool)>,
}
impl<N> Postorder<N> {
    /// Schedule one root for entry.
    pub fn new(root: N) -> Self {
        Self {
            pending: vec![(root, false)],
        }
    }
    /// Append an entry directly to the retained stack. Expansion callbacks
    /// use this for each direct dependency in its original order.
    pub fn push(&mut self, node: N) {
        self.pending.push((node, false));
    }
    /// Stream dependencies into the retained stack without a child buffer.
    /// The callback only pushes children; older pending entries stay in place.
    /// Returns the number of appended edges, including repeated identities.
    pub fn expand_with(&mut self, node: N, children: impl FnOnce(&mut Self)) -> usize {
        self.pending.push((node, true));
        let start = self.pending.len();
        children(self);
        let count = self.pending.len() - start;
        // Reverse only these children: the parent and older pending siblings
        // retain their positions, and repeated edges retain their checkpoints.
        self.pending[start..].reverse();
        count
    }
}
impl<N> Iterator for Postorder<N> {
    type Item = (N, bool);
    fn next(&mut self) -> Option<Self::Item> {
        self.pending.pop()
    }
}
