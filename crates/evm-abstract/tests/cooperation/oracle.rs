//! Independent concrete completion of the custom abstract fixture.
use super::{
    fixtures,
    policy::{Calls, Policy},
};
use crate::native;
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
use evm_abstract::domain::NumericValue;

// Reuse revm execution/journaling only. These support constructors are concrete
// oracle inputs; they never convert custom abstract execution into the default domain.
mod support {
    pub fn address(value: u64) -> alloy_primitives::Address {
        super::fixtures::address(value)
    }
    pub fn word(value: u64) -> evm_abstract::domain::EvmValue<evm_abstract::domain::NumericValue> {
        evm_abstract::domain::EvmValue::<evm_abstract::domain::NumericValue>::constant(
            alloy_primitives::U256::from(value),
        )
    }
}
#[path = "../known_recursion/oracle.rs"]
mod concrete;

fn concrete_input(
    code: &[u8],
    callee: Option<&[u8]>,
) -> (
    evm_abstract::world::EvmWorld<NumericValue>,
    evm_abstract::world::EvmEntry<NumericValue>,
) {
    use evm_abstract::{
        Fork,
        world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
    };
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "downstream concrete witness");
    for (owner, code) in [(fixtures::OWNER, Some(code)), (fixtures::CALLEE, callee)] {
        let Some(code) = code else {
            continue;
        };
        let mut account =
            EvmAccount::<NumericValue>::from_hex(&alloy_primitives::hex::encode(code), Fork::Osaka)
                .unwrap();
        account.balance = support::word(1_000_000);
        account.storage_unknown = false;
        if owner == fixtures::OWNER {
            account
                .storage
                .insert(alloy_primitives::U256::ZERO, support::word(41));
        }
        world.insert(support::address(owner), account).unwrap();
    }
    let entry = EvmEntry::<NumericValue>::concrete(
        support::address(fixtures::OWNER),
        support::address(0x900),
        support::word(0),
        EvmByteArray::<NumericValue>::empty(),
    );
    (world, entry)
}

pub fn assert_call_status_witnesses(code: &[u8], callee: &[u8]) {
    use alloy_primitives::U256;
    use evm_abstract::analysis::{OutcomeKind, Status};
    let (world, entry) = concrete_input(code, Some(callee));
    let success = concrete::execute(&world, &entry);
    assert_eq!(success.kind, OutcomeKind::Return);
    assert_eq!(success.output_word(64), U256::ONE);
    assert_eq!(success.output_word(0), U256::from(42));
    assert!(success.steps.iter().any(|s| s.depth == 2
        && s.code == support::address(fixtures::CALLEE)
        && s.opcode == revm_bytecode::opcode::RETURN));
    // Zero gas forwarded into a nonempty callee fails locally, while the root
    // has enough transaction gas to observe status0 and its untouched memory.
    let mut failed_code = code.to_vec();
    let gas = failed_code
        .iter()
        .position(|op| *op == revm_bytecode::opcode::GAS)
        .unwrap();
    failed_code[gas] = revm_bytecode::opcode::PUSH0;
    let (failed_world, failed_entry) = concrete_input(&failed_code, Some(callee));
    let failure = concrete::execute(&failed_world, &failed_entry);
    assert_eq!(failure.kind, OutcomeKind::Return);
    assert_eq!(failure.output_word(64), U256::ZERO);
    assert_eq!(failure.output_word(0), U256::ZERO);
    eprintln!(
        "revm CALL witnesses: forwarded GAS -> (status1,data42); forwarded0 -> (status0,data0)"
    );
    let result = native::run(
        &native::product(),
        AnalysisInput::new(world, entry).unwrap(),
        fixtures::config(1_000_000_000),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(result.status(), Status::Converged);
    success.covers(&result);
    failure.covers(&result);
    concrete::unchanged(&result, &[fixtures::OWNER, fixtures::CALLEE]);
}

pub fn assert_guard_coverage(
    code: &[u8],
    result: &evm_abstract::analysis::EvmWorldAnalysis<super::carrier::ExcludingWord>,
    algebra: &evm_abstract::domain::ValueAlgebra<Policy<'_>>,
) {
    use alloy_primitives::U256;
    let (world, entry) = concrete_input(code, None);
    let witness = concrete::execute(&world, &entry);
    assert_eq!(witness.kind, evm_abstract::analysis::OutcomeKind::Return);
    assert_eq!(witness.output_word(0), U256::ONE);
    assert!(witness.logs.is_empty());
    assert!(
        result.outcomes().iter().any(|o| {
            o.kind == witness.kind
                && o.data.len().contains(U256::from(witness.data.len()))
                && witness.data.iter().enumerate().all(|(i, b)| {
                    o.data
                        .byte_at(i, algebra.borrowed())
                        .contains(U256::from(*b))
                })
                && witness.storage.iter().all(|((owner, slot), v)| {
                    o.store
                        .read(
                            *owner,
                            &evm_abstract::domain::EvmValue::constant(*slot),
                            algebra.borrowed(),
                        )
                        .contains(*v)
                })
                && witness
                    .balances
                    .iter()
                    .all(|(owner, v)| o.store.read_balance(*owner).contains(*v))
                && o.store.possible_logs().is_empty()
        }),
        "one joint custom outcome must cover the concrete data, storage, balances and logs"
    );
}

#[test]
fn one_custom_outcome_covers_the_concrete_parent_and_rolled_back_delegate_subtree() {
    use alloy_primitives::{U256, hex};
    use evm_abstract::{
        Fork,
        analysis::{EnginePolicies, OutcomeKind, Status},
        domain::ValueAlgebra,
        world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
    };
    let fixture = fixtures::rollback();
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "custom-carrier concrete witness");
    for (owner, code) in [
        (fixtures::OWNER, &fixture.root),
        (fixtures::CALLEE, &fixture.child),
    ] {
        let mut account =
            EvmAccount::<NumericValue>::from_hex(&hex::encode(code), Fork::Osaka).unwrap();
        account.balance = support::word(1_000_000);
        account.storage_unknown = false;
        if owner == fixtures::OWNER {
            account.storage.insert(U256::ZERO, support::word(41));
        }
        world.insert(support::address(owner), account).unwrap();
    }
    let input = EvmEntry::<NumericValue>::concrete(
        support::address(fixtures::OWNER),
        support::address(0x900),
        support::word(0),
        EvmByteArray::<NumericValue>::empty(),
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

    let default_result = native::run(
        &native::product(),
        AnalysisInput::new(world, input).unwrap(),
        fixtures::config(1_000_000_000),
        &Control::default(),
    )
    .unwrap();
    assert_eq!(default_result.status(), Status::Converged);
    witness.covers(&default_result);
    let calls = Calls::default();
    let algebra = ValueAlgebra::new(Policy::new(&calls));
    let result = {
        let mut extension = evm_abstract::analysis::cooperation::NoExtension;
        AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension).run(
            AnalysisInput::new(fixtures::rollback_world(&fixture), fixtures::entry()).unwrap(),
            fixtures::config(1_000_000_000),
            &Control::default(),
        )
    }
    .unwrap();
    assert!(
        result.outcomes().iter().any(|outcome| {
            outcome.kind == witness.kind
                && outcome
                    .data
                    .read_word(&fixtures::word(0), algebra.borrowed())
                    .singleton()
                    == Some(U256::ONE)
                && outcome
                    .store
                    .read(
                        fixtures::address(fixtures::OWNER),
                        &fixtures::word(0),
                        algebra.borrowed(),
                    )
                    .contains(U256::from(41))
                && outcome
                    .store
                    .read(
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
