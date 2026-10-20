use super::*;
use crate::{
    Fork,
    analysis::{
        ChildFrame, Continuation, DiagnosticKind, ExecutionConfig, FrontierReason, OutcomeKind,
        control::Control, progress::Observer, transfer,
    },
    bytecode::Program,
    domain::{Domain, provenance::Origin, relational::QueryReason, symbolic::ExprId},
    world::{Account, ByteArray, Code, Entry, Existence, World},
};
use alloy_primitives::Address;

fn fixture(value: Option<u64>) -> (World, Entry, Arc<CodeCatalog>) {
    fixture_with_code("60026003016008565b00", value)
}

fn fixture_with_code(code: &str, value: Option<u64>) -> (World, Entry, Arc<CodeCatalog>) {
    let program = Program::from_hex_with_fork(code, Fork::Osaka).unwrap();
    let (catalog, _) = CodeCatalog::new(Fork::Osaka)
        .with_runtime(program.clone())
        .unwrap();
    let address = Address::repeat_byte(0x22);
    let mut account = Account::unknown();
    account.code = Code::Runtime(program);
    if let Some(value) = value {
        account
            .storage
            .insert(U256::ZERO, AbstractValue::constant(U256::from(value)));
    }
    let mut world = World::offline(Fork::Osaka, "memo state", "test");
    world.insert(address, account).unwrap();
    (world, Entry::new(address), Arc::new(catalog))
}

