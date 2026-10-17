use super::*;
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, OutcomeKind,
        transfer::{StorageReadPolicy, execute_inner, initial},
    },
    domain::{AbstractValue, Domain, relational::QueryReason},
    world::{Account, Entry, Existence, World},
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

fn fixture() -> (World, Entry, ExecutionConfig, Domain, MachinePayload) {
    let mut world = World::new(Fork::Osaka, "late RIPEMD Failure");
    world
        .insert(
            address(0x101),
            Account::from_hex("60016002015b00", world.fork()).unwrap(),
        )
        .unwrap();
    for target in [3, 0x200, 0x300] {
        world.insert(address(target), Account::empty()).unwrap();
    }
    let entry = Entry::concrete(address(0x101), address(0x1000), word(0), ByteArray::empty());
    let mut config = ExecutionConfig::default();
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    let payload = initial(&world, &entry, domain).unwrap();
    (world, entry, config, domain, payload)
}

#[test]
fn relative_failure_restores_effects_without_finalizing_and_normal_execution_continues() {
    for may_touch in [false, true] {
        let (world, entry, config, domain, mut input) = fixture();
        if may_touch {
            input.store.may_touch_all();
        } else {
            input.store.touch_account(address(3));
        }
        let touch = input.store.touched_in_transaction(address(3));
        assert_eq!(touch, if may_touch { None } else { Some(true) });
        input.store.touch_account(address(0x300));
        input
            .store
            .write(address(0x200), &word(0), &word(7), domain);
        let mut budget = WorkBudget::new(config.max_work);
        let result = execute_inner(
            &world,
            &entry,
            input.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            true,
        );
        assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
        assert_eq!(result.outcomes.len(), 1);
        let failed = &result.outcomes[0];
        assert_eq!(failed.kind, OutcomeKind::Failure);
        assert_eq!(failed.store.existence(address(3)), Existence::Present);
        assert_eq!(failed.store.touched_in_transaction(address(3)), touch);
        assert_eq!(
            failed.store.touched_in_transaction(address(0x300)),
            Some(false)
        );
        assert_eq!(
            failed
                .store
                .read(address(0x200), &word(0), domain)
                .singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(result.successors.len(), 1, "pure ADD must continue");
        assert_eq!(
            result.payload.active().stack.last().unwrap().singleton(),
            Some(U256::from(3))
        );
        assert_eq!(
            result.payload.store, input.store,
            "the alternative cannot mutate live execution"
        );
        assert!(budget.used() > 0 && !budget.exhausted());
    }
}

#[test]
fn precompile_root_has_a_post_touch_failure_before_native_dispatch() {
    let (world, _, config, domain, _) = fixture();
    let entry = Entry::concrete(address(3), address(0x1000), word(0), ByteArray::empty());
    let input = initial(&world, &entry, domain).unwrap();
    assert_eq!(input.store.touched_in_transaction(address(3)), Some(true));
    let result = execute_inner(
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut WorkBudget::new(config.max_work),
        StorageReadPolicy::Abstract,
        false,
    );
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    for kind in [OutcomeKind::Failure, OutcomeKind::Return] {
        assert!(result.outcomes.iter().any(|outcome| {
            outcome.kind == kind && outcome.store.existence(address(3)) == Existence::Absent
        }));
    }
}

fn assert_refused(result: &Execution, input: &MachinePayload, reason: FrontierReason) {
    assert_eq!(result.payload, *input);
    assert!(result.executed_pcs.is_empty());
    assert!(result.outcomes.is_empty());
    assert!(result.completed_calls.is_empty());
    assert!(result.successors.is_empty());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].pc, 0);
    assert_eq!(result.frontiers[0].reason, reason);
}

#[test]
fn metered_inspection_refusal_and_sticky_exhaustion_leave_input_unchanged() {
    let (world, entry, config, domain, mut input) = fixture();
    input.store.touch_account(address(3));
    input.active_mut().stack.push(word(9).with_identity(7, 11));
    for (maximum, already_exhausted) in [
        (0, false),
        (TOUCH_QUERY_WORK, false),
        (config.max_work, true),
    ] {
        let mut budget = WorkBudget::new(maximum);
        if already_exhausted {
            assert!(!budget.charge(usize::MAX));
            assert!(budget.exhausted());
        }
        let result = execute_inner(
            &world,
            &entry,
            input.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        assert_refused(&result, &input, FrontierReason::Work);
        assert!(
            result.frontiers[0].target.is_none(),
            "a refused scan must not construct a key"
        );
    }
}

#[test]
fn checked_completion_refusal_cannot_install_the_extra_failure() {
    let (world, entry, config, domain, mut input) = fixture();
    input.store.touch_account(address(3));
    // Allow inspection and all extra clone/drop work, but no completion work.
    // This exercises refusal inside the existing checked finalization path.
    let mut admitted = WorkBudget::new(usize::MAX);
    reserve(&mut admitted, TOUCH_QUERY_WORK).unwrap();
    reserve_failure(&input, false, &mut admitted).unwrap();
    let mut budget = WorkBudget::new(admitted.used());
    let result = execute_inner(
        &world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut budget,
        StorageReadPolicy::Abstract,
        false,
    );
    assert_refused(&result, &input, FrontierReason::Work);
    assert!(budget.exhausted());
}

#[test]
fn cancellation_stays_typed_and_never_becomes_an_evm_failure() {
    let (world, entry, config, domain, mut input) = fixture();
    input.store.touch_account(address(3));
    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let mut budget = WorkBudget::new(config.max_work);
        cancellation.cancel();
        let result = execute_inner(
            &world,
            &entry,
            input.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        assert_refused(
            &result,
            &input,
            FrontierReason::Relations(QueryReason::Cancelled),
        );
        assert_eq!(budget.used(), 0);
    });
}
