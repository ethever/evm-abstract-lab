//! Iterative dependency scheduling; memoization and admission remain caller
//! policies so pointer, structural and contextual identities cannot be mixed.

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
    /// Retain a parent for exit and enter its ordered dependencies first.
    /// Callers skip memo hits before expanding; repeated siblings therefore
    /// retain their existing cancellation and charging boundaries.
    pub fn expand<I: IntoIterator<Item = N>>(&mut self, node: N, children: I)
    where
        I::IntoIter: DoubleEndedIterator,
    {
        self.pending.push((node, true));
        self.pending
            .extend(children.into_iter().rev().map(|child| (child, false)));
    }
}
impl<N> Iterator for Postorder<N> {
    type Item = (N, bool);
    fn next(&mut self) -> Option<Self::Item> {
        self.pending.pop()
    }
}
