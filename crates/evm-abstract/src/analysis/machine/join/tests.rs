use super::{EntryAlternatives, JoinBoundary, MachineError};
use crate::{
    analysis::{Continuation, FrontierReason, OutcomeKind, RootFrame, Status, machine::fixtures},
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, ValueQuery},
        symbolic::{ByteStateKind, ExprId, ExprKind, SlotStateKind},
    },
    resource::WorkBudget,
    world::{ByteArray, Snapshot},
};
use alloy_primitives::U256;

fn scalar_guard(value: &AbstractValue) -> ExprId {
    match value.expression().expect("joined scalar expression").kind() {
        ExprKind::Ite { condition, .. } => condition.clone(),
        other => panic!("expected scalar ITE, got {other:?}"),
    }
}

fn bytes_guard(bytes: &ByteArray) -> ExprId {
    match bytes.state_root().kind() {
        ByteStateKind::Ite { condition, .. } => condition.clone(),
        other => panic!("expected byte ITE, got {other:?}"),
    }
}

#[test]
fn one_selector_pairs_every_frame_value_root_checkpoint_and_continuation() {
    let domain = fixtures::config().domain().unwrap();
    let mut left = fixtures::payload(domain);
    let mut right = left.clone();
    let predicate = ExprId::fresh().unwrap();
    let policy = domain.spec().relations();
    assert_eq!(
        left.relations.assume(&predicate, true, &policy),
        CheckResult::Sat
    );
    assert_eq!(
        right.relations.assume(&predicate, false, &policy),
        CheckResult::Sat
    );
    for (payload, at, marker) in [(&mut left, 0x80, 11), (&mut right, 0xc0, 22)] {
        payload
            .store
            .write_checked(
                fixtures::address(0x101),
                &fixtures::word(0),
                &fixtures::word(marker),
                domain,
            )
            .unwrap();
        let saved = payload.store.snapshot();
        let frame = payload.active_mut();
        frame.stack = vec![fixtures::word(at)];
        frame
            .memory
            .write_word(&fixtures::word(at), &fixtures::word(7), 4096, domain)
            .unwrap();
        frame.calldata = ByteArray::exact(&[marker as u8]);
        frame.returndata = ByteArray::exact(&[marker as u8, 1]);
        frame.call_value = fixtures::word(marker);
        frame.saved_store = saved;
        let child = RootFrame {
            state: frame.clone(),
        }
        .into_child(Continuation {
            return_block: Some(1),
            output_offset: fixtures::word(at),
            output_size: fixtures::word(marker),
            creation: None,
        });
        payload.call_stack.push_child(child);
        payload.normalize();
    }
    let joined = left.join(&right, domain).unwrap();
    let selector = scalar_guard(&joined.active().stack[0]);
    for frame in joined.call_stack.iter() {
        assert_eq!(scalar_guard(&frame.stack[0]), selector);
        assert_eq!(scalar_guard(&frame.call_value), selector);
        for bytes in [&frame.memory, &frame.calldata, &frame.returndata] {
            assert_eq!(bytes_guard(bytes), selector);
        }
        match frame
            .saved_store
            .state()
            .slot_root(fixtures::address(0x101), false)
            .unwrap()
            .kind()
        {
            SlotStateKind::Ite { condition, .. } => assert_eq!(*condition, selector),
            other => panic!("checkpoint lost guard: {other:?}"),
        }
    }
    match joined
        .store
        .slot_root(fixtures::address(0x101), false)
        .unwrap()
        .kind()
    {
        SlotStateKind::Ite { condition, .. } => assert_eq!(*condition, selector),
        other => panic!("store lost guard: {other:?}"),
    }
    let continuation = &joined.call_stack.active_child().unwrap().continuation;
    assert_eq!(scalar_guard(&continuation.output_offset), selector);
    assert_eq!(scalar_guard(&continuation.output_size), selector);
    let pointer = &joined.active().stack[0];
    let loaded = joined
        .active()
        .memory
        .read_word_checked(pointer, domain)
        .unwrap();
    assert_eq!(
        joined
            .relations
            .unique_value(loaded.expression().unwrap(), &policy),
        ValueQuery::Unique(U256::from(7))
    );
    let mut chosen = joined.relations.clone();
    assert_eq!(chosen.assume(&selector, true, &policy), CheckResult::Sat);
    assert_eq!(
        chosen.can_equal(&predicate, U256::ZERO, &policy),
        CheckResult::Unsat
    );
}

