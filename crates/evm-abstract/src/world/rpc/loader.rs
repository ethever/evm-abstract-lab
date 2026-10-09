//! One synchronous owner drives a private asynchronous HTTP reactor. Dropping
//! a cancelled request/body future closes that operation before acquisition
//! can install its observations; no detached per-request worker is created.

use super::{
    AccountRequest, RpcBlock, RpcContext, RpcError, RpcInput, context, invalid,
    wire::{Data, FeeHistory, Header, Method, Params, Quantity, Selector},
};
use super::{HeaderField, HeaderValue, ResponseReason};
use crate::{
    Address, U256,
    analysis::{
        control::Control,
        progress::{Event, Phase},
    },
    domain::AbstractValue,
    world::{Account, Existence, SnapshotEnvironment, World},
};
use alloy_primitives::{B256, hex};
use reqwest::Client;
mod transport;

#[derive(Debug)]
pub(super) struct Loader {
    pub input: RpcInput,
    pub client: Client,
    pub runtime: Option<tokio::runtime::Runtime>,
    pub control: Control,
    pub next_id: usize,
    pub chain_id: Option<U256>,
    pub block_hash: Option<B256>,
    pub block_number: Option<U256>,
    pub header: Option<Header>,
    pub environment: Option<SnapshotEnvironment>,
    pub round: usize,
    pub acquired_accounts: usize,
    pub acquired_slots: usize,
}

impl Loader {
    pub(super) fn context(
        &self,
        method: &'static str,
        account: Option<Address>,
        slot: Option<U256>,
    ) -> RpcContext {
        let mut context = context(&self.input, method, account, slot);
        context.chain_id = self.chain_id;
        context.block_hash = self.block_hash.or(context.block_hash);
        context
    }

    fn selector(&self) -> Selector {
        Selector {
            block_hash: self.block_hash.expect("pinning precedes state requests"),
            require_canonical: true,
        }
    }

    pub(super) fn world(&self) -> World {
        let mut world = World::anchored(
            self.input.fork,
            self.chain_id.expect("world follows pinning"),
            self.block_hash.expect("world follows pinning"),
            "explicit-rpc",
        );
        world.snapshot_environment = self.environment.clone();
        world
    }

    pub(super) fn observe_world(&mut self, world: &World) {
        self.acquired_accounts = world.accounts().len();
        self.acquired_slots = world
            .accounts()
            .values()
            .map(|account| account.storage.len())
            .sum();
        self.progress();
    }

    pub(super) fn progress(&self) {
        self.control.observer().emit(Event::Acquisition {
            round: self.round,
            accounts: self.acquired_accounts,
            slots: self.acquired_slots,
            requests: self.next_id,
        });
    }

