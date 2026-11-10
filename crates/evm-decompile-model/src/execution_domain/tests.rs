use super::{
    ETHEREUM_RETURN_DATA_CEILING, ETHEREUM_TRANSACTION_GAS_CEILING, ExecutionDomain, memory_gas,
};
use crate::{Fork, U256};

#[test]
fn only_the_explicit_transaction_domain_supplies_a_length_bound() {
    assert_eq!(ExecutionDomain::default(), ExecutionDomain::Generic);
    assert!(ExecutionDomain::Generic.is_generic());
    assert!(!ExecutionDomain::EthereumTransactions.is_generic());
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        assert_eq!(ExecutionDomain::Generic.return_data_size_bound(fork), None);
        assert_eq!(
            ExecutionDomain::EthereumTransactions.return_data_size_bound(fork),
            Some((U256::from(1) << 41) - U256::from(1)),
        );
    }
}

#[test]
fn quadratic_memory_cost_proves_the_common_byte_ceiling() {
    assert_eq!(ETHEREUM_RETURN_DATA_CEILING, 1_u64 << 41);
    let word_ceiling = ETHEREUM_RETURN_DATA_CEILING / 32;
    assert!(memory_gas(word_ceiling) > ETHEREUM_TRANSACTION_GAS_CEILING);
    // The selected power-of-two boundary is conservative, rather than a host
    // allocation threshold or an exact maximal memory length.
    assert!(memory_gas(word_ceiling / 2) <= ETHEREUM_TRANSACTION_GAS_CEILING);
    assert!(word_ceiling * 32 < u64::MAX);
}

#[test]
fn variable_length_precompile_outputs_cannot_cross_the_common_ceiling() {
    let output_length = u128::from(ETHEREUM_RETURN_DATA_CEILING);
    let words = output_length.div_ceil(8);
    // EIP-2565 uses max(base_length, modulus_length), at least one exponent
    // iteration, and floor(words^2 / 3). Larger lengths only increase the cost.
    let modexp_minimum_gas = words * words / 3;
    assert!(modexp_minimum_gas > ETHEREUM_TRANSACTION_GAS_CEILING);
    // Osaka rejects a modulus longer than 1024 bytes before producing output.
    const { assert!(1024 < ETHEREUM_RETURN_DATA_CEILING) };
    // IDENTITY receives its input from the caller's charged memory range.
    assert!(memory_gas(ETHEREUM_RETURN_DATA_CEILING / 32) > ETHEREUM_TRANSACTION_GAS_CEILING);
}

#[test]
fn fixed_size_precompile_families_fit_the_derived_bound() {
    let cancun = [32_u64, 32, 32, 64, 64, 32, 64, 64];
    let prague_bls = [128_u64, 128, 256, 256, 32, 128, 256];
    let osaka_p256 = [32_u64];
    assert_eq!(
        cancun.into_iter().chain(prague_bls).chain(osaka_p256).max(),
        Some(256)
    );
    for length in cancun.into_iter().chain(prague_bls).chain(osaka_p256) {
        assert!(length < ETHEREUM_RETURN_DATA_CEILING);
    }
}

#[test]
fn semantic_domain_transport_requires_an_exact_supported_string() {
    for domain in [
        ExecutionDomain::Generic,
        ExecutionDomain::EthereumTransactions,
    ] {
        let json = serde_json::to_string(&domain).unwrap();
        assert_eq!(
            serde_json::from_str::<ExecutionDomain>(&json).unwrap(),
            domain
        );
    }
    for json in [
        "null",
        "0",
        "[]",
        r#""ethereum""#,
        r#""ethereum-transactions""#,
        r#"{"ethereum_transactions":null}"#,
        r#"{"generic":null}"#,
    ] {
        assert!(
            serde_json::from_str::<ExecutionDomain>(json).is_err(),
            "{json}"
        );
    }
}
