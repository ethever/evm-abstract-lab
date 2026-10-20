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
    let code = hex::decode("60205f5f5f5f6102015af15060205ff3").unwrap();
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
    for outcome in returns {
        assert_eq!(
            outcome
                .data
                .read_word_with(&word(0), algebra.borrowed())
                .singleton(),
            Some(U256::from(42))
        );
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
