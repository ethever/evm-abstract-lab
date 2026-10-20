use super::{
    fixtures::*,
    policy::{Calls, Policy},
};
use alloy_primitives::{U256, hex};
use evm_abstract::{
    Fork,
    analysis::{
        EnginePolicies, ExecutionConfig, OutcomeKind, Status, analyze_world, analyze_world_with,
    },
    domain::{AbstractValue, Domain, ValueAlgebra},
    world::{Account, AddressInput, ByteArray, Entry, EvmAccount, World},
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
    let result = analyze_world_with(
        world,
        entry(),
        ExecutionConfig {
            max_work: 1_000_000_000,
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
        &algebra,
        &EnginePolicies::default(),
    )
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
            .read_word_with(&word(64), algebra.borrowed())
            .singleton();
        let data = outcome
            .data
            .read_word_with(&word(0), algebra.borrowed())
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
                .read_with(address(OWNER), &word(0), algebra.borrowed())
                .contains(U256::from(42))
        );
    }
}

#[test]
fn existing_default_entry_and_memory_wrapper_still_execute_the_same_code() {
    let mut world = World::new(Fork::Osaka, "default wrapper comparison");
    world
        .insert(
            address(OWNER),
            Account::from_hex("602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = Entry::concrete(
        address(OWNER),
        address(0x900),
        AbstractValue::constant(U256::ZERO),
        ByteArray::empty(),
    );
    let result = analyze_world(
        world,
        entry,
        ExecutionConfig {
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
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
                .read_word(&AbstractValue::constant(U256::ZERO), Domain::default())
                .singleton(),
            Some(U256::from(42))
        );
    }
}
