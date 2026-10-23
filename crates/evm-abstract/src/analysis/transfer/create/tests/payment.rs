use super::super::test_support::{NoFactoryPrices, Policy, RECEIPT, address, arm, fixture, value};
use crate::{
    analysis::{
        FrontierReason, MachineEdgeKind,
        transfer::{create::create, empty_execution, testing::procedure_context as context},
    },
    domain::{DomainSpec, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn policy(target: U256, fee: usize) -> ValueAlgebra<Policy> {
    ValueAlgebra::new(Policy {
        inner: ProductAlgebra::from_spec(DomainSpec::default()),
        target,
        target_fee: fee,
        top_fee: 128,
        cancel: None,
    })
}

#[test]
fn paid_literal_array_and_creation_helpers_preserve_the_pure_facade_and_store() {
    use crate::{
        domain::{EvmValue, NumericValue, summary},
        world::{EvmAccount, EvmByteArray, EvmStore, EvmWorld},
    };
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let mut budget = WorkBudget::new(1_000_000);
    for word in [
        U256::ZERO,
        U256::ONE,
        U256::from_be_slice(address(0x101).create(0).as_slice()),
    ] {
        assert_eq!(
            summary::literal(word, domain.borrowed(), &mut budget).unwrap(),
            EvmValue::<NumericValue>::constant(word)
        );
    }
    assert_eq!(
        EvmByteArray::empty_with_payment(domain.borrowed(), &mut budget).unwrap(),
        EvmByteArray::<NumericValue>::empty()
    );
    assert_eq!(
        EvmByteArray::memory_with_payment(domain.borrowed(), &mut budget).unwrap(),
        EvmByteArray::<NumericValue>::memory()
    );
    let mut world = EvmWorld::new(crate::Fork::Osaka, "pure/paid creation equality");
    world
        .insert(address(0x404), EvmAccount::<NumericValue>::absent())
        .unwrap();
    let mut pure = EvmStore::new(&world, domain.borrowed());
    let mut paid = pure.clone();
    pure.begin_creation(address(0x404));
    paid.begin_creation_with_payment(address(0x404), domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(paid, pure);
}

#[test]
fn default_unknown_constructor_fee_refuses_before_native_literal_hook() {
    let _scope = RECEIPT.scope();
    let (world, config, _, payload) = fixture();
    let program = payload.active().program.clone().unwrap();
    let args = [value(0), value(0), value(0)];
    let mut result = empty_execution(payload);
    let domain = ValueAlgebra::new(NoFactoryPrices(ProductAlgebra::from_spec(
        DomainSpec::default(),
    )));
    let mut budget = WorkBudget::new(usize::MAX);
    arm(U256::from(0xdead), 64);
    create(
        &mut result,
        &world,
        opcode::CREATE,
        &args,
        &program,
        &mut context(&config, &mut budget, domain.borrowed()),
        0,
    );
    assert_eq!(RECEIPT.get().factories, 0);
    assert_eq!(RECEIPT.get().top_factories, 0);
    assert!(result.successors.is_empty() && result.outcomes.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn max_nonce_literal_fee_reaches_nonce_bump_and_refuses_its_factory() {
    let _scope = RECEIPT.scope();
    let (world, config, _, payload) = fixture();
    let program = payload.active().program.clone().unwrap();
    let args = [value(0), value(0), value(0)];
    let mut result = empty_execution(payload);
    let domain = policy(U256::ONE, usize::MAX);
    let mut budget = WorkBudget::new(usize::MAX);
    arm(U256::ONE, 64);
    create(
        &mut result,
        &world,
        opcode::CREATE,
        &args,
        &program,
        &mut context(&config, &mut budget, domain.borrowed()),
        0,
    );
    let receipt = RECEIPT.get();
    assert!(
        receipt.factories > 0,
        "earlier finite zero factories must actually execute"
    );
    assert_eq!((receipt.target_quotes, receipt.target_factories), (1, 0));
    assert!(result.successors.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(
        result.payload.store.nonce(address(0x101)).singleton(),
        Some(U256::ZERO)
    );
}

#[cfg(not(feature = "imbl"))]
#[test]
fn std_post_nonce_clone_quotes_actual_new_nonce_before_child_gas_copy() {
    let _scope = RECEIPT.scope();
    let (world, config, domain, payload) = fixture();
    let program = payload.active().program.clone().unwrap();
    let target = domain.numeric_algebra().target;
    let args = [value(0), value(0), value(0)];
    let mut result = empty_execution(payload);
    let mut budget = WorkBudget::new(usize::MAX);
    arm(target, usize::MAX);
    create(
        &mut result,
        &world,
        opcode::CREATE,
        &args,
        &program,
        &mut context(&config, &mut budget, domain.borrowed()),
        0,
    );
    let receipt = RECEIPT.get();
    assert_eq!(
        receipt.nonce_factories, 1,
        "the updated nonce must really exist"
    );
    assert_eq!(
        receipt.top_factories, 0,
        "native Body avoids unrelated root finalization factories"
    );
    assert_eq!(
        receipt.fresh_nonce_quotes, 1,
        "must quote the new payload at ChildGas copy"
    );
    assert_eq!(
        receipt.fresh_nonce_clones, 0,
        "MAX-priced new N must never be copied"
    );
    println!("native-post-nonce-MAX {receipt:?} work={}", budget.used());
    assert!(result.successors.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(
        result.payload.store.nonce(address(0x101)).singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn finite_new_nonce_clone_fee_is_paid_on_each_actual_copy_and_keeps_branches() {
    let _scope = RECEIPT.scope();
    let (world, config, domain, payload) = fixture();
    let program = payload.active().program.clone().unwrap();
    let args = [value(0), value(0), value(0)];
    let mut result = empty_execution(payload);
    let mut budget = WorkBudget::new(usize::MAX);
    arm(domain.numeric_algebra().target, 100_003);
    create(
        &mut result,
        &world,
        opcode::CREATE,
        &args,
        &program,
        &mut context(&config, &mut budget, domain.borrowed()),
        0,
    );
    let receipt = RECEIPT.get();
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    assert_eq!(receipt.top_factories, 0);
    assert!(
        result
            .successors
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Call)
    );
    assert!(
        result
            .successors
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Failure)
    );
    assert!(
        receipt.nonce_factories >= 2,
        "creator and new-account nonce must both be constructed"
    );
    // Three full-payload invoices cover these retained versions. std copies
    // each nonce eagerly; imbl shares the first versions and copies it once
    // when begin_creation updates the shared nonce map.
    assert_eq!(
        (receipt.fresh_nonce_quotes, receipt.fresh_nonce_clones),
        (3, if cfg!(feature = "imbl") { 1 } else { 3 })
    );
    assert!(budget.used() >= receipt.fresh_nonce_quotes * 100_003);
    println!(
        "native-post-nonce-finite {receipt:?} work={}",
        budget.used()
    );
}

#[test]
fn unobserved_nonce_default_top_refuses_before_its_unknown_native_hook() {
    let _scope = RECEIPT.scope();
    let (_, _, _, payload) = fixture();
    let owner = address(0xbeef);
    let domain = ValueAlgebra::new(Policy {
        inner: ProductAlgebra::from_spec(DomainSpec::default()),
        target: U256::from(0xdead),
        target_fee: 128,
        top_fee: usize::MAX,
        cancel: None,
    });
    let mut budget = WorkBudget::new(usize::MAX);
    arm(U256::from(0xdead), 64);
    assert_eq!(
        payload
            .store
            .nonce_with_payment(owner, domain.borrowed(), &mut budget),
        Err(crate::resource::CloneError::Work)
    );
    assert_eq!(
        (RECEIPT.get().top_quotes, RECEIPT.get().top_factories),
        (1, 0)
    );
}

#[test]
fn begin_creation_refuses_before_state_change_when_actual_nonce_factory_is_unpriced() {
    let _scope = RECEIPT.scope();
    let (_, _, _, payload) = fixture();
    let mut store = payload.store;
    let expected = store.clone();
    let domain = policy(U256::ONE, usize::MAX);
    let mut budget = WorkBudget::new(usize::MAX);
    arm(U256::ONE, 64);
    assert_eq!(
        store.begin_creation_with_payment(address(0x404), domain.borrowed(), &mut budget),
        Err(crate::resource::CloneError::Work)
    );
    assert_eq!(
        (RECEIPT.get().target_quotes, RECEIPT.get().target_factories),
        (1, 0)
    );
    assert_eq!(
        RECEIPT.get().factories,
        2,
        "only the separately paid plane zeros were staged"
    );
    assert_eq!(store, expected);
}
