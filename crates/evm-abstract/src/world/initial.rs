//! Immutable initial-account projection for the layered compatibility adapter.
//!
//! The caller checks intersection with prior abstract inputs. This container
//! operation preserves snapshot/header/source identity and validates each account;
//! it has no access to transaction Store, history or rollback checkpoints.

use super::{EvmAccount, EvmWorld, WorldError};
use crate::domain::WordElement;
use alloy_primitives::Address;
use std::collections::BTreeMap;

impl<N: WordElement> EvmWorld<N> {
    pub(crate) fn with_initial_accounts(
        &self,
        accounts: BTreeMap<Address, EvmAccount<N>>,
    ) -> Result<Self, WorldError> {
        let mut projected = self.clone();
        for (address, account) in accounts {
            projected.accounts.remove(&address);
            projected.insert(address, account)?;
        }
        Ok(projected)
    }
}
