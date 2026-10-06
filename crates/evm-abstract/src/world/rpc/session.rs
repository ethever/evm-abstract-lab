//! Incremental acquisition owns one client and a monotone fixed-snapshot cache.
//!
//! A fetched account becomes visible only after all account observations and
//! both final identity checks succeed. Execution owns its transaction state;
//! this cache contains only initial observations at the once-resolved block hash.

use super::{AccountRequest, AcquisitionLimit, Loader, RpcError, RpcInput, configured_loader};
use crate::world::World;
use alloy_primitives::Address;
use std::collections::BTreeSet;

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

    /// Number of HTTP requests attempted, including checks and failures.
    pub fn requests(&self) -> usize {
        self.loader.next_id
    }

    /// Finish acquisition, retaining the observations without the RPC client.
    pub fn into_world(self) -> World {
        self.world
    }
}
