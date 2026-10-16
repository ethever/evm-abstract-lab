//! Transaction touch facts and empty-account clearing under EIP-161.
//!
//! Option<bool> denotes {false}, {true}, or both. A touch is monotone during
//! forward execution, but ordinary child rollback restores the captured facts.

use super::Store;
use crate::{
    domain::AbstractValue,
    world::{Code, Existence},
};
use alloy_primitives::{Address, U256};

pub(super) fn and(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

pub(super) fn or(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

fn is_zero(value: &AbstractValue) -> Option<bool> {
    if !value.may_be_zero() {
        Some(false)
    } else if !value.may_be_nonzero() {
        Some(true)
    } else {
        None
    }
}

impl Store {
    /// Transaction touch guarantee for every address without an explicit entry.
    /// None includes both touched and untouched accounts, even when unlisted.
    pub fn touched_default(&self) -> Option<bool> {
        self.touched_default
    }

    /// Some(true) means every represented path touched the account; Some(false)
    /// means none did; None retains both possibilities. Missing keys use the
    /// transaction default, independently of account-presence observations.
    pub fn touched_in_transaction(&self, address: Address) -> Option<bool> {
        self.touched
            .get(&address)
            .copied()
            .unwrap_or(self.touched_default)
    }

    /// Record a definite protocol touch inside the operation's checkpoint.
    /// Touching an absent account does not materialize an empty account.
    pub fn touch_account(&mut self, address: Address) {
        self.touched.insert(address, Some(true));
    }

    /// Cover an optional new touch without forgetting an earlier definite one.
    pub fn may_touch_account(&mut self, address: Address) {
        let touched = or(self.touched_in_transaction(address), None);
        self.touched.insert(address, touched);
    }

    /// Cover a recipient whose concrete address is unknown. Existing must-touch
    /// guarantees remain valid; every other address may have been selected.
    pub fn may_touch_all(&mut self) {
        self.touched
            .update_values(|value| *value = or(*value, None));
        self.touched_default = or(self.touched_default, None);
    }

    /// Empty is code-empty AND nonce-zero AND balance-zero. Any proved nonempty
    /// component prevents clearing; uncertain zero/nonzero never proves empty.
    pub(super) fn is_empty(&self, address: Address) -> Option<bool> {
        let code_empty = match self.code(address) {
            Some(Code::Empty) => Some(true),
            Some(Code::Runtime(program)) => Some(program.byte_len() == 0),
            Some(Code::Delegation(_)) => Some(false),
            Some(Code::Unknown) | None => None,
        };
        and(
            code_empty,
            and(
                is_zero(&self.nonce(address)),
                is_zero(&self.read_balance(address)),
            ),
        )
    }

    pub(super) fn delete_account(&mut self, address: Address) {
        self.persistent.reset_account(address);
        self.transient.reset_account(address);
        self.codes.insert(address, Code::Empty);
        self.nonces
            .insert(address, AbstractValue::constant(U256::ZERO));
        self.balances
            .insert(address, AbstractValue::constant(U256::ZERO));
        self.existence.insert(address, Existence::Absent);
    }
}
