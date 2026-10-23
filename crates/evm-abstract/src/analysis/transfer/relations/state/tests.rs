//! Current read facts constrain only their original live Store source.

use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{address, fixture, word},
        testing::TestValue,
        transfer::{StorageReadPolicy, execute_inner, initial},
    },
    domain::{ProductAlgebra, ValueAlgebra, symbolic::ExprId},
    resource::WorkBudget,
};

#[test]
fn branch_projects_the_written_current_slot_without_retargeting_rollback_or_other_owners() {
    // Incoming stack supplies v. SSTORE(0,v); if SLOAD(0)<2 goto PC11.
    // The true current read is exactly1, false exactly2. The old checkpoint,
    // another owner, and transient storage have equal numeric bounds but
    // independent sources and must retain both values.
    let (world, entry) = fixture(&[(0x101, "5f5560025f5410600b57005b00"), (0x102, "00")]);
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    let config = ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut input = initial(&world, &entry, domain).unwrap();
    let fresh = || {
        TestValue::unsigned_range(crate::U256::ONE, crate::U256::from(2))
            .unwrap()
            .with_expression(ExprId::fresh().unwrap())
    };
    input
        .store
        .write_checked(address(0x101), &word(0), &fresh(), domain)
        .unwrap();
    input
        .store
        .write_checked(address(0x102), &word(0), &fresh(), domain)
        .unwrap();
    input
        .store
        .write_transient_checked(address(0x101), &word(0), &fresh(), domain)
        .unwrap();
    input.active_mut().saved_store = input.store.snapshot();
    let checkpoint = input.active().saved_store.clone();
    input.active_mut().stack = vec![fresh()];
    input.normalize();
    let execution = execute_inner(
        &world,
        &entry,
        input,
        &config,
        domain,
        &crate::analysis::engine::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
        false,
    );
    assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
    for (block, expected) in [(2, 1), (1, 2)] {
        let branch = execution
            .successors
            .iter()
            .find(|next| next.payload.active().key.basic_block_index == block)
            .unwrap();
        let read = branch
            .payload
            .store
            .read_checked(address(0x101), &word(0), domain)
            .unwrap();
        assert_eq!(read.singleton(), Some(crate::U256::from(expected)));
        assert_eq!(
            branch.payload.active().saved_store.state(),
            checkpoint.state()
        );
        for independent in [
            branch
                .payload
                .active()
                .saved_store
                .state()
                .read_checked(address(0x101), &word(0), domain)
                .unwrap(),
            branch
                .payload
                .store
                .read_checked(address(0x102), &word(0), domain)
                .unwrap(),
            branch
                .payload
                .store
                .read_transient_checked(address(0x101), &word(0), domain)
                .unwrap(),
        ] {
            assert!(independent.contains(crate::U256::ONE));
            assert!(independent.contains(crate::U256::from(2)));
        }
    }
}

#[test]
fn a_read_before_sstore_cannot_constrain_the_new_current_slot() {
    // Keep the old read on stack, write an independent v, then branch on old<2.
    let (world, entry) = fixture(&[(0x101, "5f54905f5560029010600d57005b00")]);
    let values = ValueAlgebra::new(ProductAlgebra::default());
    let domain = values.borrowed();
    let config = ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut input = initial(&world, &entry, domain).unwrap();
    let fresh = || {
        TestValue::unsigned_range(crate::U256::ONE, crate::U256::from(2))
            .unwrap()
            .with_expression(ExprId::fresh().unwrap())
    };
    input
        .store
        .write_checked(address(0x101), &word(0), &fresh(), domain)
        .unwrap();
    input.active_mut().saved_store = input.store.snapshot();
    let checkpoint = input.active().saved_store.clone();
    input.active_mut().stack = vec![fresh()];
    input.normalize();
    let execution = execute_inner(
        &world,
        &entry,
        input,
        &config,
        domain,
        &crate::analysis::engine::RecentDistinctJumps,
        &mut budget,
        StorageReadPolicy::Abstract,
        false,
    );
    assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
    assert_eq!(execution.successors.len(), 2);
    for branch in execution.successors {
        let read = branch
            .payload
            .store
            .read_checked(address(0x101), &word(0), domain)
            .unwrap();
        assert!(read.contains(crate::U256::ONE));
        assert!(read.contains(crate::U256::from(2)));
        assert_eq!(
            branch.payload.active().saved_store.state(),
            checkpoint.state()
        );
    }
}
