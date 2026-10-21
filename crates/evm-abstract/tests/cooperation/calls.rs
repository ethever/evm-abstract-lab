use super::{
    fixtures::*,
    policy::{Calls, Policy},
};
use crate::native;
use alloy_primitives::{U256, hex};
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::{EnginePolicies, ExecutionConfig, OutcomeKind, Status},
    domain::{EvmValue, ValueAlgebra},
    world::{AddressInput, EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};

#[test]
fn semantic_singleton_without_finite_constants_resolves_call_and_keeps_code_owner_context() {
    // Keep CALL's status at offset64 instead of POP: a rejected/failed call
    // legitimately leaves the initially zero output area unchanged.
    let code = hex::decode("60205f5f5f5f6102015af160405260605ff3").unwrap();
    let mut world = world(&code);
    let mut callee = EvmAccount::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap();
    callee.balance = word(1_000_000);
    world.insert(address(CALLEE), callee).unwrap();
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension).run(
            AnalysisInput::new(world, entry()).unwrap(),
            ExecutionConfig {
                max_work: 1_000_000_000,
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        result
            .states()
            .iter()
            .any(|state| state.active().code_address == address(CALLEE)
                && state.active().address == address(CALLEE)
                && state.active().caller == AddressInput::Concrete(address(OWNER)))
    );
    let returns: Vec<_> = result
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    let mut successes = 0;
    let mut failures = 0;
    for outcome in &returns {
        let status = outcome
            .data
            .read_word(&word(64), algebra.borrowed())
            .singleton();
        let data = outcome
            .data
            .read_word(&word(0), algebra.borrowed())
            .singleton();
        eprintln!("CALL joint result: status={status:?}, output={data:?}");
        match status {
            Some(status) if status == U256::ONE => {
                successes += 1;
                assert_eq!(
                    data,
                    Some(U256::from(42)),
                    "successful known CALL must retain its data"
                );
            }
            Some(status) if status == U256::ZERO => {
                failures += 1;
                assert_eq!(
                    data,
                    Some(U256::ZERO),
                    "failed CALL has not written the output area"
                );
            }
            other => {
                panic!("known CALL must keep return data paired with its resume status: {other:?}")
            }
        }
    }
    assert!(
        successes > 0,
        "no successful42 path is a production regression"
    );
    assert!(
        failures > 0,
        "unknown GAS includes an insufficient-gas alternative"
    );
    super::oracle::assert_call_status_witnesses(&code, &hex::decode("602a5f5260205ff3").unwrap());
    for outcome in returns {
        assert!(
            !outcome
                .store
                .read(address(OWNER), &word(0), algebra.borrowed())
                .contains(U256::from(42))
        );
    }
}

#[test]
fn nonexact_private_call_output_size_keeps_both_concrete_results() {
    // CALLVALUE is the actual requested output extent, independently 0..=32.
    let code = hex::decode("345f5f5f5f6102015af160405260605ff3").unwrap();
    let mut world = world(&code);
    world
        .insert(
            address(CALLEE),
            EvmAccount::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let mut entry = entry();
    entry.environment.value = evm_abstract::domain::EvmValue::from_numeric(
        super::carrier::ExcludingWord::range(U256::ZERO, U256::from(32)),
    );
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension).run(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: 1_000_000_000,
                root_terminal_coverage: false,
                ..ExecutionConfig::default()
            },
            &Control::default(),
        )
    }
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    let mut success = false;
    for outcome in result
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
    {
        let status = outcome
            .data
            .read_word(&word(64), algebra.borrowed())
            .singleton();
        let data = outcome.data.read_word(&word(0), algebra.borrowed());
        if status == Some(U256::ONE) {
            success = true;
            assert!(
                data.contains(U256::ZERO),
                "zero requested bytes leaves memory zero"
            );
            assert!(
                data.contains(U256::from(42)),
                "32 requested bytes copies the complete word"
            );
        }
    }
    assert!(
        success,
        "the actual successful CALL/resume path must remain"
    );
}

#[test]
fn existing_default_entry_and_memory_wrapper_still_execute_the_same_code() {
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "default wrapper comparison");
    world
        .insert(
            address(OWNER),
            EvmAccount::<NumericValue>::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::<NumericValue>::concrete(
        address(OWNER),
        address(0x900),
        EvmValue::<NumericValue>::constant(U256::ZERO),
        EvmByteArray::<NumericValue>::empty(),
    );
    let result = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig {
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
        &Control::default(),
    )
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    let returns: Vec<_> = result
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(
            outcome
                .data
                .read_word(
                    &EvmValue::<NumericValue>::constant(U256::ZERO),
                    native::product().borrowed()
                )
                .singleton(),
            Some(U256::from(42))
        );
    }
}