    pub(super) fn pin(&mut self) -> Result<(), RpcError> {
        self.control.observer().phase(Phase::Pinning);
        self.check_chain()?;
        let (method, params) = match self.input.block {
            RpcBlock::Latest => (Method::BlockByNumber, Params::Latest("latest", false)),
            RpcBlock::Number(number) => (
                Method::BlockByNumber,
                Params::Number(Quantity(U256::from(number)), false),
            ),
            RpcBlock::Hash(hash) => (Method::BlockByHash, Params::Hash(hash, false)),
        };
        let context = self.context(method.name(), None, None);
        let header: Header = self.call(&context, method, params)?;
        if let RpcBlock::Hash(expected) = self.input.block
            && header.hash.0 != expected
        {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context),
                observed: header.hash.0,
            });
        }
        if let RpcBlock::Number(expected) = self.input.block
            && header.number.0 != U256::from(expected)
        {
            return Err(invalid(
                &context,
                ResponseReason::BlockNumber {
                    expected: U256::from(expected),
                    observed: header.number.0,
                },
            ));
        }
        if header.excess_blob_gas.is_some() != header.blob_gas_used.is_some() {
            return Err(invalid(
                &context,
                ResponseReason::BlobHeaderFields {
                    excess_blob_gas: header.excess_blob_gas.is_some(),
                    blob_gas_used: header.blob_gas_used.is_some(),
                },
            ));
        }
        self.block_hash = Some(header.hash.0);
        self.block_number = Some(header.number.0);
        self.header = Some(header.clone());
        let blob_base_fee = if header.excess_blob_gas.is_some() {
            Some(self.blob_fee(header.number.0)?)
        } else {
            None
        };
        self.environment = Some(SnapshotEnvironment {
            number: header.number.0,
            parent_hash: header.parent_hash.0,
            timestamp: header.timestamp.0,
            coinbase: header.miner.0,
            prevrandao: header.mix_hash.0,
            gas_limit: header.gas_limit.0,
            base_fee: header.base_fee.map(|value| value.0),
            blob_base_fee,
            excess_blob_gas: header.excess_blob_gas.map(|value| value.0),
            blob_gas_used: header.blob_gas_used.map(|value| value.0),
        });
        Ok(())
    }

    fn blob_fee(&mut self, number: U256) -> Result<U256, RpcError> {
        // Fee history uses a block number, not an EIP-1898 hash selector. Check
        // canonical number->hash on both sides, never re-resolve a moving tag.
        // This obtains the chain's real blob schedule rather than guessing a
        // BPO update fraction from the VM fork name alone.
        self.check_chain()?;
        self.check_storage_block(number)?;
        let context = self.context(Method::FeeHistory.name(), None, None);
        let history: FeeHistory = self.call(
            &context,
            Method::FeeHistory,
            Params::FeeHistory(Quantity(U256::from(1)), Quantity(number), []),
        )?;
        if history.oldest_block.0 != number || history.base_fee_per_blob_gas.len() != 2 {
            return Err(invalid(
                &context,
                ResponseReason::FeeHistory {
                    expected: number,
                    observed: history.oldest_block.0,
                    fees: history.base_fee_per_blob_gas.len(),
                },
            ));
        }
        self.check_chain()?;
        self.check_storage_block(number)?;
        Ok(history.base_fee_per_blob_gas[0].0)
    }

    pub(super) fn check_chain(&mut self) -> Result<(), RpcError> {
        let context = self.context(Method::ChainId.name(), None, None);
        let observed: Quantity = self.call(&context, Method::ChainId, Params::Empty([]))?;
        if let Some(expected) = self.chain_id {
            if observed.0 != expected {
                return Err(RpcError::ChainMismatch {
                    context: Box::new(context),
                    observed: observed.0,
                });
            }
        } else {
            self.chain_id = Some(observed.0);
        }
        Ok(())
    }

    fn verify_header(&self, context: &RpcContext, header: &Header) -> Result<(), RpcError> {
        if Some(header.hash.0) != self.block_hash {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context.clone()),
                observed: header.hash.0,
            });
        }
        if let Some(initial) = &self.header
            && let Some(reason) = header_change(initial, header)
        {
            return Err(invalid(context, reason));
        }
        Ok(())
    }

    pub(super) fn check_block(&mut self) -> Result<(), RpcError> {
        let hash = self.block_hash.expect("block checks follow pinning");
        let context = self.context(Method::BlockByHash.name(), None, None);
        let header: Header = self.call(&context, Method::BlockByHash, Params::Hash(hash, false))?;
        self.verify_header(&context, &header)
    }

    pub(super) fn storage_block_number(&self) -> Result<U256, RpcError> {
        self.block_number.ok_or_else(|| {
            invalid(
                &self.context(Method::BlockByHash.name(), None, None),
                ResponseReason::MissingBlockNumber,
            )
        })
    }

    pub(super) fn check_storage_block(&mut self, number: U256) -> Result<(), RpcError> {
        let context = self.context(Method::BlockByNumber.name(), None, None);
        let header: Header = self.call(
            &context,
            Method::BlockByNumber,
            Params::Number(Quantity(number), false),
        )?;
        self.verify_header(&context, &header)?;
        if header.number.0 != number {
            return Err(invalid(
                &context,
                ResponseReason::BlockNumber {
                    expected: number,
                    observed: header.number.0,
                },
            ));
        }
        Ok(())
    }

    pub(super) fn account(
        &mut self,
        world: &mut World,
        request: &AccountRequest,
    ) -> Result<(), RpcError> {
        let address = request.address;
        let code_context = self.context(Method::Code.name(), Some(address), None);
        let code: Data = self.call(
            &code_context,
            Method::Code,
            Params::Account(address, self.selector()),
        )?;
        let mut account =
            Account::from_hex(&hex::encode(code.0), self.input.fork).map_err(|source| {
                RpcError::Code {
                    context: Box::new(code_context.clone()),
                    source,
                }
            })?;
        account.storage_unknown = true;
        let context = self.context(Method::Balance.name(), Some(address), None);
        let balance: Quantity = self.call(
            &context,
            Method::Balance,
            Params::Account(address, self.selector()),
        )?;
        account.balance = AbstractValue::constant(balance.0);
        let context = self.context(Method::Nonce.name(), Some(address), None);
        let nonce: Quantity = self.call(
            &context,
            Method::Nonce,
            Params::Account(address, self.selector()),
        )?;
        account.nonce = AbstractValue::constant(nonce.0);
        account.existence = if account.code != crate::world::Code::Empty
            || balance.0 != U256::ZERO
            || nonce.0 != U256::ZERO
        {
            Existence::Present
        } else {
            Existence::Unknown
        };
        for &key in &request.slots {
            let observed = self.storage(address, key)?;
            if observed != U256::ZERO {
                account.existence = Existence::Present;
            }
            account
                .storage
                .insert(key, AbstractValue::constant(observed));
        }
        world
            .insert(address, account)
            .map_err(|source| RpcError::World {
                context: Box::new(code_context),
                source,
            })?;
        Ok(())
    }

    pub(super) fn storage(&mut self, address: Address, key: U256) -> Result<U256, RpcError> {
        let context = self.context(Method::Storage.name(), Some(address), Some(key));
        let data: Data = self.call(
            &context,
            Method::Storage,
            Params::Storage(address, Quantity(key), self.selector()),
        )?;
        if data.0.len() != 32 {
            return Err(invalid(
                &context,
                ResponseReason::StorageLength {
                    expected: 32,
                    observed: data.0.len(),
                },
            ));
        }
        Ok(U256::from_be_slice(&data.0))
    }
}

