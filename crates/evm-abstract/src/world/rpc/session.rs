//! Incremental acquisition owns one client and a monotone fixed-snapshot cache.
//!
//! Fetched accounts and storage batches become visible only after all requested
//! observations and both final identity checks succeed. Execution owns its
//! transaction state; this cache contains only initial observations at the
//! once-resolved block hash.

use super::{AccountRequest, AcquisitionLimit, Loader, RpcError, RpcInput, configured_loader};
use crate::{domain::Value, world::World};
use alloy_primitives::{Address, U256};
use std::collections::{BTreeMap, BTreeSet};

/// Bounded RPC acquisition whose observed accounts never change or disappear.
#[derive(Debug)]
pub struct Session {
    loader: Loader,
    world: World,
}

impl Session {
    /// Load the explicitly selected accounts as one checked initial batch.
    pub fn load(input: &RpcInput) -> Result<Self, RpcError> {
        let mut loader = configured_loader(input)?;
        loader.pin()?;
        let mut world = loader.world();
        for request in &input.accounts {
            loader.account(&mut world, request)?;
        }
        loader.check_chain()?;
        loader.check_block()?;
        Ok(Self { loader, world })
    }

    /// Current complete account observations at the fixed chain and block hash.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Acquire a discovered account, or reuse its prior cached observation.
    ///
    /// Returns `true` only when a new account was installed. No storage slots
    /// are inferred: unrequested slots of present accounts remain unknown.
    /// Failed acquisition consumes its attempted requests without inserting
    /// partial account data into the world.
    pub fn fetch_account(&mut self, address: Address) -> Result<bool, RpcError> {
        if self.world.account(address).is_some() {
            return Ok(false);
        }
        let input = &self.loader.input;
        if self.world.accounts().len() >= input.max_accounts {
            return Err(RpcError::AcquisitionLimit {
                context: Box::new(self.loader.context("eth_getCode", Some(address), None)),
                resource: AcquisitionLimit::Accounts,
                limit: input.max_accounts,
            });
        }
        let mut pending = self.loader.world();
        self.loader.check_chain()?;
        self.loader.check_block()?;
        self.loader.account(
            &mut pending,
            &AccountRequest {
                address,
                slots: BTreeSet::new(),
            },
        )?;
        self.loader.check_chain()?;
        self.loader.check_block()?;
        let account = pending
            .account(address)
            .expect("validated account was inserted");
        self.world
            .insert(address, account.clone())
            .map_err(|source| RpcError::World {
                context: Box::new(self.loader.context("eth_getCode", Some(address), None)),
                source,
            })?;
        Ok(true)
    }

    /// Acquire missing initial slots of an already observed account.
    ///
    /// Returns the keys installed by this batch, in ascending order. Cached
    /// slots, including observed zeros, and complete storage need no requests.
    /// Every new fact uses the session's exact canonical block hash. The fixed
    /// height is checked against that hash before and after the batch, including
    /// when the endpoint can still return a reorganized block by its old hash.
    /// A missing or malformed pinned height is an error before acquisition. The
    /// cumulative request budget also bounds retained slot observations;
    /// pending storage grows only after a successful bounded request.
    ///
    /// Failed acquisition retains its attempted request count and installs none
    /// of this batch. Account code, balance, nonce and established slot facts
    /// are never acquired again or overwritten.
    pub fn fetch_storage(
        &mut self,
        address: Address,
        slots: &BTreeSet<U256>,
    ) -> Result<Vec<U256>, RpcError> {
        let account = self
            .world
            .account(address)
            .ok_or_else(|| RpcError::Configuration {
                context: Box::new(self.loader.context(
                    "eth_getStorageAt",
                    Some(address),
                    slots.first().copied(),
                )),
                reason: "storage acquisition requires an already observed account",
            })?;
        if !account.storage_unknown {
            return Ok(Vec::new());
        }
        let mut missing = slots
            .iter()
            .copied()
            .filter(|key| !account.storage.contains_key(key))
            .peekable();
        if missing.peek().is_none() {
            return Ok(Vec::new());
        }
        let block_number = self.loader.storage_block_number()?;
        self.loader.check_chain()?;
        self.loader.check_storage_block(block_number)?;
        let mut pending = BTreeMap::new();
        for key in missing {
            pending.insert(key, Value::constant(self.loader.storage(address, key)?));
        }
        self.loader.check_chain()?;
        self.loader.check_storage_block(block_number)?;
        let installed = pending.keys().copied().collect();
        self.world
            .install_storage(address, pending)
            .map_err(|source| RpcError::World {
                context: Box::new(self.loader.context("eth_getStorageAt", Some(address), None)),
                source,
            })?;
        Ok(installed)
    }

    /// Number of HTTP requests attempted, including checks and failures.
    pub fn requests(&self) -> usize {
        self.loader.next_id
    }

    /// Finish acquisition, retaining the observations without the RPC client.
    pub fn into_world(self) -> World {
        self.world
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, configured_loader};
    use crate::{
        Address, Fork, U256,
        world::{Account, rpc::RpcInput},
    };
    use alloy_primitives::B256;
    use std::collections::BTreeSet;

    #[test]
    fn complete_and_absent_storage_need_no_requests_or_extra_observations() {
        for account in [Account::empty(), Account::absent()] {
            let mut loader =
                configured_loader(&RpcInput::new("http://127.0.0.1:1", Fork::Osaka)).unwrap();
            loader.chain_id = Some(U256::from(1));
            loader.block_hash = Some(B256::repeat_byte(0x11));
            let mut world = loader.world();
            let address = Address::repeat_byte(0x22);
            world.insert(address, account.clone()).unwrap();
            let mut session = Session { loader, world };
            assert!(
                session
                    .fetch_storage(address, &BTreeSet::from([U256::ZERO, U256::from(2)]))
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(session.requests(), 0);
            assert_eq!(session.world().account(address), Some(&account));
        }
    }
}