#[test]
fn checked_join_and_rename_leave_all_original_fields_unchanged_on_failure() {
    let mut config = fixtures::config();
    let domain = config.domain().unwrap();
    let left = fixtures::payload(domain);
    let mut right = left.clone();
    right
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    let before = (left.clone(), right.clone());
    config.analysis.relations.max_nodes = 1;
    let failure = left.join(&right, config.domain().unwrap()).unwrap_err();
    assert!(matches!(
        failure,
        MachineError::Expression(_) | MachineError::Bytes(_)
    ));
    assert_eq!((left.clone(), right.clone()), before);

    let local = ExprId::fresh().unwrap();
    right
        .active_mut()
        .stack
        .push(AbstractValue::top().with_expression(local.clone()));
    let expression = ExprId::operation(
        revm_bytecode::opcode::ADD,
        &[local.clone(), ExprId::constant(U256::from(1))],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    right
        .active_mut()
        .stack
        .push(AbstractValue::top().with_expression(expression));
    let snapshot = right.clone();
    let map = std::collections::BTreeMap::from([(local, ExprId::fresh().unwrap())]);
    assert!(
        right
            .rename_fresh(&map, config.analysis.relations.expressions())
            .is_err()
    );
    assert_eq!(right, snapshot);
}

#[test]
fn previously_represented_whole_payload_is_absorbed_without_a_new_selector() {
    let domain = fixtures::config().domain().unwrap();
    let first = fixtures::payload(domain);
    let mut second = first.clone();
    second
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    let mut alternatives = EntryAlternatives::new(&first);
    let mut budget = WorkBudget::new(10_000_000);
    let joined = alternatives
        .join(&first, &second, domain, &mut budget)
        .unwrap()
        .unwrap();
    alternatives.retain(second.clone());
    let identities = joined.fresh_leaves();
    assert!(
        alternatives
            .join(&joined, &second, domain, &mut budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(joined.fresh_leaves(), identities);
    assert_eq!(first.join(&first, domain).unwrap(), first);
}

#[test]
fn repeated_same_word_write_loop_closes_in_finite_transfers_in_both_modes() {
    for enabled in [true, false] {
        let mut config = fixtures::config();
        config.analysis.relations.enabled = enabled;
        let analysis = fixtures::run("5b60075f525f56", config);
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        assert!(analysis.transfers() <= 5, "{}", analysis.transfers());
        assert_eq!(analysis.states().len(), 1);
        assert_eq!(
            analysis.states()[0]
                .exit
                .as_ref()
                .unwrap()
                .active()
                .memory
                .read_word(&fixtures::word(0), Domain::default())
                .singleton(),
            Some(U256::from(7))
        );
    }
}

#[test]
fn bytecode_branch_join_keeps_the_pointer_paired_with_its_memory() {
    let code = "5f35600f5760076080526080601a565b600760c05260c0601a565b515f5260205ff3";
    let (world, mut entry) = fixtures::fixture(&[(0x101, code)]);
    entry.environment.calldata = ByteArray::unknown();
    let analysis = crate::analysis::analyze_world(world, entry, fixtures::config()).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    let returns: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect();
    assert!(!returns.is_empty());
    for outcome in returns {
        assert_eq!(
            outcome
                .data
                .read_word(&fixtures::word(0), Domain::default())
                .singleton(),
            Some(U256::from(7))
        );
    }
    // One real join block receives both branch edges with context_depth=0.
    let join = analysis
        .states()
        .iter()
        .find(|state| {
            state.program().unwrap().blocks()[state.active().basic_block_index].start_pc == 26
        })
        .unwrap();
    assert_eq!(
        analysis
            .edges()
            .iter()
            .filter(|edge| edge.to == join.id)
            .count(),
        2
    );
    crate::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
}

#[test]
fn repeated_two_memory_words_reach_a_whole_machine_fixed_point() {
    let analysis = fixtures::run("5b60075f5260096020525f56", fixtures::config());
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.transfers() <= 5, "{}", analysis.transfers());
    assert_eq!(analysis.states().len(), 1);
    let memory = &analysis.states()[0].exit.as_ref().unwrap().active().memory;
    for (offset, expected) in [(0, 7), (32, 9)] {
        assert_eq!(
            memory
                .read_word(&fixtures::word(offset), Domain::default())
                .singleton(),
            Some(U256::from(expected))
        );
    }
    crate::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
}

#[test]
fn repeated_two_storage_slots_reach_a_whole_machine_fixed_point() {
    let analysis = fixtures::run("5b60075f5560096001555f56", fixtures::config());
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.transfers() <= 5, "{}", analysis.transfers());
    assert_eq!(analysis.states().len(), 1);
    let store = &analysis.states()[0].exit.as_ref().unwrap().store;
    for (slot, expected) in [(0, 7), (1, 9)] {
        assert_eq!(
            store
                .read_checked(
                    fixtures::address(0x101),
                    &fixtures::word(slot),
                    Domain::default()
                )
                .unwrap()
                .singleton(),
            Some(U256::from(expected))
        );
    }
    crate::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
}

#[test]
fn growing_loop_preserves_the_uncovered_incoming_state_at_its_policy_boundary() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 2;
    config.analysis.relations.enabled = false;
    // Both the counter and its stored word change. Scalar-only widening cannot
    // discard the root/counter pairing to close this whole-state loop.
    let analysis = fixtures::run("60015b805f52600101600256", config);
    assert_eq!(analysis.status(), Status::Incomplete);
    let frontier = analysis
        .frontiers()
        .iter()
        .find(|frontier| {
            matches!(
                frontier.reason,
                FrontierReason::StateJoin(JoinBoundary::Alternatives { limit: 2 })
            )
        })
        .expect("explicit whole-state policy frontier");
    let incoming = frontier
        .payload
        .as_ref()
        .expect("unmerged incoming payload");
    assert_eq!(frontier.target.as_ref(), Some(&incoming.key()));
    assert_eq!(incoming.active().stack.len(), 1);
    assert!(incoming.active().stack[0].contains(U256::from(3)));
    let covered = analysis
        .states()
        .iter()
        .find(|state| state.key == incoming.key())
        .unwrap();
    assert!(!covered.entry.active().stack[0].contains(U256::from(3)));
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure)
    );
}

