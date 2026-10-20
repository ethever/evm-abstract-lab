//! Independent concrete completion of the custom abstract fixture.
use super::{
    fixtures,
    policy::{Calls, Policy},
};

// Reuse revm execution/journaling only. These support constructors are concrete
// oracle inputs; they never convert custom abstract execution into the default domain.
mod support {
    pub fn address(value: u64) -> alloy_primitives::Address {
        super::fixtures::address(value)
    }
    pub fn word(value: u64) -> evm_abstract::domain::AbstractValue {
        evm_abstract::domain::AbstractValue::constant(alloy_primitives::U256::from(value))
    }
}
#[path = "../known_recursion/oracle.rs"]
mod concrete;

#[test]
fn one_custom_outcome_covers_the_concrete_parent_and_rolled_back_delegate_subtree() {
    use alloy_primitives::{U256, hex};
    use evm_abstract::{
        Fork,
        analysis::{
            EnginePolicies, ExecutionConfig, OutcomeKind, Status, analyze_world, analyze_world_with,
        },
        domain::ValueAlgebra,
        world::{Account, ByteArray, Entry, World},
    };
    let fixture = fixtures::rollback();
    let mut world = World::new(Fork::Osaka, "custom-carrier concrete witness");
    for (owner, code) in [
        (fixtures::OWNER, &fixture.root),
        (fixtures::CALLEE, &fixture.child),
    ] {
        let mut account = Account::from_hex(&hex::encode(code), Fork::Osaka).unwrap();
        account.balance = support::word(1_000_000);
        account.storage_unknown = false;
        if owner == fixtures::OWNER {
            account.storage.insert(U256::ZERO, support::word(41));
        }
        world.insert(support::address(owner), account).unwrap();
    }
    let input = Entry::concrete(
        support::address(fixtures::OWNER),
        support::address(0x900),
        support::word(0),
        ByteArray::empty(),
    );
    let witness = concrete::execute(&world, &input);
    assert_eq!(witness.kind, OutcomeKind::Return);
    assert_eq!(witness.output_word(0), U256::ONE);
    assert_eq!(witness.slot(fixtures::OWNER, 0), U256::from(41));
    assert_eq!(witness.slot(fixtures::OWNER, 1), U256::from(41));
    assert!(witness.logs.is_empty());
    assert!(
        !witness.emitted_logs.is_empty(),
        "the delegate body must actually emit before reverting"
    );
    assert_eq!(witness.depth_failures, 0);
    let write = witness
        .steps
        .iter()
        .find(|step| {
            step.code == support::address(fixtures::CALLEE) && step.storage_write.is_some()
        })
        .unwrap();
    assert_eq!(write.code, support::address(fixtures::CALLEE));
    assert_eq!(write.owner, support::address(fixtures::OWNER));
    assert_eq!(write.caller, support::address(0x900));
    assert!(!write.is_static);
    assert_eq!(write.depth, 2);
    assert_eq!(write.pc, 3);
    assert_eq!(write.opcode, revm_bytecode::opcode::SSTORE);
    assert_eq!(write.storage_write, Some((U256::ZERO, U256::from(42))));
    let default_result = analyze_world(
        world,
        input,
        ExecutionConfig {
            max_work: 1_000_000_000,
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(default_result.status(), Status::Converged);
    witness.covers(&default_result);
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let result = analyze_world_with(
        fixtures::rollback_world(&fixture),
        fixtures::entry(),
        ExecutionConfig {
            max_work: 1_000_000_000,
            root_terminal_coverage: false,
            ..ExecutionConfig::default()
        },
        &algebra,
        &EnginePolicies::default(),
    )
    .unwrap();
    assert!(
        result.outcomes().iter().any(|outcome| {
            outcome.kind == witness.kind
                && outcome
                    .data
                    .read_word_with(&fixtures::word(0), algebra.borrowed())
                    .singleton()
                    == Some(U256::ONE)
                && outcome
                    .store
                    .read_with(
                        fixtures::address(fixtures::OWNER),
                        &fixtures::word(0),
                        algebra.borrowed(),
                    )
                    .contains(U256::from(41))
                && outcome
                    .store
                    .read_with(
                        fixtures::address(fixtures::OWNER),
                        &fixtures::word(1),
                        algebra.borrowed(),
                    )
                    .contains(U256::from(41))
                && outcome.store.possible_logs().is_empty()
        }),
        "one terminal must cover return data and both parent slots with rolled-back logs"
    );
}
