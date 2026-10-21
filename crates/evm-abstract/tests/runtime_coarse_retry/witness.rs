//! One typed snapshot assignment, shared across current/rollback roots and
//! owners. Unsupported expression forms fail instead of using field membership
//! as a substitute for a common valuation. These summaries have no path formula.
use crate::native;

use super::support::{B, BALANCE, U, address, word};
use alloy_primitives::U256;
use evm_abstract::domain::{NumericValue, ProductAlgebra};
use evm_abstract::{
    domain::{
        ValueAlgebra, WordObservation,
        symbolic::{ByteStateKind, ExprId, ExprKind, SlotStateId, SlotStateKind, SourceId},
    },
    world::{EvmByteArray, EvmStore},
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Assignment {
    scalars: BTreeMap<ExprId, U256>,
    cells: BTreeMap<(SourceId, U256), U256>,
}

impl Assignment {
    fn choose(numeric: &WordObservation) -> U256 {
        let value = numeric
            .finite_constants()
            .as_values()
            .and_then(|values| values.first())
            .copied()
            .unwrap_or_else(|| numeric.interval().unsigned_bounds().0);
        assert!(
            numeric.contains(value),
            "no admitted fixture representative"
        );
        value
    }

    fn cell(&mut self, source: &SourceId, key: U256, value: U256) {
        if let Some(old) = self.cells.insert((source.clone(), key), value) {
            assert_eq!(
                old, value,
                "one immutable source cell received two assignments"
            );
        }
    }

    fn scalar(&mut self, expression: &ExprId, value: U256) {
        match expression.kind() {
            ExprKind::Constant(constant) => assert_eq!(*constant, value),
            ExprKind::Fresh(_) => {
                if let Some(old) = self.scalars.insert(expression.clone(), value) {
                    assert_eq!(old, value);
                }
            }
            ExprKind::SlotRead { state, key } => {
                self.slots(
                    state,
                    key.as_constant().expect("fixture uses constant keys"),
                    value,
                );
            }
            _ => panic!("unsupported snapshot scalar: {expression:?}"),
        }
    }

    fn slots(&mut self, root: &SlotStateId, key: U256, value: U256) {
        match root.kind() {
            SlotStateKind::Zero => assert_eq!(value, U256::ZERO),
            SlotStateKind::Input { source, bindings } => {
                self.cell(source, key, value);
                for binding in bindings {
                    let value = self
                        .cells
                        .get(&(source.clone(), binding.index))
                        .copied()
                        .unwrap_or_else(|| Self::choose(&binding.numeric));
                    assert!(binding.numeric.contains(value));
                    self.cell(source, binding.index, value);
                    self.scalar(&binding.value, value);
                }
            }
            SlotStateKind::Store {
                previous,
                key: at,
                value: written,
            } => {
                let at = at
                    .as_constant()
                    .expect("fixture only stores at constant keys");
                if at == key {
                    self.scalar(written, value);
                    // All other cells of the overwritten input can be zero;
                    // check every binding even when that history is not read.
                    self.history(previous);
                } else {
                    self.history_value(written);
                    self.slots(previous, key, value);
                }
            }
            _ => panic!("conditional snapshot is outside this fixture: {root:?}"),
        }
    }

    fn history_value(&mut self, expression: &ExprId) {
        let value = expression
            .as_constant()
            .expect("only literal ordered writes");
        self.scalar(expression, value);
    }

    fn history(&mut self, root: &SlotStateId) {
        match root.kind() {
            SlotStateKind::Zero => {}
            SlotStateKind::Input { source, bindings } => {
                for binding in bindings {
                    let value = self
                        .cells
                        .get(&(source.clone(), binding.index))
                        .copied()
                        .unwrap_or_else(|| Self::choose(&binding.numeric));
                    assert!(binding.numeric.contains(value));
                    self.cell(source, binding.index, value);
                    self.scalar(&binding.value, value);
                }
            }
            SlotStateKind::Store {
                previous, value, ..
            } => {
                self.history_value(value);
                self.history(previous);
            }
            _ => panic!("conditional history is outside this fixture"),
        }
    }

    pub(super) fn exact_bytes(&mut self, data: &EvmByteArray<NumericValue>, expected: &[u8]) {
        assert_eq!(data.len().singleton(), Some(U256::from(expected.len())));
        let ByteStateKind::Exact(bytes) = data.state_root().kind() else {
            panic!("expected one immutable exact returned buffer");
        };
        assert_eq!(bytes.as_ref(), expected);
        assert_eq!(
            data.exact_bytes(native::product().borrowed()).as_deref(),
            Some(expected)
        );
    }

    /// Complete these two fixture planes with zero at all unlisted keys.
    /// SourceId includes its namespace; equal tokens never merge owners/planes.
    pub(super) fn store(
        &mut self,
        store: &EvmStore<NumericValue>,
        persistent: u64,
        transient: u64,
        domain: &ValueAlgebra<ProductAlgebra>,
    ) {
        for (owner, slot0, slot1) in [(B, persistent, 5), (U, 0, 0)] {
            for (transient_plane, values) in [
                (false, [slot0, slot1]),
                (true, [if owner == B { transient } else { 0 }, 0]),
            ] {
                let root = store.slot_root(address(owner), transient_plane).unwrap();
                for (key, expected) in values.into_iter().enumerate() {
                    self.slots(&root, U256::from(key), U256::from(expected));
                    let value = if transient_plane {
                        store.read_transient(address(owner), &word(key as u64), domain.borrowed())
                    } else {
                        store.read(address(owner), &word(key as u64), domain.borrowed())
                    };
                    assert!(value.contains(U256::from(expected)));
                }
            }
            for (value, expected) in [
                (store.read_balance(address(owner)), BALANCE),
                (store.nonce(address(owner)), 0),
            ] {
                assert!(value.contains(U256::from(expected)));
                if let Some(expression) = value.expression() {
                    self.scalar(expression, U256::from(expected));
                } else {
                    // Unlisted balance/nonce defaults denote independent
                    // account fields, with no hidden shared scalar identity.
                    assert!(
                        value.numeric().is_top() || value.singleton() == Some(U256::from(expected))
                    );
                }
            }
        }
    }
}
