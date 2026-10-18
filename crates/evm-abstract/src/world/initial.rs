//! Immutable initial-account projection for the layered compatibility adapter.
//!
//! The caller checks intersection with prior abstract inputs. This container
//! operation preserves snapshot/header/source identity and validates each account;
//! it has no access to transaction Store, history or rollback checkpoints.

use super::{Account, World, WorldError};
use alloy_primitives::Address;
use std::collections::BTreeMap;

impl World {
    pub(crate) fn with_initial_accounts(
        &self,
        accounts: BTreeMap<Address, Account>,
    ) -> Result<Self, WorldError> {
        let mut projected = self.clone();
        for (address, account) in accounts {
            projected.accounts.remove(&address);
            projected.insert(address, account)?;
        }
        Ok(projected)
    }
}
