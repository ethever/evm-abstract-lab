//! One synchronous owner drives a private asynchronous HTTP reactor. Dropping
//! a cancelled request/body future closes that operation before acquisition
//! can install its observations; no detached per-request worker is created.

use super::framework::Client;
use super::session::AccountObservation;
use super::{AccountRequest, RpcBlock, RpcContext, RpcError, RpcInput, context, invalid};
use super::{HeaderField, HeaderValue, ResponseReason};
use crate::{
    Address, U256,
    analysis::{
        control::Control,
        progress::{Event, Phase},
    },
    domain::{EvmValue, WordElement},
    world::{EvmAccount, EvmWorld, Existence, SnapshotEnvironment},
};
use alloy::{
    primitives::Bytes,
    rpc::types::{BlockId, BlockNumberOrTag, FeeHistory, Header},
};
use alloy_primitives::{B256, hex};
mod transport;

#[derive(Debug)]
pub(super) struct Loader {
    pub input: RpcInput,
    pub client: Client,
    pub control: Control,
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

    fn selector(&self) -> BlockId {
        BlockId::hash_canonical(self.block_hash.expect("pinning precedes state requests"))
    }

    pub(super) fn world<N: WordElement>(&self) -> EvmWorld<N> {
        let mut world = EvmWorld::<N>::anchored(
            self.input.fork,
            self.chain_id.expect("world follows pinning"),
            self.block_hash.expect("world follows pinning"),
            "explicit-rpc",
        );
        world.snapshot_environment = self.environment.clone();
        world
    }

