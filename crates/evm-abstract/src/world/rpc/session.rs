//! Incremental acquisition owns one client and a monotone fixed-snapshot cache.
//!
//! Fetched accounts and storage batches become visible only after all requested
//! observations and both final identity checks succeed. Execution owns its
//! transaction state; this cache contains only initial observations at the
//! once-resolved block hash.

#[cfg(test)]
use super::configured_loader;
use super::{
    AccountRequest, AcquisitionLimit, Loader, RpcError, RpcInput, configured_loader_with_control,
};
use crate::{
    analysis::{
        control::Control,
        progress::{Observer, Phase},
    },
    domain::WordElement,
    world::{Code, EvmWorld, Existence},
};
use alloy_primitives::{Address, U256};
use std::collections::{BTreeMap, BTreeSet};

/// Exact transport observations remain independent of the chosen numeric carrier.
/// Only an atomically checked batch is published to this cache or the native world.
#[derive(Debug)]
pub(super) struct AccountObservation {
    pub(super) code: Code,
    pub(super) balance: U256,
    pub(super) nonce: U256,
    pub(super) existence: Existence,
    pub(super) storage: BTreeMap<U256, U256>,
}

/// Bounded RPC acquisition whose observed accounts never change or disappear.
#[derive(Debug)]
pub struct Session<N: WordElement> {
    loader: Loader,
    world: EvmWorld<N>,
    observations: BTreeMap<Address, AccountObservation>,
}

impl<N: WordElement> Session<N> {
    /// Load the explicitly selected accounts as one checked initial batch.
    pub fn load(input: &RpcInput) -> Result<Self, RpcError> {
        Self::load_with_control(input, &Control::default())
    }

    /// Load initial observations with cooperative cancellation and progress.
    pub fn load_with_control(input: &RpcInput, control: &Control) -> Result<Self, RpcError> {
        let mut loader = configured_loader_with_control(input, control)?;
        loader.pin()?;
        control.observer().phase(Phase::Acquiring);
        let mut world = loader.world();
        let mut observations = BTreeMap::new();
        for request in &input.accounts {
            observations.insert(request.address, loader.account(&mut world, request)?);
        }
        loader.check_chain()?;
        loader.check_block()?;
        loader.observe_world(&world);
        Ok(Self {
            loader,
            world,
            observations,
        })
    }

    /// Observe acquisition without enabling cancellation.
    pub fn load_with_observer(input: &RpcInput, observer: &Observer) -> Result<Self, RpcError> {
        Self::load_with_control(input, &Control::with_observer(observer.clone()))
    }

    pub(crate) fn discovery_round(&mut self, round: usize) {
        self.loader.round = round;
        self.loader.progress();
    }

    /// Current complete account observations at the fixed chain and block hash.
    pub fn world(&self) -> &EvmWorld<N> {
        &self.world
    }

    pub(crate) fn layered<'a, D, C, P, S, X>(
        &self,
        engine: crate::analysis::AnalysisEngine<'a, D, C, P, S, X>,
        entry: crate::world::EvmEntry<N>,
        config: crate::analysis::ExecutionConfig,
        control: &Control,
    ) -> super::layers::AcquisitionSessionResult<'a, D, C, P, S, X>
    where
        D: crate::domain::EvmNumericAlgebra<Element = N>,
        C: crate::analysis::ContextPolicy,
        P: crate::analysis::PartitionPolicy,
        S: crate::analysis::StatePolicy<D, P::Class>,
        X: crate::analysis::cooperation::Composition<N>,
    {
        super::layers::start(
            self.loader.world(),
            &self.observations,
            engine,
            entry,
            config,
            control,
        )
    }

    pub(crate) fn refine_layered<D, C, P, S, X>(
        &self,
        session: &mut crate::analysis::layers::AnalysisSession<'_, D, C, P, S, X>,
        addresses: impl IntoIterator<Item = Address>,
        control: &Control,
    ) -> Result<
        (),
        crate::analysis::layers::SessionError<
            <X as crate::analysis::cooperation::Composition<N>>::Error,
        >,
    >
    where
        D: crate::domain::EvmNumericAlgebra<Element = N>,
        C: crate::analysis::ContextPolicy,
        P: crate::analysis::PartitionPolicy,
        S: crate::analysis::StatePolicy<D, P::Class>,
        X: crate::analysis::cooperation::Composition<N>,
    {
        super::layers::refine(&self.world, &self.observations, session, addresses, control)
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
        self.loader.control.observer().phase(Phase::Acquiring);
        self.loader.check_chain()?;
        self.loader.check_block()?;
        let observation = self.loader.account(
            &mut pending,
            &AccountRequest {
                address,
                slots: BTreeSet::new(),
            },
        )?;
        self.loader.check_chain()?;
        self.loader.check_block()?;
        let account = pending
            .accounts
            .remove(&address)
            .expect("validated account was inserted");
        self.world
            .insert(address, account)
            .map_err(|source| RpcError::World {
                context: Box::new(self.loader.context("eth_getCode", Some(address), None)),
                source,
            })?;
        self.observations.insert(address, observation);
        self.loader.observe_world(&self.world);
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
                reason: super::ConfigurationReason::StorageAccountMissing,
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
        self.loader.control.observer().phase(Phase::Acquiring);
        self.loader.check_chain()?;
        self.loader.check_storage_block(block_number)?;
        let mut pending = BTreeMap::new();
        for key in missing {
            pending.insert(key, self.loader.storage(address, key)?);
        }
        self.loader.check_chain()?;
        self.loader.check_storage_block(block_number)?;
        let installed = pending.keys().copied().collect();
        self.world
            .install_storage(address, &pending)
            .map_err(|source| RpcError::World {
                context: Box::new(self.loader.context("eth_getStorageAt", Some(address), None)),
                source,
            })?;
        if let Some(account) = self.observations.get_mut(&address) {
            if pending.values().any(|value| *value != U256::ZERO) {
                account.existence = Existence::Present;
            }
            account.storage.extend(pending);
        }
        self.loader.observe_world(&self.world);
        Ok(installed)
    }

    /// Number of HTTP requests attempted, including checks and failures.
    pub fn requests(&self) -> usize {
        self.loader.client.requests()
    }

    /// Finish acquisition, retaining the observations without the RPC client.
    pub fn into_world(self) -> EvmWorld<N> {
        self.world
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, configured_loader};
    use alloy_primitives::B256;
    use std::collections::BTreeSet;
    use {
        crate::Address, crate::Fork, crate::U256, crate::world::rpc::RpcInput,
        crate::world::test_support::TestAccount,
    };

    #[test]
    fn complete_and_absent_storage_need_no_requests_or_extra_observations() {
        for account in [TestAccount::empty(), TestAccount::absent()] {
            let mut loader =
                configured_loader(&RpcInput::new("http://127.0.0.1:1", Fork::Osaka)).unwrap();
            loader.chain_id = Some(U256::from(1));
            loader.block_hash = Some(B256::repeat_byte(0x11));
            let mut world = loader.world();
            let address = Address::repeat_byte(0x22);
            world.insert(address, account.clone()).unwrap();
            let mut session = Session {
                loader,
                world,
                observations: Default::default(),
            };
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
