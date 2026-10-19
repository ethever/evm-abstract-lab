//! Finite must-equalities between stack positions, without retained expressions.

use super::{AbstractValue, BTreeMap, MachineError, identify};

#[cfg(test)]
mod tests;

/// Each class is labelled by its first position; singleton positions label themselves.
/// Mixed numeric bounds on one node deliberately lose their alias guarantee.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::analysis) struct StackAliases(Vec<usize>);

impl StackAliases {
    const MAX_STACK: usize = 1024;

    /// Prepaid logical work for retained labels, two tree passes, copying and
    /// intersection. Length and tree depth are available without graph traversal.
    pub(in crate::analysis) fn work_for_len(len: usize) -> usize {
        let levels = usize::try_from(len.max(1).ilog2())
            .unwrap_or(usize::MAX)
            .saturating_add(1);
        len.saturating_mul(levels.saturating_add(1))
            .saturating_mul(16)
    }

    pub(in crate::analysis) fn work_size(&self) -> usize {
        Self::work_for_len(self.0.len())
    }

    /// Only membership in this partition is evidence; absent positions never alias.
    pub(in crate::analysis) fn same_class(&self, left: usize, right: usize) -> bool {
        matches!((self.0.get(left), self.0.get(right)), (Some(a), Some(b)) if a == b)
    }

    /// Caller reserves tree work and numeric comparison work before entering.
    pub(in crate::analysis) fn form(
        stack: &[AbstractValue],
        enabled: bool,
    ) -> Result<Self, MachineError> {
        MachineError::check_cancelled()?;
        if stack.len() > Self::MAX_STACK {
            return Err(MachineError::Shape);
        }
        let mut nodes = BTreeMap::<usize, (usize, bool)>::new();
        if enabled {
            for (position, value) in stack.iter().enumerate() {
                MachineError::check_cancelled()?;
                if let Some(expression) = value.expression() {
                    let (leader, uniform) = nodes
                        .entry(expression.immutable_node_key())
                        .or_insert((position, true));
                    // Numeric equality never compares ExprId. A different bound
                    // drops this node's entire class instead of choosing a bound.
                    *uniform &= value.numeric() == stack[*leader].numeric();
                }
            }
        }
        let mut labels = Vec::with_capacity(stack.len());
        for (position, value) in stack.iter().enumerate() {
            MachineError::check_cancelled()?;
            let leader = value.expression().and_then(|expression| {
                nodes
                    .get(&expression.immutable_node_key())
                    .and_then(|&(leader, uniform)| uniform.then_some(leader))
            });
            labels.push(leader.unwrap_or(position));
        }
        Ok(Self(labels))
    }

    /// Intersection of equivalence relations; leader numbers are local to each
    /// partition, so equal guarantees survive even when the leaders differ.
    pub(in crate::analysis) fn intersect(&self, other: &Self) -> Result<Self, MachineError> {
        MachineError::check_cancelled()?;
        if self.0.len() != other.0.len() {
            return Err(MachineError::Shape);
        }
        let mut classes = BTreeMap::new();
        let mut labels = Vec::with_capacity(self.0.len());
        for (position, (&left, &right)) in self.0.iter().zip(&other.0).enumerate() {
            MachineError::check_cancelled()?;
            labels.push(*classes.entry((left, right)).or_insert(position));
        }
        Ok(Self(labels))
    }

    /// A fresh leader per invocation, sharing only its expression. Every stack
    /// position keeps its own numerical bound, including mixed-bound singletons.
    pub(in crate::analysis) fn instantiate(
        &self,
        stack: &[AbstractValue],
        enabled: bool,
    ) -> Result<Vec<AbstractValue>, MachineError> {
        MachineError::check_cancelled()?;
        if self.0.len() != stack.len() {
            return Err(MachineError::Shape);
        }
        let mut output: Vec<AbstractValue> = Vec::with_capacity(stack.len());
        for (position, (value, &leader)) in stack.iter().zip(&self.0).enumerate() {
            MachineError::check_cancelled()?;
            if leader > position {
                return Err(MachineError::Shape);
            }
            let value = if enabled && leader < position {
                output[leader].expression().map_or_else(
                    || identify(value.clone(), enabled),
                    |expression| value.clone().with_expression(expression.clone()),
                )
            } else {
                identify(value.clone(), enabled)
            };
            output.push(value);
        }
        Ok(output)
    }
}
