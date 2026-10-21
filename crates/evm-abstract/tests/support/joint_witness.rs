//! Finite joint assignments shared by private and public root coverage tests.
//! Storage and complete-data admission remain in each consumer.

use super::{ExprId, ExprKind, SlotStateId, SlotStateKind, SourceId, U256};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct JointWitness {
    words: BTreeMap<ExprId, U256>,
    pub(super) cells: BTreeMap<(SourceId, U256), U256>,
}

impl JointWitness {
    pub(super) fn assign<K: Ord>(map: &mut BTreeMap<K, U256>, key: K, value: U256) -> bool {
        match map.get(&key) {
            Some(previous) => *previous == value,
            None => {
                map.insert(key, value);
                true
            }
        }
    }

    pub(super) fn expression(&mut self, expression: &ExprId, expected: U256) -> bool {
        match expression.kind() {
            ExprKind::Constant(value) => *value == expected,
            ExprKind::Input(_) | ExprKind::Fresh(_) => {
                Self::assign(&mut self.words, expression.clone(), expected)
            }
            ExprKind::SlotRead { state, key } => key
                .as_constant()
                .is_some_and(|key| self.slot(state, key, expected)),
            _ => false,
        }
    }

    pub(super) fn slot(&mut self, state: &SlotStateId, key: U256, expected: U256) -> bool {
        match state.kind() {
            SlotStateKind::Zero => expected == U256::ZERO,
            SlotStateKind::Input { source, bindings } => {
                if let Some(binding) = bindings.iter().find(|binding| binding.index == key)
                    && (!binding.numeric.contains(expected)
                        || !self.expression(&binding.value, expected))
                {
                    return false;
                }
                Self::assign(&mut self.cells, (source.clone(), key), expected)
            }
            SlotStateKind::Store {
                previous,
                key: written,
                value,
            } => match written.as_constant() {
                Some(written) if written == key => self.expression(value, expected),
                Some(_) => self.slot(previous, key, expected),
                None => false,
            },
            SlotStateKind::Ite { .. } => false,
        }
    }
}
