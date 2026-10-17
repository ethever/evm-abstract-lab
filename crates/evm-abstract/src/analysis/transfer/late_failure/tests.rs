use super::*;
use crate::{
    Fork,
    analysis::{
        ChildFrame, Continuation, ExecutionConfig, OutcomeKind,
        transfer::{StorageReadPolicy, execute_inner, initial},
    },
    bytecode::Program,
    domain::{
        AbstractValue, Domain,
        provenance::Origin,
        relational::{QueryReason, RelationLimits, RelationState},
        symbolic::ExprId,
    },
    world::{AbstractLog, Account, Entry, Existence, LogKey, LogSourceKind, World},
};
use std::num::NonZeroUsize;

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

fn reserved_work(payload: &MachinePayload, procedure: bool) -> usize {
    let mut budget = WorkBudget::new(usize::MAX);
    reserve_failure(payload, procedure, &mut budget).unwrap();
    budget.used()
}

fn finite_words(count: u64) -> AbstractValue {
    let domain = Domain::new(NonZeroUsize::new(16).unwrap());
    (1..count).fold(word(0), |value, next| domain.join(&value, &word(next)))
}

fn guarded_relations(count: u64) -> RelationState {
    let limits = RelationLimits::default();
    let mut left = RelationState::default();
    let mut right = RelationState::default();
    for _ in 0..count {
        left.add_unsigned_bounds(
            &ExprId::fresh().unwrap(),
            U256::ZERO,
            U256::from(7),
            &limits,
        );
        right.add_unsigned_bounds(
            &ExprId::fresh().unwrap(),
            U256::from(8),
            U256::from(15),
            &limits,
        );
    }
    let joined = left
        .join_guarded(&right, ExprId::fresh().unwrap(), &limits)
        .unwrap();
    assert_eq!(joined.constraints().len(), 0);
    assert_eq!(joined.path().atoms(), count as usize * 2);
    joined
}

