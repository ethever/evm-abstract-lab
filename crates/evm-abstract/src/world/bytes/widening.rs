//! Conservative metadata form for explicitly selected whole-case cycle widening.

use super::{
    EvmByteArray, RangeError,
    refinement::{Node, walk},
};
use crate::domain::AlgebraRef;
#[cfg(test)]
use crate::domain::ValueAlgebra;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{ByteStateKind, ExprError, ExprId, ExprLimits, SlotStateKind},
};
use alloy_primitives::U256;
use std::collections::BTreeMap;

/// Constant folding after the caller's registered-selector projection is an
/// authoritative proof, while nonconstant external/manual symbols stay intact.
pub(in crate::world) fn refresh_constant<N: WordElement>(value: &mut EvmValue<N>) {
    if let Some(constant) = value.expression().and_then(ExprId::as_constant) {
        *value = value.clone().with_numeric(N::constant(constant));
    }
}

/// Retain guarantees consumed by the authoritative graph, including guarantees
/// referenced by another retained fact. Singleton provenance remains essential
/// even when Zero/Exact normalization removes the original write expression.
pub(in crate::world) fn retained_facts<N: WordElement>(
    pending: Vec<Node>,
    facts: &BTreeMap<ExprId, EvmValue<N>>,
    limits: ExprLimits,
) -> Result<BTreeMap<ExprId, EvmValue<N>>, ExprError> {
    let mut retained = BTreeMap::new();
    let mut work = 0usize;
    let nodes = walk(pending, limits, &mut |node, pending| {
        match node {
            Node::Bytes(bytes) => match bytes.kind() {
                ByteStateKind::Zero => pending.push(Node::Expression(ExprId::constant(U256::ZERO))),
                ByteStateKind::Exact(_) => {
                    // Only retained metadata is inspected, never payload length.
                    work = work.saturating_add(facts.len());
                    pending.extend(
                        facts
                            .keys()
                            .filter(|e| e.as_constant().is_some_and(|n| n <= U256::from(255)))
                            .cloned()
                            .map(Node::Expression),
                    );
                }
                ByteStateKind::WriteByte { value, .. } => {
                    if let Some(value) = value.as_constant() {
                        pending.push(Node::Expression(ExprId::constant(value & U256::from(255))));
                    }
                }
                ByteStateKind::WriteWord { value, .. } => {
                    if let Some(value) = value.as_constant() {
                        pending.extend(
                            value
                                .to_be_bytes::<32>()
                                .into_iter()
                                .map(|byte| Node::Expression(ExprId::constant(U256::from(byte)))),
                        );
                        work = work.saturating_add(32);
                    }
                }
                _ => {}
            },
            Node::Slots(slots) if matches!(slots.kind(), SlotStateKind::Zero) => {
                pending.push(Node::Expression(ExprId::constant(U256::ZERO)));
            }
            _ => {}
        }
        if work > limits.max_nodes {
            return Err(ExprError::Limit {
                nodes: work,
                depth: 1,
            });
        }
        if let Node::Expression(expression) = node
            && let Some(value) = facts.get(expression)
        {
            work = work
                .saturating_add(value.numeric().work_size())
                .saturating_add(1);
            if work > limits.max_nodes {
                return Err(ExprError::Limit {
                    nodes: work,
                    depth: expression.depth(),
                });
            }
            if let Some(value_expression) = value.expression() {
                pending.push(Node::Expression(value_expression.clone()));
            }
            retained.insert(expression.clone(), value.clone());
        }
        Ok(())
    })?;
    let nodes = nodes.saturating_add(work);
    if nodes > limits.max_nodes {
        return Err(ExprError::Limit { nodes, depth: 1 });
    }
    Ok(retained)
}

impl<N: WordElement> EvmByteArray<N> {
    /// Canonicalize disposable cycle metadata without rewriting immutable
    /// content, external operand guarantees, source identity, or logical length.
    pub(crate) fn widening_form<D: EvmNumericAlgebra<Element = N>>(
        &self,
        _domain: AlgebraRef<'_, D>,
        limits: ExprLimits,
    ) -> Result<Self, RangeError> {
        self.check_existing(limits)?;
        let mut pending = vec![Node::Bytes(self.root.clone())];
        if let Some(length) = self.length.expression() {
            pending.push(Node::Expression(length.clone()));
        }
        let mut out = Self {
            length: self.length.clone(),
            bytes: BTreeMap::new(),
            default: EvmValue::<N>::unknown_byte(),
            memory: self.memory,
            root: self.root.clone(),
            allocations: Default::default(),
            operands: retained_facts(pending, &self.operands, limits)?,
            // Keep the bounded conditional provenance tree; pruning it once per
            // materialized location would repeat the complete root traversal.
            conditional: self.conditional.clone(),
            failure: None,
        };
        refresh_constant(&mut out.length);
        out.check_existing(limits)?;
        Ok(out)
    }
}
