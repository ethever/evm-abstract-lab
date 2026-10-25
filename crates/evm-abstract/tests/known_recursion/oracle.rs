//! Shared concrete collection, with recursive unchanged-state checks below.
use super::support::{address, word};
use crate::native;
use alloy_primitives::U256;
use evm_abstract::{analysis::EvmWorldAnalysis, domain::NumericValue, world::Existence};
#[path = "../support/revm.rs"]
mod concrete;
pub(super) use concrete::{execute, execute_gas};

pub(super) fn unchanged(analysis: &EvmWorldAnalysis<NumericValue>, owners: &[u64]) {
    for outcome in analysis.outcomes() {
        for owner in owners {
            assert_eq!(
                outcome.store.nonce(address(*owner)).singleton(),
                Some(U256::ZERO)
            );
            assert_eq!(outcome.store.existence(address(*owner)), Existence::Present);
            assert_eq!(
                outcome.store.created_in_transaction(address(*owner)),
                Some(false)
            );
            assert_eq!(
                outcome.store.pending_destruction(address(*owner)),
                Some(false)
            );
            assert_eq!(
                outcome
                    .store
                    .read_transient(address(*owner), &word(1), native::product().borrowed())
                    .singleton(),
                Some(U256::ZERO)
            );
        }
    }
}
