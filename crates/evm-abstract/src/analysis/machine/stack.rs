//! A nonempty call stack with exactly one root and typed child continuations.

use super::{ChildFrame, FrameState, MachineError, RootFrame, join};
use crate::domain::{
    Domain,
    symbolic::{ExprId, ExprLimits},
};
use serde::Serialize;

/// Root and nested execution frames, ordered from oldest caller to active frame.
///
/// The root cannot be popped. Every nested frame has a parent and a mandatory
/// continuation; only the last child, or the root when there are no children,
/// is active. Private storage prevents arbitrary insertion or root removal.
///
/// Mutations must use the child-entry and child-completion operations:
///
/// ```compile_fail
/// use evm_abstract::analysis::CallStack;
/// fn clear_frames(stack: &mut CallStack) {
///     stack.children.clear();
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallStack {
    root: RootFrame,
    children: Vec<ChildFrame>,
}

impl CallStack {
    /// Start a call stack at its entry frame.
    pub fn new(root: RootFrame) -> Self {
        Self {
            root,
            children: Vec::new(),
        }
    }

    pub(crate) fn from_parts(root: RootFrame, children: Vec<ChildFrame>) -> Self {
        Self { root, children }
    }

    /// The entry frame of this stack, whether active or suspended.
    pub fn root(&self) -> &RootFrame {
        &self.root
    }

    /// Nested frames from oldest child to active child.
    pub fn children(&self) -> &[ChildFrame] {
        &self.children
    }

    pub(crate) fn children_mut(&mut self) -> &mut [ChildFrame] {
        &mut self.children
    }

    /// Number of frames, including the root; always at least one.
    pub fn depth(&self) -> usize {
        self.children.len() + 1
    }

    /// Common execution data of the active root or child.
    pub fn active(&self) -> &FrameState {
        self.children
            .last()
            .map_or(&self.root.state, |child| &child.state)
    }

    /// Mutable execution data without exposing the stack's structural storage.
    pub fn active_mut(&mut self) -> &mut FrameState {
        self.children
            .last_mut()
            .map_or(&mut self.root.state, |child| &mut child.state)
    }

    /// The active child, or no child when the root is active.
    pub fn active_child(&self) -> Option<&ChildFrame> {
        self.children.last()
    }

    /// The active child's suspended parent; absent while the root is active.
    pub fn parent(&self) -> Option<&FrameState> {
        match self.children.len() {
            0 => None,
            1 => Some(&self.root.state),
            depth => Some(&self.children[depth - 2].state),
        }
    }

    /// Suspend the active frame and enter a child with its return contract.
    pub fn push_child(&mut self, child: ChildFrame) {
        self.children.push(child);
    }

    /// Complete the active child and resume its parent; never remove the root.
    pub fn pop_child(&mut self) -> Option<ChildFrame> {
        self.children.pop()
    }

    /// Common execution data in root-to-active order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &FrameState> {
        std::iter::once(&self.root.state).chain(self.children.iter().map(|child| &child.state))
    }

    pub(super) fn forget_identities(&mut self) {
        for frame in self.iter_mut() {
            for value in &mut frame.stack {
                value.forget_identity();
            }
            frame.call_value.forget_identity();
            for value in [&mut frame.address_operand, &mut frame.caller_operand]
                .into_iter()
                .flatten()
            {
                value.forget_identity();
            }
        }
        for child in &mut self.children {
            child.continuation.output_offset.forget_identity();
            child.continuation.output_size.forget_identity();
        }
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut FrameState> {
        std::iter::once(&mut self.root.state)
            .chain(self.children.iter_mut().map(|child| &mut child.state))
    }

    pub(super) fn join_guarded(
        &self,
        incoming: &Self,
        domain: Domain,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, MachineError> {
        if self.depth() != incoming.depth() {
            return Err(MachineError::Shape);
        }
        let mut out = self.clone();
        out.root.state =
            self.root
                .state
                .join_guarded(&incoming.root.state, domain, selector.clone(), limits)?;
        for (child, incoming) in out.children.iter_mut().zip(&incoming.children) {
            if child.continuation.return_block != incoming.continuation.return_block
                || child.continuation.creation != incoming.continuation.creation
            {
                return Err(MachineError::Shape);
            }
            child.state =
                child
                    .state
                    .join_guarded(&incoming.state, domain, selector.clone(), limits)?;
            let old = &mut child.continuation;
            let incoming = &incoming.continuation;
            old.output_offset = join::join_value(
                &old.output_offset,
                &incoming.output_offset,
                domain,
                &selector,
                limits,
            )?;
            old.output_size = join::join_value(
                &old.output_size,
                &incoming.output_size,
                domain,
                &selector,
                limits,
            )?;
        }
        Ok(out)
    }
}
