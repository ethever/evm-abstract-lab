//! Ordinary If layouts prepared from the projected semantic tree before rows
//! are formatted. Source arms and typed predicate polarity remain separate from
//! their spelling; the executable witness is never edited by this plan.
use super::{LiftedRenderError as Error, LiftedRenderIndex};
use crate::{EntityId, MAX_STRUCTURED_WIRE_DEPTH, MAX_STRUCTURED_WIRE_NODES, StructuredKind};
use std::collections::{BTreeMap, BTreeSet};

/// Selected arms of one ordinary source If. Require and failure-first projection
/// retain priority; consumers use this layout only for their Original case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiftedBranchLayout {
    /// Complement the original typed predicate, rather than its numeric word.
    pub invert: bool,
    /// Original source arm rendered as the visible then body.
    pub then_node: EntityId,
    /// Original source arm rendered as else, when it has semantic content.
    pub else_node: Option<EntityId>,
}

/// One immutable rendering boundary under one actual call-substitution policy.
/// Rebuild after changing expansion; layouts cannot borrow a procedure's caller
/// context or a different prepared projection.
#[derive(Debug, Default)]
pub struct LiftedBranchLayouts {
    layouts: BTreeMap<EntityId, LiftedBranchLayout>,
}

impl LiftedBranchLayouts {
    /// Prepared layout for the exact source If occurrence.
    pub fn get(&self, source: EntityId) -> Option<&LiftedBranchLayout> {
        self.layouts.get(&source)
    }
}

impl LiftedRenderIndex<'_> {
    /// Prepare ordinary If layouts in one bounded postorder, without formatting
    /// body strings or rescanning each If's descendants. The caller reports the
    /// certified calls its actual prepared view substitutes: browser base indexes
    /// deliberately do not own that admission. Canonical definitions report
    /// false for their own body root, which they expand instead of calling.
    ///
    /// Annotations and dependency tokens are context, independent of visibility
    /// toggles. Live scalar transfers, calls, diagnostics and all controls remain
    /// content. Even a double-empty If keeps its condition at the original site.
    /// Refusal returns no partial map; owners retain their existing failure path.
    pub fn plan_branch_layouts(
        &self,
        boundary: EntityId,
        mut substituted_call: impl FnMut(EntityId) -> bool,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<LiftedBranchLayouts, Error> {
        let mut pay = |amount| {
            if reserve(amount) {
                Ok(())
            } else {
                Err(Error::Limit)
            }
        };
        pay(32)?;
        let mut pending = vec![(boundary, 1_usize)];
        let mut order = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some((id, depth)) = pending.pop() {
            pay(32)?;
            let node = self.node(id).ok_or(Error::MissingWitness(id))?;
            if !seen.insert(id) {
                return Err(Error::MissingWitness(id));
            }
            let atomic_call = substituted_call(id);
            let children = if atomic_call {
                0
            } else {
                match &node.kind {
                    StructuredKind::Sequence(children) => children.len(),
                    StructuredKind::If { .. } | StructuredKind::While { .. } => 2,
                    StructuredKind::Indirect { arms, .. } => arms.len(),
                    _ => 0,
                }
            };
            if depth > MAX_STRUCTURED_WIRE_DEPTH
                || order
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(children)
                    .saturating_add(1)
                    > MAX_STRUCTURED_WIRE_NODES
            {
                return Err(Error::Limit);
            }
            pay(children.saturating_mul(8))?;
            order.push((node, atomic_call));
            if atomic_call {
                continue;
            }
            match &node.kind {
                StructuredKind::Sequence(children) => {
                    pending.extend(children.iter().map(|node| (node.id, depth + 1)));
                }
                StructuredKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    pending.push((then_branch.id, depth + 1));
                    pending.push((else_branch.id, depth + 1));
                }
                StructuredKind::While { header, body, .. } => {
                    pending.push((header.id, depth + 1));
                    pending.push((body.id, depth + 1));
                }
                StructuredKind::Indirect { arms, .. } => {
                    pending.extend(arms.iter().map(|arm| (arm.body.id, depth + 1)));
                }
                _ => {}
            }
        }
        let mut content = BTreeMap::new();
        let mut result = LiftedBranchLayouts::default();
        for (node, atomic_call) in order.into_iter().rev() {
            pay(32)?;
            let substantive = if atomic_call {
                true
            } else {
                match &node.kind {
                    StructuredKind::Sequence(children) => {
                        pay(children.len().saturating_mul(4))?;
                        children.iter().any(|child| content[&child.id])
                    }
                    StructuredKind::Block { block } => {
                        if let Some(high) = self.block(*block) {
                            pay(high.statements.len().saturating_mul(8))?;
                            !self.raw_blocks.contains(block)
                                || high
                                    .statements
                                    .iter()
                                    .any(|statement| self.statement_has_row(statement, boundary))
                        } else {
                            true
                        }
                    }
                    StructuredKind::Transfer { edge } => {
                        // Missing metadata is a diagnostic, never empty evidence.
                        !self.edges.contains_key(edge)
                            || self
                                .transfer(*edge)
                                .is_none_or(|transfer| !transfer.arguments.is_empty())
                    }
                    StructuredKind::If {
                        then_branch,
                        else_branch,
                        ..
                    } => {
                        let then_content = content[&then_branch.id];
                        let else_content = content[&else_branch.id];
                        let layout = if !then_content && else_content {
                            LiftedBranchLayout {
                                invert: true,
                                then_node: else_branch.id,
                                else_node: None,
                            }
                        } else {
                            LiftedBranchLayout {
                                invert: false,
                                then_node: then_branch.id,
                                else_node: else_content.then_some(else_branch.id),
                            }
                        };
                        pay(32)?;
                        result.layouts.insert(node.id, layout);
                        true
                    }
                    // The predicate, loop or exit itself remains observable
                    // presentation, even when all descendant bodies are empty.
                    _ => true,
                }
            };
            content.insert(node.id, substantive);
        }
        Ok(result)
    }
}
