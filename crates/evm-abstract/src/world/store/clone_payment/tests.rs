use super::super::{EvmLog, LogKey, LogSourceKind};
use super::*;
use crate::{
    U256,
    domain::{EvmValue, symbolic::ExprId},
    resource::clone_payment::tests::{RECEIPT, assert_unobserved, fixture, private_value, reset},
    world::EvmByteArray,
};
use alloy_primitives::B256;

#[test]
fn live_store_prices_every_owned_numeric_container_and_preserves_saved_source() {
    let _scope = RECEIPT.scope();
    let (world, entry, _, policy) = fixture("00");
    let owner = entry.address;
    let mut store = EvmStore::new(&world, policy.borrowed());
    let mut count = 0;
    for plane in [&mut store.persistent, &mut store.transient] {
        plane
            .slots
            .insert((owner, U256::ZERO), private_value(100_000));
        plane.defaults.insert(owner, private_value(100_000));
        plane
            .operands
            .insert(ExprId::fresh().unwrap(), private_value(100_000));
        plane.global_default = private_value(100_000);
        count += 4;
    }
    store.balances.insert(owner, private_value(100_000));
    store.nonces.insert(owner, private_value(100_000));
    store.balance_default = private_value(100_000);
    count += 3;
    let mut data = EvmByteArray::<crate::resource::clone_payment::tests::Word>::exact(&[1]);
    data.update_values(&mut |value| {
        let numeric = value.numeric().clone().with_predicate(100_000);
        *value = value.clone().with_numeric(numeric);
    });
    store.possible_logs.insert(
        LogKey {
            address: owner,
            code_address: owner,
            code_hash: B256::ZERO,
            source_kind: LogSourceKind::Runtime,
            pc: 0,
        },
        EvmLog {
            topics: vec![private_value(100_000)],
            data,
        },
    );
    count += 4;
    let original = store.clone();
    reset();
    let mut measured = WorkBudget::new(usize::MAX);
    store.pay_clone_with(&mut measured).unwrap();
    assert_unobserved();
    assert_eq!(RECEIPT.get().private_quotes, count);
    let fee = measured.used();
    assert!(fee > count * 100_000);
    reset();
    let mut refused = WorkBudget::new(fee - 1);
    assert_eq!(store.pay_clone_with(&mut refused), Err(CloneError::Work));
    assert_unobserved();
    assert!(refused.used() >= store.clone_prefix_work());
    assert_eq!(store, original);
    reset();
    let mut budget = WorkBudget::new(fee);
    store.pay_clone_with(&mut budget).unwrap();
    let copied = store.clone();
    // imbl shares the owner maps and log entries. The two operand BTreeMaps
    // and three inline defaults still own their numeric copies.
    let owned = if cfg!(feature = "imbl") { 2 + 3 } else { count };
    assert_eq!(RECEIPT.get().bytes, owned * 4096);
    assert_eq!(copied, original);
    assert!(copied.balance_default.same_identity(&store.balance_default));
    assert_eq!(budget.used(), fee);
    // The saved/live distinction is about the actual owned object, not its
    // values being cheap or its scalar keys being small constants.
    store.balances.insert(owner, EvmValue::constant(U256::ZERO));
    assert_eq!(copied, original);
}