fn header_change(initial: &Header, observed: &Header) -> Option<ResponseReason> {
    let optional = |quantity: Option<Quantity>| {
        quantity.map_or(HeaderValue::Absent, |value| HeaderValue::Quantity(value.0))
    };
    for (field, expected, observed) in [
        (
            HeaderField::ParentHash,
            HeaderValue::Hash(initial.parent_hash.0),
            HeaderValue::Hash(observed.parent_hash.0),
        ),
        (
            HeaderField::Number,
            HeaderValue::Quantity(initial.number.0),
            HeaderValue::Quantity(observed.number.0),
        ),
        (
            HeaderField::Timestamp,
            HeaderValue::Quantity(initial.timestamp.0),
            HeaderValue::Quantity(observed.timestamp.0),
        ),
        (
            HeaderField::Miner,
            HeaderValue::Address(initial.miner.0),
            HeaderValue::Address(observed.miner.0),
        ),
        (
            HeaderField::MixHash,
            HeaderValue::Hash(initial.mix_hash.0),
            HeaderValue::Hash(observed.mix_hash.0),
        ),
        (
            HeaderField::GasLimit,
            HeaderValue::Quantity(initial.gas_limit.0),
            HeaderValue::Quantity(observed.gas_limit.0),
        ),
        (
            HeaderField::BaseFee,
            optional(initial.base_fee),
            optional(observed.base_fee),
        ),
        (
            HeaderField::ExcessBlobGas,
            optional(initial.excess_blob_gas),
            optional(observed.excess_blob_gas),
        ),
        (
            HeaderField::BlobGasUsed,
            optional(initial.blob_gas_used),
            optional(observed.blob_gas_used),
        ),
    ] {
        if expected != observed {
            return Some(ResponseReason::HeaderChanged {
                field,
                expected,
                observed,
            });
        }
    }
    None
}