#[test]
fn hit_keeps_new_store_and_all_saved_checkpoints() {
    let (old_world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let domain = Domain::default();
    let input =
        crate::analysis::transfer::procedure::testing::initial(&old_world, &entry, domain).unwrap();
    let mut ledger = WorkBudget::new(config.max_work);
    let execution = crate::analysis::transfer::procedure::testing::execute(
        &old_world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    let mut reuse = ExecutionReuse::new(
        catalog,
        config.analysis.max_states,
        config.max_work,
        config.max_memory_bytes,
    );
    reuse.remember(&input, &execution, &mut ledger);
    assert_eq!(reuse.stats().entries, 1);
    let (mut new_world, _, _) = fixture(Some(3));
    let other = Address::repeat_byte(0x44);
    let mut other_account = Account::empty();
    other_account
        .storage
        .insert(U256::ZERO, AbstractValue::constant(U256::from(7)));
    new_world.insert(other, other_account).unwrap();
    let new =
        crate::analysis::transfer::procedure::testing::initial(&new_world, &entry, domain).unwrap();
    assert_ne!(input.store, new.store);
    assert_ne!(input.active().saved_store, new.active().saved_store);
    assert_eq!(
        new.store
            .touched_in_transaction(Address::from_word(U256::from(3).into())),
        Some(false)
    );
    let hit = reuse.lookup(&new, &mut ledger).unwrap().unwrap();
    assert_eq!(hit.payload.store, new.store);
    assert_eq!(hit.payload.active().saved_store, new.active().saved_store);
    assert_eq!(
        hit.payload.active().stack.last().unwrap().singleton(),
        Some(U256::from(5))
    );
    for successor in hit.successors {
        assert_eq!(successor.payload.store, new.store);
        assert_eq!(
            successor.payload.active().saved_store,
            new.active().saved_store
        );
    }
    let mut ordinary = crate::analysis::transfer::procedure::testing::execute(
        &new_world,
        &entry,
        new.clone(),
        &config,
        domain,
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    ordinary.payload.normalize();
    let cached = reuse.lookup(&new, &mut ledger).unwrap().unwrap();
    assert_eq!(cached.payload, ordinary.payload);
    assert_eq!(cached.executed_pcs, ordinary.executed_pcs);
    assert_eq!(cached.successors.len(), ordinary.successors.len());
    for (cached, mut ordinary) in cached.successors.into_iter().zip(ordinary.successors) {
        ordinary.payload.normalize();
        assert_eq!(cached.kind, ordinary.kind);
        assert_eq!(cached.payload, ordinary.payload);
    }
    let mut nested = new.clone();
    nested.call_stack.push_child(ChildFrame {
        state: input.active().clone(),
        continuation: Continuation {
            return_block: Some(1),
            output_offset: AbstractValue::constant(U256::ZERO),
            output_size: AbstractValue::constant(U256::from(32)),
            creation: None,
        },
    });
    let nested_hit = reuse.lookup(&nested, &mut ledger).unwrap().unwrap();
    let mut expected = nested.clone();
    expected.active_mut().key = nested_hit.payload.active().key.clone();
    expected.active_mut().stack = nested_hit.payload.active().stack.clone();
    assert_eq!(nested_hit.payload, expected);
    assert_eq!(
        nested_hit.payload.call_stack.root(),
        nested.call_stack.root()
    );
    assert_eq!(
        nested_hit.payload.active().saved_store,
        nested.active().saved_store
    );
    for successor in &nested_hit.successors {
        let mut expected = nested.clone();
        expected.active_mut().key = successor.payload.active().key.clone();
        expected.active_mut().stack = successor.payload.active().stack.clone();
        assert_eq!(successor.payload, expected);
    }
    assert!(reuse.retained_bytes <= reuse.max_retained_bytes);
}

#[test]
fn touched_ripemd_inputs_fall_back_to_fresh_failure_and_normal_execution() {
    let (mut world, entry, catalog) = fixture_with_code("60026003016008565b00", None);
    let ripemd = Address::from_word(U256::from(3).into());
    let other = Address::repeat_byte(0x44);
    for address in [ripemd, other] {
        world.insert(address, Account::empty()).unwrap();
    }
    let config = ExecutionConfig::default();
    let domain = Domain::default();
    let zero = AbstractValue::constant(U256::ZERO);
    let mut input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    input
        .active_mut()
        .stack
        .push(AbstractValue::constant(U256::from(7)));
    input.active_mut().key.stack_height = 1;
    assert_eq!(input.store.touched_in_transaction(ripemd), Some(false));
    let original = input.clone();
    let mut preparation = WorkBudget::new(config.max_work);
    let execution = crate::analysis::transfer::procedure::testing::execute(
        &world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut preparation,
        transfer::StorageReadPolicy::Abstract,
    );
    assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
    assert!(execution.outcomes.is_empty());
    assert_eq!(execution.successors.len(), 1);
    let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, config.max_memory_bytes);
    reuse.remember(&input, &execution, &mut preparation);
    assert_eq!(reuse.stats().entries, 1);
    let before_stats = reuse.stats();
    let before_bytes = reuse.retained_bytes;
    let before_dependencies = reuse.dependencies.clone();
    for may_touch in [false, true] {
        let mut current = input.clone();
        if may_touch {
            current.store.may_touch_account(ripemd);
        } else {
            current.store.touch_account(ripemd);
        }
        let touch = if may_touch { None } else { Some(true) };
        assert_eq!(current.store.touched_in_transaction(ripemd), touch);
        // Distinguish live effects from the real root checkpoint on Failure.
        current.store.touch_account(other);
        current.store.write(
            other,
            &zero,
            &AbstractValue::constant(U256::from(9)),
            domain,
        );
        assert_eq!(current.call_stack, input.call_stack);
        assert_eq!(reuse.key(&current), reuse.key(&input));
        let before = current.clone();
        let mut lookup = WorkBudget::new(config.max_work);
        assert!(reuse.lookup(&current, &mut lookup).unwrap().is_none());
        assert_eq!(lookup.used(), ELIGIBILITY_WORK + TOUCH_QUERY_WORK);
        assert!(!lookup.exhausted());
        assert_eq!(current, before);
        assert_eq!(reuse.stats(), before_stats);
        assert_eq!(reuse.memo.len(), 1);
        assert_eq!(reuse.retained_bytes, before_bytes);
        assert_eq!(reuse.dependencies, before_dependencies);

        let mut ledger = WorkBudget::new(config.max_work);
        let fresh = crate::analysis::transfer::procedure::testing::execute(
            &world,
            &entry,
            current.clone(),
            &config,
            domain,
            &mut ledger,
            transfer::StorageReadPolicy::Abstract,
        );
        assert!(fresh.frontiers.is_empty(), "{:?}", fresh.frontiers);
        assert!(fresh.diagnostics.is_empty());
        assert!(fresh.completed_calls.is_empty());
        assert_eq!(fresh.outcomes.len(), 1);
        let failed = &fresh.outcomes[0];
        assert_eq!(failed.kind, OutcomeKind::Failure);
        assert_eq!(failed.data, ByteArray::empty());
        let saved = current.active().saved_store.state();
        assert_eq!(saved.touched_in_transaction(ripemd), Some(false));
        let mut restored = current.store.clone();
        restored.restore(current.active().saved_store.clone());
        assert_eq!(restored.touched_in_transaction(ripemd), touch);
        assert_eq!(restored.touched_in_transaction(other), Some(false));
        assert_eq!(
            restored.read(other, &zero, domain),
            saved.read(other, &zero, domain)
        );
        restored.finalize_transaction(domain).unwrap();
        let existence = if may_touch {
            Existence::Unknown
        } else {
            Existence::Absent
        };
        assert_eq!(failed.store.existence(ripemd), existence);
        assert_eq!(restored.existence(ripemd), existence);
        assert_eq!(failed.store.code(ripemd), Some(&Code::Empty));
        assert_eq!(
            failed.store.read_balance(ripemd).singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(failed.store.nonce(ripemd).singleton(), Some(U256::ZERO));
        assert_eq!(
            failed.store.read(ripemd, &zero, domain).singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(
            failed.store.read(other, &zero, domain),
            saved.read(other, &zero, domain)
        );
        assert_eq!(failed.store.touched_in_transaction(ripemd), Some(false));
        if !may_touch {
            assert_eq!(failed.store, restored);
        }
        assert_eq!(fresh.successors.len(), 1, "pure ADD/JUMP must continue");
        assert_eq!(fresh.successors[0].kind, execution.successors[0].kind);
        assert_eq!(fresh.payload.store, current.store);
        assert_eq!(
            fresh.payload.active().saved_store,
            current.active().saved_store
        );
        assert_eq!(
            fresh.payload.active().stack.last().unwrap().singleton(),
            Some(U256::from(5))
        );
        for successor in &fresh.successors {
            assert_eq!(successor.payload.store, current.store);
            assert_eq!(
                successor.payload.active().saved_store,
                current.active().saved_store
            );
        }
        let mut learning = WorkBudget::new(config.max_work);
        reuse.remember(&current, &fresh, &mut learning);
        assert_eq!(learning.used(), ELIGIBILITY_WORK);
        assert!(!learning.exhausted() && !ledger.exhausted());
        assert_eq!(reuse.stats(), before_stats);
        assert_eq!(reuse.memo.len(), 1);
        assert_eq!(reuse.retained_bytes, before_bytes);
        assert_eq!(reuse.dependencies, before_dependencies);
        assert_eq!(current, before);
    }
    assert_eq!(input, original);
}

#[test]
fn budget_and_current_cancellation_refuse_reuse_without_partial_effects() {
    let (world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, Domain::default())
            .unwrap();
    let before = input.clone();
    let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, config.max_memory_bytes);
    let mut preparation = WorkBudget::new(config.max_work);
    let execution = crate::analysis::transfer::procedure::testing::execute(
        &world,
        &entry,
        input.clone(),
        &config,
        Domain::default(),
        &mut preparation,
        transfer::StorageReadPolicy::Abstract,
    );
    reuse.remember(&input, &execution, &mut preparation);
    assert_eq!(reuse.stats().entries, 1);
    let before_stats = reuse.stats();
    let before_bytes = reuse.retained_bytes;
    let before_dependencies = reuse.dependencies.clone();
    for allowance in [0, ELIGIBILITY_WORK - 1] {
        let mut refused = WorkBudget::new(allowance);
        assert!(reuse.lookup(&input, &mut refused).unwrap().is_none());
        reuse.remember(&input, &execution, &mut refused);
        assert_eq!(refused.used(), 0);
        assert!(!refused.exhausted());
        assert_eq!(reuse.stats(), before_stats);
        assert_eq!(reuse.retained_bytes, before_bytes);
        assert_eq!(reuse.memo.len(), 1);
        assert_eq!(reuse.dependencies, before_dependencies);
    }
    for allowance in [ELIGIBILITY_WORK, ELIGIBILITY_WORK + TOUCH_QUERY_WORK - 1] {
        // Metadata fits, but the independent Store lookup must be declined
        // before its getter or any full signature/key construction.
        for learning in [false, true] {
            let mut refused = WorkBudget::new(allowance);
            if learning {
                reuse.remember(&input, &execution, &mut refused);
            } else {
                assert!(reuse.lookup(&input, &mut refused).unwrap().is_none());
            }
            assert_eq!(refused.used(), ELIGIBILITY_WORK);
            assert!(!refused.exhausted());
            assert_eq!(reuse.stats(), before_stats);
            assert_eq!(reuse.retained_bytes, before_bytes);
            assert_eq!(reuse.memo.len(), 1);
            assert_eq!(reuse.dependencies, before_dependencies);
            assert_eq!(input, before);
        }
    }
    let mut exhausted = WorkBudget::new(0);
    assert!(!exhausted.charge(1));
    assert!(reuse.lookup(&input, &mut exhausted).unwrap().is_none());
    assert!(exhausted.exhausted());
    assert_eq!(exhausted.used(), 0);
    let cancellation = Control::new(Observer::default());
    cancellation.cancellation().cancel();
    let mut ledger = WorkBudget::new(config.max_work);
    cancellation.scope(|| {
        assert!(matches!(
            reuse.lookup(&input, &mut ledger),
            Err(MachineError::Relations(QueryReason::Cancelled))
        ));
    });
    assert_eq!(ledger.used(), 0);
    let captured = Control::new(Observer::default());
    let mut captured_ledger = captured.scope(|| WorkBudget::new(config.max_work));
    captured.cancellation().cancel();
    assert!(reuse.lookup(&input, &mut captured_ledger).is_err());
    assert_eq!(captured_ledger.used(), 0);
    assert_eq!(reuse.stats(), before_stats);
    assert_eq!(input, before);
    assert!(reuse.lookup(&input, &mut ledger).unwrap().is_some());
}

#[test]
fn ineligible_inputs_pay_only_metadata_before_lookup_or_remember() {
    let config = ExecutionConfig::default();
    let domain = Domain::default();
    let (world, entry, _) = fixture(None);
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let mut preparation = WorkBudget::new(config.max_work);
    let clean = crate::analysis::transfer::procedure::testing::execute(
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut preparation,
        transfer::StorageReadPolicy::Abstract,
    );
    for code in ["f100", "00", "60026003016008565b00"] {
        let (world, entry, catalog) = fixture_with_code(code, None);
        let input =
            crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
        let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, config.max_memory_bytes);
        let mut candidates = Vec::new();
        if code != "60026003016008565b00" {
            // CALL/STOP are rejected from cached block metadata on lookup.
            candidates.push(input.clone());
        } else {
            let mut initcode = input.clone();
            initcode.active_mut().code = FrameCode::InitCode;
            initcode.active_mut().key.mode = FrameCode::InitCode;
            candidates.push(initcode);
            let mut related = input.clone();
            related
                .relations
                .assume(&ExprId::fresh().unwrap(), true, &domain.spec().relations());
            assert!(!related.relations.is_empty());
            candidates.push(related);
            let mut absent = input.clone();
            absent.active_mut().program = None;
            candidates.push(absent);
        }
        for candidate in candidates {
            let mut ledger = WorkBudget::new(config.max_work);
            assert!(reuse.lookup(&candidate, &mut ledger).unwrap().is_none());
            assert_eq!(ledger.used(), ELIGIBILITY_WORK);
            reuse.remember(&candidate, &clean, &mut ledger);
            assert_eq!(ledger.used(), 2 * ELIGIBILITY_WORK);
            assert!(!ledger.exhausted());
            assert_eq!(reuse.stats(), ReuseStats::default());
            assert!(reuse.memo.is_empty());
            assert_eq!(reuse.retained_bytes, 0);
        }
    }
}

#[test]
fn disqualified_results_return_before_full_signature_or_key() {
    let (world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let domain = Domain::default();
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, config.max_memory_bytes);
    enum Disqualify {
        Successors,
        Frontier,
        Outcome,
        CompletedCall,
        Diagnostic,
    }
    let disqualify = [
        Disqualify::Successors,
        Disqualify::Frontier,
        Disqualify::Outcome,
        Disqualify::CompletedCall,
        Disqualify::Diagnostic,
    ];
    for disqualify in disqualify {
        let mut preparation = WorkBudget::new(config.max_work);
        let mut execution = crate::analysis::transfer::procedure::testing::execute(
            &world,
            &entry,
            input.clone(),
            &config,
            domain,
            &mut preparation,
            transfer::StorageReadPolicy::Abstract,
        );
        assert!(!execution.successors.is_empty());
        match disqualify {
            Disqualify::Successors => execution.successors.clear(),
            Disqualify::Frontier => {
                execution.frontiers.push(transfer::ExecutionFrontier {
                    pc: 0,
                    reason: FrontierReason::Work,
                    target: None,
                    range: None,
                    payload: None,
                });
            }
            Disqualify::Outcome => {
                execution.outcomes.push(transfer::Outcome {
                    kind: OutcomeKind::Return,
                    data: ByteArray::empty(),
                    store: execution.payload.store.clone(),
                });
            }
            Disqualify::CompletedCall => {
                execution.completed_calls.push(transfer::CompletedCall {
                    payload: execution.payload.clone(),
                    kind: OutcomeKind::Return,
                    data: ByteArray::empty(),
                    output_store: execution.payload.store.clone(),
                    output_kind: OutcomeKind::Return,
                    output_data: ByteArray::empty(),
                });
            }
            Disqualify::Diagnostic => {
                execution.diagnostics.push((0, DiagnosticKind::InvalidJump));
            }
        }
        let mut ledger = WorkBudget::new(config.max_work);
        reuse.remember(&input, &execution, &mut ledger);
        assert_eq!(ledger.used(), ELIGIBILITY_WORK);
        assert!(!ledger.exhausted());
        assert_eq!(reuse.stats(), ReuseStats::default());
        assert!(reuse.memo.is_empty());
        assert_eq!(reuse.retained_bytes, 0);
    }
}

#[test]
fn eligible_paths_keep_full_signature_program_stack_and_policy_guards() {
    let (world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let domain = Domain::default();
    let mut input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    input
        .active_mut()
        .stack
        .push(AbstractValue::constant(U256::from(7)));
    input.active_mut().key.stack_height = 1;
    let mut reuse = ExecutionReuse::new(
        Arc::clone(&catalog),
        4,
        config.max_work,
        config.max_memory_bytes,
    );
    let mut ledger = WorkBudget::new(config.max_work);
    assert!(reuse.lookup(&input, &mut ledger).unwrap().is_none());
    assert_eq!(
        ledger.used(),
        ELIGIBILITY_WORK + TOUCH_QUERY_WORK + reuse.signature_work(&input)
    );
    assert_eq!(reuse.stats().misses, 1);
    let execution = crate::analysis::transfer::procedure::testing::execute(
        &world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    let before = ledger.used();
    let signature = reuse.signature_work(&input);
    reuse.remember(&input, &execution, &mut ledger);
    assert_eq!(reuse.stats().entries, 1);
    assert!(ledger.used() > before + ELIGIBILITY_WORK + TOUCH_QUERY_WORK + signature);
    let memo = reuse.memo.values().next().unwrap();
    let comparison = memo.work_size() + input.active().work_size();
    let copy = input.work_size() * (memo.successors.len() + 1) + memo.work_size();
    let before = ledger.used();
    assert!(reuse.lookup(&input, &mut ledger).unwrap().is_some());
    assert_eq!(
        ledger.used() - before,
        ELIGIBILITY_WORK + TOUCH_QUERY_WORK + signature + comparison + copy
    );

    let mut stack = input.clone();
    stack.active_mut().stack[0] = AbstractValue::constant(U256::from(8));
    let mut policy = input.clone();
    policy.active_mut().key.is_static = true;
    for changed in [stack, policy] {
        let before = ledger.used();
        assert!(reuse.lookup(&changed, &mut ledger).unwrap().is_none());
        assert_eq!(
            ledger.used() - before,
            ELIGIBILITY_WORK + TOUCH_QUERY_WORK + reuse.signature_work(&changed)
        );
    }
    let mut source = input.clone();
    source.active_mut().stack[0] = source.active().stack[0]
        .clone()
        .with_origin(Origin::Address);
    assert_eq!(reuse.key(&source), reuse.key(&input));
    let comparison = reuse.memo.values().next().unwrap().work_size() + source.active().work_size();
    let before = ledger.used();
    assert!(reuse.lookup(&source, &mut ledger).unwrap().is_none());
    assert_eq!(
        ledger.used() - before,
        ELIGIBILITY_WORK + TOUCH_QUERY_WORK + reuse.signature_work(&source) + comparison
    );

    let mut different_program = input.clone();
    different_program.active_mut().program =
        Some(Program::from_hex_with_fork("60026004016008565b00", Fork::Osaka).unwrap());
    assert_eq!(
        different_program.active().key.code_hash,
        input.active().key.code_hash
    );
    assert_ne!(different_program.active().program, input.active().program);
    let before_stats = reuse.stats();
    let mut ledger = WorkBudget::new(config.max_work);
    assert!(
        reuse
            .lookup(&different_program, &mut ledger)
            .unwrap()
            .is_none()
    );
    let full_key_work =
        ELIGIBILITY_WORK + TOUCH_QUERY_WORK + reuse.signature_work(&different_program);
    assert_eq!(ledger.used(), full_key_work);
    reuse.remember(&different_program, &execution, &mut ledger);
    assert_eq!(ledger.used(), 2 * full_key_work);
    assert_eq!(reuse.stats(), before_stats);
    assert_eq!(reuse.memo.len(), 1);

    reuse.set_catalog(Arc::new(CodeCatalog::new(Fork::Osaka)));
    let mut ledger = WorkBudget::new(config.max_work);
    assert!(reuse.lookup(&input, &mut ledger).unwrap().is_none());
    assert_eq!(ledger.used(), ELIGIBILITY_WORK);
    assert_eq!(reuse.stats(), before_stats);
    reuse.set_catalog(catalog);
    assert!(reuse.lookup(&input, &mut ledger).unwrap().is_some());
}

#[test]
fn optional_retention_cap_refuses_entry_without_changing_execution() {
    let (world, entry, catalog) = fixture(None);
    let config = ExecutionConfig::default();
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, Domain::default())
            .unwrap();
    let mut ledger = WorkBudget::new(config.max_work);
    let execution = crate::analysis::transfer::procedure::testing::execute(
        &world,
        &entry,
        input.clone(),
        &config,
        Domain::default(),
        &mut ledger,
        transfer::StorageReadPolicy::Abstract,
    );
    let original_stack = execution.payload.active().stack.clone();
    let mut reuse = ExecutionReuse::new(catalog, 4, config.max_work, 0);
    reuse.remember(&input, &execution, &mut ledger);
    assert_eq!(reuse.stats().entries, 0);
    assert_eq!(reuse.retained_bytes, 0);
    assert_eq!(execution.payload.active().stack, original_stack);
    assert!(!ledger.exhausted());
}