    pub(super) fn observe_world<N: WordElement>(&mut self, world: &EvmWorld<N>) {
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
            requests: self.client.requests(),
        });
    }

    pub(super) fn pin(&mut self) -> Result<(), RpcError> {
        self.control.observer().phase(Phase::Pinning);
        self.check_chain()?;
        let method = if matches!(self.input.block, RpcBlock::Hash(_)) {
            "eth_getBlockByHash"
        } else {
            "eth_getBlockByNumber"
        };
        let context = self.context(method, None, None);
        let header: Header = match self.input.block {
            RpcBlock::Latest => self.call(&context, (BlockNumberOrTag::Latest, false))?,
            RpcBlock::Number(number) => {
                self.call(&context, (BlockNumberOrTag::Number(number), false))?
            }
            RpcBlock::Hash(hash) => self.call(&context, (hash, false))?,
        };
        if let RpcBlock::Hash(expected) = self.input.block
            && header.hash != expected
        {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context),
                observed: header.hash,
            });
        }
        if let RpcBlock::Number(expected) = self.input.block
            && U256::from(header.number) != U256::from(expected)
        {
            return Err(invalid(
                &context,
                ResponseReason::BlockNumber {
                    expected: U256::from(expected),
                    observed: U256::from(header.number),
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
        self.block_hash = Some(header.hash);
        self.block_number = Some(U256::from(header.number));
        self.header = Some(header.clone());
        let blob_base_fee = if header.excess_blob_gas.is_some() {
            Some(self.blob_fee(U256::from(header.number))?)
        } else {
            None
        };
        self.environment = Some(SnapshotEnvironment {
            number: U256::from(header.number),
            parent_hash: header.parent_hash,
            timestamp: U256::from(header.timestamp),
            coinbase: header.beneficiary,
            prevrandao: header.mix_hash,
            gas_limit: U256::from(header.gas_limit),
            base_fee: header.base_fee_per_gas.map(U256::from),
            blob_base_fee,
            excess_blob_gas: header.excess_blob_gas.map(U256::from),
            blob_gas_used: header.blob_gas_used.map(U256::from),
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
        let context = self.context("eth_feeHistory", None, None);
        let history: FeeHistory = self.call(
            &context,
            (
                U256::from(1),
                BlockNumberOrTag::Number(u64::try_from(number).expect("SDK block height is u64")),
                [0.0_f64; 0],
            ),
        )?;
        if U256::from(history.oldest_block) != number || history.base_fee_per_blob_gas.len() != 2 {
            return Err(invalid(
                &context,
                ResponseReason::FeeHistory {
                    expected: number,
                    observed: U256::from(history.oldest_block),
                    fees: history.base_fee_per_blob_gas.len(),
                },
            ));
        }
        self.check_chain()?;
        self.check_storage_block(number)?;
        Ok(U256::from(history.base_fee_per_blob_gas[0]))
    }

    pub(super) fn check_chain(&mut self) -> Result<(), RpcError> {
        let context = self.context("eth_chainId", None, None);
        // Alloy omits zero-sized params. Keep an explicit empty positional
        // array for the endpoint-facing acquisition contract.
        let observed: U256 = self.call(&context, Vec::<()>::new())?;
        if let Some(expected) = self.chain_id {
            if observed != expected {
                return Err(RpcError::ChainMismatch {
                    context: Box::new(context),
                    observed,
                });
            }
        } else {
            self.chain_id = Some(observed);
        }
        Ok(())
    }

    fn verify_header(&self, context: &RpcContext, header: &Header) -> Result<(), RpcError> {
        if Some(header.hash) != self.block_hash {
            return Err(RpcError::BlockMismatch {
                context: Box::new(context.clone()),
                observed: header.hash,
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
        let context = self.context("eth_getBlockByHash", None, None);
        let header: Header = self.call(&context, (hash, false))?;
        self.verify_header(&context, &header)
    }

    pub(super) fn storage_block_number(&self) -> Result<U256, RpcError> {
        self.block_number.ok_or_else(|| {
            invalid(
                &self.context("eth_getBlockByHash", None, None),
                ResponseReason::MissingBlockNumber,
            )
        })
    }

    pub(super) fn check_storage_block(&mut self, number: U256) -> Result<(), RpcError> {
        let context = self.context("eth_getBlockByNumber", None, None);
        let header: Header = self.call(
            &context,
            (
                BlockNumberOrTag::Number(u64::try_from(number).expect("SDK block height is u64")),
                false,
            ),
        )?;
        self.verify_header(&context, &header)?;
        if U256::from(header.number) != number {
            return Err(invalid(
                &context,
                ResponseReason::BlockNumber {
                    expected: number,
                    observed: U256::from(header.number),
                },
            ));
        }
        Ok(())
    }

    pub(super) fn account<N: WordElement>(
        &mut self,
        world: &mut EvmWorld<N>,
        request: &AccountRequest,
    ) -> Result<AccountObservation, RpcError> {
        let address = request.address;
        let code_context = self.context("eth_getCode", Some(address), None);
        let code: Bytes = self.call(&code_context, (address, self.selector()))?;
        let mut account =
            EvmAccount::<N>::from_hex(&hex::encode(code), self.input.fork).map_err(|source| {
                RpcError::Code {
                    context: Box::new(code_context.clone()),
                    source,
                }
            })?;
        account.storage_unknown = true;
        let context = self.context("eth_getBalance", Some(address), None);
        let balance: U256 = self.call(&context, (address, self.selector()))?;
        account.balance = EvmValue::<N>::constant(balance);
        let context = self.context("eth_getTransactionCount", Some(address), None);
        let nonce: U256 = self.call(&context, (address, self.selector()))?;
        account.nonce = EvmValue::<N>::constant(nonce);
        account.existence = if account.code != crate::world::Code::Empty
            || balance != U256::ZERO
            || nonce != U256::ZERO
        {
            Existence::Present
        } else {
            Existence::Unknown
        };
        let mut storage = std::collections::BTreeMap::new();
        for &key in &request.slots {
            let observed = self.storage(address, key)?;
            storage.insert(key, observed);
            if observed != U256::ZERO {
                account.existence = Existence::Present;
            }
            account
                .storage
                .insert(key, EvmValue::<N>::constant(observed));
        }
        let observation = AccountObservation {
            code: account.code.clone(),
            balance,
            nonce,
            existence: account.existence,
            storage,
        };
        world
            .insert(address, account)
            .map_err(|source| RpcError::World {
                context: Box::new(code_context),
                source,
            })?;
        Ok(observation)
    }

    pub(super) fn storage(&mut self, address: Address, key: U256) -> Result<U256, RpcError> {
        let context = self.context("eth_getStorageAt", Some(address), Some(key));
        let data: Bytes = self.call(&context, (address, key, self.selector()))?;
        if data.len() != 32 {
            return Err(invalid(
                &context,
                ResponseReason::StorageLength {
                    expected: 32,
                    observed: data.len(),
                },
            ));
        }
        Ok(U256::from_be_slice(&data))
    }
}

fn header_change(initial: &Header, observed: &Header) -> Option<ResponseReason> {
    let optional = |quantity: Option<u64>| {
        quantity.map_or(HeaderValue::Absent, |value| {
            HeaderValue::Quantity(U256::from(value))
        })
    };
    for (field, expected, observed) in [
        (
            HeaderField::ParentHash,
            HeaderValue::Hash(initial.parent_hash),
            HeaderValue::Hash(observed.parent_hash),
        ),
        (
            HeaderField::Number,
            HeaderValue::Quantity(U256::from(initial.number)),
            HeaderValue::Quantity(U256::from(observed.number)),
        ),
        (
            HeaderField::Timestamp,
            HeaderValue::Quantity(U256::from(initial.timestamp)),
            HeaderValue::Quantity(U256::from(observed.timestamp)),
        ),
        (
            HeaderField::Miner,
            HeaderValue::Address(initial.beneficiary),
            HeaderValue::Address(observed.beneficiary),
        ),
        (
            HeaderField::MixHash,
            HeaderValue::Hash(initial.mix_hash),
            HeaderValue::Hash(observed.mix_hash),
        ),
        (
            HeaderField::GasLimit,
            HeaderValue::Quantity(U256::from(initial.gas_limit)),
            HeaderValue::Quantity(U256::from(observed.gas_limit)),
        ),
        (
            HeaderField::BaseFee,
            optional(initial.base_fee_per_gas),
            optional(observed.base_fee_per_gas),
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