fn push_child(payload: &mut MachinePayload) {
    let state = payload.active().clone();
    payload.call_stack.push_child(ChildFrame {
        state,
        continuation: Continuation {
            return_block: Some(0),
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
}

#[test]
fn shared_retention_is_not_clone_work_and_error_keys_still_pay_raw_code_hashing() {
    let (_, _, _, domain, mut small) = fixture();
    push_child(&mut small);
    let leaf = ExprId::fresh().unwrap();
    small.active_mut().stack = vec![finite_words(2).with_expression(leaf.clone())];
    small.active_mut().calldata = ByteArray::exact(&[0]);
    small.relations = guarded_relations(1);
    let mut large = small.clone();
    let mut expression = leaf;
    for _ in 0..16 {
        expression = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[expression, ExprId::fresh().unwrap()],
            RelationLimits::default().expressions(),
        )
        .unwrap();
    }
    large.active_mut().stack[0] = finite_words(8).with_expression(expression);
    large.active_mut().calldata = ByteArray::exact(&[0; 4096]);
    let program = Program::decode_with_fork(&[0x5b; 4096], Fork::Osaka).unwrap();
    large.active_mut().program = Some(program.clone());
    large.relations = guarded_relations(8);
    let mut history = large.store.clone();
    for owner in 0x400..0x410 {
        history.write(address(owner), &word(0), &word(owner), domain);
    }
    large.call_stack.iter_mut().next().unwrap().saved_store = history.snapshot();
    assert_eq!(
        small.active().calldata.metadata_len(),
        large.active().calldata.metadata_len()
    );
    assert!(large.relations.path().nodes() > small.relations.path().nodes());
    assert!(large.active().stack[0].expression().unwrap().nodes() > 1);
    assert_eq!(reserved_work(&small, false), reserved_work(&large, false));

    // These are the same number of code entries and handles. Only the real
    // failure-key hashing component may grow with their raw runtime bytes.
    let mut short_code = small.store.clone();
    short_code.deploy_code(
        address(0x101),
        Program::decode_with_fork(&[0x5b], Fork::Osaka).unwrap(),
    );
    let mut long_code = short_code.clone();
    long_code.deploy_code(address(0x101), program);
    let short = store_work(&short_code, true, &mut WorkBudget::new(usize::MAX)).unwrap();
    let long = store_work(&long_code, true, &mut WorkBudget::new(usize::MAX)).unwrap();
    assert_eq!(short.clone, long.clone);
    assert!(long.key_error > short.key_error);
    let mut long_input = small.clone();
    long_input.store = long_code;
    let mut short_input = small;
    short_input.store = short_code;
    assert!(reserved_work(&long_input, false) > reserved_work(&short_input, false));
}

fn guarded_array(domain: Domain) -> ByteArray {
    let expression = ExprId::fresh().unwrap();
    let mut left = ByteArray::memory();
    let mut right = left.clone();
    for (array, lower) in [(&mut left, 2u64), (&mut right, 4u64)] {
        let value = AbstractValue::unsigned_range(U256::from(lower), U256::from(lower + 1))
            .unwrap()
            .with_expression(expression.clone());
        array.write_byte(&word(0), &value, 4096, domain).unwrap();
    }
    left.join_guarded(
        &right,
        domain,
        ExprId::fresh().unwrap(),
        domain.spec().relations().expressions(),
    )
    .unwrap()
}

#[test]
fn owned_metadata_members_origins_and_top_level_branches_increase_the_fee() {
    let (_, _, _, domain, mut input) = fixture();
    let before = reserved_work(&input, true);
    input.active_mut().stack.push(word(9));
    assert!(reserved_work(&input, true) > before);
    let before = reserved_work(&input, true);
    input.active_mut().stack[0] = word(9)
        .with_origin(Origin::Calldata)
        .with_origin(Origin::Storage);
    assert!(
        reserved_work(&input, true) > before,
        "owned OriginSet entries must be copied"
    );
    let before = reserved_work(&input, true);
    let entries = input.store.finalization_entries();
    input
        .store
        .write_balance(address(0x200), word(9).with_origin(Origin::Calldata));
    assert_eq!(input.store.finalization_entries(), entries);
    assert!(
        reserved_work(&input, true) > before,
        "Store values also own their OriginSets"
    );
    let before = reserved_work(&input, true);
    input.active_mut().memory = ByteArray::exact(&[1, 2, 3, 4]);
    assert!(
        reserved_work(&input, true) > before,
        "sparse byte-map entries are owned"
    );
    let before = reserved_work(&input, true);
    input
        .active_mut()
        .memory
        .expand(&word(64), &word(32), 4096, domain)
        .unwrap();
    assert!(
        reserved_work(&input, true) > before,
        "allocation witnesses are owned"
    );

    let expression = ExprId::fresh().unwrap();
    let mut members = RelationState::default();
    members
        .observe(
            &expression,
            finite_words(2).numeric(),
            &RelationLimits::default(),
        )
        .unwrap();
    input.relations = members;
    assert!(
        matches!(input.relations.constraints().next(), Some(Constraint::Members { values, .. }) if values.len() == 2)
    );
    let before = reserved_work(&input, true);
    let mut members = RelationState::default();
    members
        .observe(
            &expression,
            finite_words(8).numeric(),
            &RelationLimits::default(),
        )
        .unwrap();
    input.relations = members;
    assert_eq!(input.relations.constraints().len(), 1);
    assert!(
        reserved_work(&input, true) > before,
        "Members values own a BTreeSet"
    );

    let one = guarded_array(domain);
    assert_eq!(one.cloned_conditional_branches(), 1);
    let mut two = one.clone();
    two.copy_from(
        &word(64),
        &guarded_array(domain),
        &word(0),
        &word(1),
        4096,
        domain,
    )
    .unwrap();
    assert_eq!(two.cloned_conditional_branches(), 2);
    let one_work = array_work(&one, &mut WorkBudget::new(usize::MAX)).unwrap();
    let two_work = array_work(&two, &mut WorkBudget::new(usize::MAX)).unwrap();
    assert!(
        two_work > one_work,
        "the top-level conditional Vec is copied"
    );

    let log_key = LogKey {
        address: address(0x101),
        code_address: address(0x101),
        code_hash: input.active().key.code_hash,
        source_kind: LogSourceKind::Runtime,
        pc: 0,
    };
    let mut logged = input.clone();
    logged
        .store
        .emit_log(
            log_key.clone(),
            AbstractLog {
                topics: vec![word(1)],
                data: ByteArray::exact(&[1]),
            },
            domain,
        )
        .unwrap();
    assert!(reserved_work(&logged, true) > reserved_work(&input, true));
    let mut more_log_data = input;
    more_log_data
        .store
        .emit_log(
            log_key,
            AbstractLog {
                topics: vec![word(1), word(2)],
                data: ByteArray::exact(&[1, 2, 3, 4]),
            },
            domain,
        )
        .unwrap();
    assert!(reserved_work(&more_log_data, true) > reserved_work(&logged, true));
    let before = reserved_work(&more_log_data, true);
    push_child(&mut more_log_data);
    assert!(
        reserved_work(&more_log_data, true) > before,
        "child snapshots and continuations are retained"
    );
    let before = reserved_work(&more_log_data, true);
    more_log_data
        .call_stack
        .children_mut()
        .last_mut()
        .unwrap()
        .continuation
        .output_size = word(0).with_origin(Origin::Returndata);
    assert!(
        reserved_work(&more_log_data, true) > before,
        "continuation values own OriginSets"
    );
}

#[test]
fn exact_reservation_admits_failure_and_one_short_keeps_input_and_prior_debits() {
    let (_, _, config, domain, mut input) = fixture();
    input.store.touch_account(address(3));
    input.active_mut().stack.push(word(9).with_identity(7, 11));
    input
        .store
        .write(address(0x200), &word(0), &word(7), domain);
    let needed = TOUCH_QUERY_WORK + reserved_work(&input, true);
    let prior = 37;
    for (maximum, accepted) in [(prior + needed, true), (prior + needed - 1, false)] {
        let mut budget = WorkBudget::new(maximum);
        assert!(budget.charge(prior));
        let mut result = super::super::empty_execution(input.clone());
        let mut context = TransferContext {
            config: &config,
            domain,
            budget: &mut budget,
            root_address: None,
            input_scope: None,
            query_cache: Default::default(),
            procedure: true,
            discover_code: false,
        };
        assert_eq!(at_block_entry(&mut result, &mut context), accepted);
        assert_eq!(
            result.payload, input,
            "the normal alternative must remain intact"
        );
        if accepted {
            assert_eq!(budget.used(), maximum);
            assert!(!budget.exhausted());
            assert!(result.frontiers.is_empty());
            assert_eq!(result.outcomes.len(), 1);
            let failed = &result.outcomes[0];
            assert_eq!(failed.kind, OutcomeKind::Failure);
            assert_eq!(failed.store.touched_in_transaction(address(3)), Some(true));
            assert_eq!(
                failed
                    .store
                    .read(address(0x200), &word(0), domain)
                    .singleton(),
                Some(U256::ZERO)
            );
        } else {
            assert_refused(&result, &input, FrontierReason::Work);
            assert!(budget.exhausted());
            assert!(
                budget.used() > prior + TOUCH_QUERY_WORK,
                "paid census work is never refunded"
            );
            assert!(
                budget.used() < maximum,
                "the final failed reservation is atomic"
            );
        }
    }
}