#[test]
fn new_unknown_source_does_not_match_old_unknown_content() {
    let domain = fixtures::config().domain().unwrap();
    let mut first = fixtures::payload(domain);
    first.active_mut().returndata = ByteArray::unknown();
    let mut second = first.clone();
    second.active_mut().returndata = ByteArray::unknown();
    let alternatives = EntryAlternatives::new(&first);
    let mut budget = WorkBudget::new(10_000_000);
    assert!(
        alternatives
            .join(&first, &second, domain, &mut budget)
            .unwrap()
            .is_some()
    );
    // An older rollback snapshot remains a complete, independent root too.
    let before = first.active().saved_store.clone();
    let mut modified = first.store.clone();
    modified
        .write_checked(
            fixtures::address(0x101),
            &fixtures::word(0),
            &fixtures::word(9),
            domain,
        )
        .unwrap();
    first.active_mut().saved_store = Snapshot::from_state(modified);
    assert_ne!(first.active().saved_store, before);
}

#[test]
fn scalar_counter_widening_retains_the_unchanged_immutable_input() {
    let mut config = fixtures::config();
    config.analysis.relations = crate::domain::relational::RelationLimits::default();
    config.max_work = crate::analysis::ExecutionConfig::default().max_work;
    // Original CALLVALUE is below an increasing counter throughout the loop.
    let (world, mut entry) = fixtures::fixture(&[(0x101, "345f5b600101600256")]);
    entry.environment.value = AbstractValue::top();
    let analysis = crate::analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.transfers() <= 5);
    let initial = analysis.states()[0].exit.as_ref().unwrap();
    let header = analysis
        .states()
        .iter()
        .find(|state| state.entry.active().stack.len() == 2)
        .unwrap();
    let original = &initial.active().stack[0];
    let preserved = &header.entry.active().stack[0];
    assert_eq!(preserved.numeric(), original.numeric());
    assert_eq!(preserved.expression(), original.expression());
    assert!(preserved.same_identity(original));
    assert!(header.entry.active().stack[1].contains(U256::ZERO));
    assert!(header.entry.active().stack[1].contains(U256::MAX));
    assert_eq!(
        header.entry.active().memory.state_root(),
        initial.active().memory.state_root()
    );
    assert_eq!(
        header.entry.active().saved_store,
        initial.active().saved_store
    );
    crate::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
}

#[test]
fn an_incoming_leaf_is_retained_when_the_old_guard_selects_a_different_root() {
    let domain = fixtures::config().domain().unwrap();
    let policy = domain.spec().relations();
    let guard = ExprId::fresh().unwrap();
    let mut old = fixtures::payload(domain);
    let mut seven = old.active().memory.clone();
    seven
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    let mut nine = old.active().memory.clone();
    nine.write_word(&fixtures::word(0), &fixtures::word(9), 4096, domain)
        .unwrap();
    old.active_mut().memory = seven
        .join_guarded(&nine, domain, guard.clone(), policy.expressions())
        .unwrap();
    let mut incoming = old.clone();
    incoming.active_mut().memory = seven;
    assert_eq!(
        incoming.relations.assume(&guard, false, &policy),
        CheckResult::Sat
    );
    let alternatives = EntryAlternatives::new(&old);
    let mut budget = WorkBudget::new(10_000_000);
    let joined = alternatives
        .join(&old, &incoming, domain, &mut budget)
        .unwrap()
        .expect("root leaf membership does not prove complete guarded coverage");
    let mut path = joined.relations.clone();
    assert_eq!(path.assume(&guard, false, &policy), CheckResult::Sat);
    let word = joined
        .active()
        .memory
        .read_word_checked(&fixtures::word(0), domain)
        .unwrap();
    assert_eq!(
        path.can_equal(word.expression().unwrap(), U256::from(7), &policy),
        CheckResult::Sat
    );
    assert_eq!(
        path.can_equal(word.expression().unwrap(), U256::from(9), &policy),
        CheckResult::Sat
    );
}
