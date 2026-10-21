//! Shared small machine fixtures for join and certificate regressions.

#[cfg(test)]
use crate::analysis::testing::{Account, ByteArray, Entry, EvmEnvironment, TestValue, World};
use crate::domain::EvmNumericAlgebra as _;

use crate::domain::ValueAlgebra;

use super::{ExecutionConfig, MachinePayload};
use crate::{
    Fork,
    analysis::{self, Config, WorldAnalysis, transfer},
    domain::{ProductAlgebra, relational::RelationLimits},
};
use alloy_primitives::{Address, U256};

pub(in crate::analysis) fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

pub(in crate::analysis) fn word(number: u64) -> TestValue {
    TestValue::constant(U256::from(number))
}

pub(in crate::analysis) fn config() -> ExecutionConfig {
    ExecutionConfig {
        analysis: Config {
            context_depth: 0,
            max_states: 256,
            max_transfers: 1024,
            ..Config::default()
        },
        max_work: 100_000_000,
        max_memory_bytes: 4096,
        use_summaries: false,
        ..ExecutionConfig::default()
    }
}

pub(in crate::analysis) fn fixture(accounts: &[(u64, &str)]) -> (World, Entry) {
    let mut world = World::new(Fork::Osaka, "machine-state regression");
    for (number, code) in accounts {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = word(1_000_000);
        world.insert(address(*number), account).unwrap();
    }
    let entry = Entry {
        address: address(0x101),
        environment: EvmEnvironment {
            to: address(0x101).into(),
            caller: address(0x900).into(),
            value: word(0),
            calldata: ByteArray::empty(),
            ..EvmEnvironment::default()
        },
    };
    (world, entry)
}

pub(in crate::analysis) fn payload(
    domain: crate::domain::AlgebraRef<'_, ProductAlgebra>,
) -> MachinePayload {
    let (world, entry) = fixture(&[(0x101, "00")]);
    transfer::initial(&world, &entry, domain).unwrap()
}

pub(in crate::analysis) fn run(
    code: &str,
    config: ExecutionConfig,
    spec: crate::domain::DomainSpec,
) -> WorldAnalysis {
    let (world, entry) = fixture(&[(0x101, code)]);
    crate::analysis::testing::execute(world, entry, config, spec).unwrap()
}
