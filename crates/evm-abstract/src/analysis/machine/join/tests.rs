mod conditional_normalization;

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
use std::collections::BTreeSet;

#[test]
fn actual_machine_join_keeps_zero_and_nonzero_branches_after_one_sided_world_proof() {
    use crate::analysis::{EdgeKind, MachineEdgeKind, transfer};
    use std::convert::Infallible;

    let mut config = fixtures::config();
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    let limits = domain.spec().relations().expressions();
    for opcode in [0x51u8, 0x54, 0x5c] {
        // MLOAD/SLOAD/TLOAD; ISZERO; JUMPI, followed by two marker returns.
        let code = format!("5f{opcode:02x}15600e5760015f5260205ff35b5f5f5260205ff3");
        let (world, entry) = fixtures::fixture(&[(0x101, &code)]);
        let mut right = transfer::initial(&world, &entry, domain).unwrap();
        let guard = ExprId::fresh().unwrap();
        if opcode == 0x51 {
            let mut yes = ByteArray::memory();
            let mut no = ByteArray::memory();
            yes.write_word(&fixtures::word(0), &fixtures::word(1), 64, domain)
                .unwrap();
            no.write_word(&fixtures::word(0), &fixtures::word(0), 64, domain)
                .unwrap();
            right.active_mut().memory = yes
                .join_guarded(&no, domain, guard.clone(), limits)
                .unwrap();
        } else {
            let mut yes = right.store.clone();
            let mut no = right.store.clone();
            for (store, value) in [(&mut yes, 1), (&mut no, 0)] {
                if opcode == 0x5c {
                    store
                        .write_transient_checked(
                            entry.address,
                            &fixtures::word(0),
                            &fixtures::word(value),
                            domain,
                        )
                        .unwrap();
                } else {
                    store
                        .write_checked(
                            entry.address,
                            &fixtures::word(0),
                            &fixtures::word(value),
                            domain,
                        )
                        .unwrap();
                }
            }
            right.store = yes
                .join_guarded(&no, domain, guard.clone(), limits)
                .unwrap();
            let saved = right.store.snapshot();
            right.active_mut().saved_store = saved;
        }
        let mut left = right.clone();
        // The left input represents the external path guard != 0; the right
        // retains the original unconstrained guard and therefore still has zero.
        let mut proof =
            |condition: &ExprId| Ok::<_, Infallible>((condition == &guard).then_some(true));
        assert!(
            left.active_mut()
                .memory
                .refine_guards(domain, limits, &mut proof)
                .unwrap()
        );
        assert!(
            left.store
                .refine_guards(domain, limits, &mut proof)
                .unwrap()
        );
        let saved = left.store.snapshot();
        left.active_mut().saved_store = saved;
        for (a, b) in [(&left, &right), (&right, &left)] {
            let joined = a.join(b, domain).unwrap();
            if opcode != 0x51 {
                let saved = joined.active().saved_store.state();
                let read = if opcode == 0x5c {
                    saved
                        .read_transient_checked(entry.address, &fixtures::word(0), domain)
                        .unwrap()
                } else {
                    saved
                        .read_checked(entry.address, &fixtures::word(0), domain)
                        .unwrap()
                };
                assert!(read.contains(U256::ZERO) && read.contains(U256::ONE));
            }
            let execution = transfer::execute(
                &world,
                &entry,
                joined,
                &config,
                domain,
                &mut WorkBudget::new(config.max_work),
                transfer::StorageReadPolicy::Abstract,
            );
            assert!(execution.frontiers.is_empty(), "{:?}", execution.frontiers);
            for branch in [EdgeKind::BranchTrue, EdgeKind::BranchFalse] {
                assert!(execution.successors.iter().any(|successor| successor.kind == MachineEdgeKind::Intraprocedural(branch)), "opcode {opcode:#x} lost {branch:?}");
            }
        }
    }
}

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
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        right.relations.assume(&predicate, false, &policy),
        CheckResult::MayBeFeasible
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
    // The shared pointer/root selection can already fold both reads to 7.
    assert_eq!(loaded.singleton(), Some(U256::from(7)), "{loaded:?}");
    let loaded_expression = loaded.symbolic_expression().unwrap();
    assert_eq!(
        joined.relations.local_value(&loaded_expression, &policy),
        ValueQuery::Exact(U256::from(7))
    );
    let mut chosen = joined.relations.clone();
    assert_eq!(
        chosen.assume(&selector, true, &policy),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        chosen.can_equal(&predicate, U256::ZERO, &policy),
        CheckResult::ProvenImpossible
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
fn growing_loop_bound_contains_the_concrete_counter_and_memory_pair() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 2;
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    // After two iterations the actual header input is (counter=3, word=2).
    // LoopBound may forget their pairing, but must include that joint state.
    let analysis = fixtures::run("60015b805f52600101600256", config);
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    let header = analysis
        .states()
        .iter()
        .find(|state| {
            state.program().unwrap().blocks()[state.active().basic_block_index].start_pc == 2
        })
        .expect("the real loop header");
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.from == header.id && edge.to == header.id)
    );
    assert_eq!(header.entry.active().stack.len(), 1);
    let counter = &header.entry.active().stack[0];
    assert!(counter.contains(U256::from(3)));
    assert_eq!(
        counter,
        &AbstractValue::from_numeric(counter.numeric().clone())
    );
    assert!(header.entry.relations.is_empty());
    let memory = &header.entry.active().memory;
    assert!(memory.is_memory());
    assert_eq!(
        memory.len().interval().unsigned_bounds(),
        (U256::ZERO, U256::from(32))
    );
    assert!(memory.len().contains(U256::from(32)));
    assert!(
        memory
            .read_word_checked(&fixtures::word(0), domain)
            .unwrap()
            .contains(U256::from(2))
    );

    // These checks establish a joint assignment, rather than relying on two
    // marginal contains() answers: the counter has no persistent identity/term,
    // the path is empty, and canonical memory has only independent input cells.
    // Its length's sole binding is a read of that same unobserved source.
    let length = memory
        .len()
        .expression()
        .expect("canonical non-singleton length");
    let ExprKind::SlotRead { state, key } = length.kind() else {
        panic!("canonical length must be a transparent observed input");
    };
    assert_eq!(key.as_constant(), Some(U256::ZERO));
    let SlotStateKind::Input {
        source: length_source,
        bindings: length_bindings,
    } = state.kind()
    else {
        panic!("canonical length source");
    };
    assert!(matches!(length_source.token().kind(), ExprKind::Fresh(_)));
    assert_eq!(length_bindings.len(), 1);
    let binding = &length_bindings[0];
    assert_eq!(binding.index, U256::ZERO);
    assert_eq!(&binding.numeric, memory.len().numeric());
    let ExprKind::SlotRead { state: raw, key } = binding.value.kind() else {
        panic!("length binding must not impose a cross-field equality");
    };
    assert_eq!(key.as_constant(), Some(U256::ZERO));
    assert_eq!(
        raw.kind(),
        &SlotStateKind::Input {
            source: length_source.clone(),
            bindings: vec![]
        }
    );
    let ByteStateKind::Input {
        source,
        length: input_length,
        bindings,
    } = memory.state_root().kind()
    else {
        panic!("the fixed point must retain independent canonical memory");
    };
    assert_eq!(input_length, length);
    assert!(matches!(source.token().kind(), ExprKind::Fresh(_)));
    let mut independent = BTreeSet::from([length_source.token().clone()]);
    assert!(independent.insert(source.token().clone()));
    let bytes = U256::from(2).to_be_bytes::<32>();
    for binding in bindings {
        let index = usize::try_from(binding.index).unwrap();
        assert!(index < bytes.len());
        assert!(matches!(binding.value.kind(), ExprKind::Fresh(_)));
        assert!(independent.insert(binding.value.clone()));
        assert!(
            AbstractValue::from_numeric(binding.numeric.clone()).contains(U256::from(bytes[index]))
        );
    }
    assert_eq!(memory.fresh_leaves(), independent);
    // Inspect the rest of this same payload for accidental shared sources.
    let mut outside_memory = header.entry.clone();
    outside_memory.active_mut().memory = ByteArray::memory();
    assert!(outside_memory.fresh_leaves().is_disjoint(&independent));
    assert!(
        memory
            .guard_conditions(domain.spec().relations().expressions())
            .unwrap()
            .is_empty()
    );
    for (expression, value) in memory.symbolic_facts() {
        assert_eq!(expression, length);
        assert_eq!(value.numeric(), memory.len().numeric());
    }
    for (index, value) in memory.stored_bytes() {
        assert!(value.expression().is_none());
        assert!(value.contains(U256::from(bytes.get(index).copied().unwrap_or(0))));
    }
    assert_eq!(
        memory.default_byte().numeric(),
        AbstractValue::unknown_byte().numeric()
    );
    let initial = &analysis.states()[0].entry;
    assert_eq!(
        header
            .entry
            .active()
            .saved_store
            .state()
            .summary_projection(domain)
            .unwrap(),
        initial
            .active()
            .saved_store
            .state()
            .summary_projection(domain)
            .unwrap()
    );
    assert_eq!(
        header.entry.store.summary_projection(domain).unwrap(),
        initial.store.summary_projection(domain).unwrap()
    );
    assert!(!analysis.outcomes().is_empty());
    assert!(
        analysis
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure)
    );
    crate::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
}

#[test]
fn acyclic_join_preserves_the_uncovered_incoming_state_at_its_policy_boundary() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 2;
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    // Three acyclic arms write and retain 1, 2 or 3, then meet at PC 39.
    // Changing memory rules out scalar-only widening; no feedback admits a
    // LoopBound before the third genuinely uncovered whole-state input.
    let code = "348015601457600114601e576003805f526027565b506001805f526027565b6002805f526027565b00";
    let (world, mut entry) = fixtures::fixture(&[(0x101, code)]);
    entry.environment.value = AbstractValue::top();
    let analysis = crate::analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert_eq!(analysis.frontiers().len(), 1);
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
    assert_eq!(incoming.active().stack[0].singleton(), Some(U256::from(3)));
    assert_eq!(
        incoming
            .active()
            .memory
            .read_word_checked(&fixtures::word(0), domain)
            .unwrap()
            .singleton(),
        Some(U256::from(3))
    );
    let covered = analysis
        .states()
        .iter()
        .find(|state| state.key == incoming.key())
        .unwrap();
    assert!(!covered.entry.active().stack[0].contains(U256::from(3)));
    assert!(
        !covered
            .entry
            .active()
            .memory
            .read_word_checked(&fixtures::word(0), domain)
            .unwrap()
            .contains(U256::from(3))
    );
    assert_eq!(
        covered.program().unwrap().blocks()[covered.active().basic_block_index].start_pc,
        39
    );
    assert!(!analysis.edges().iter().any(|edge| edge.from == covered.id));
    assert!(frontier.from.is_some());
    assert!(
        analysis
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return),
        "accepted arms still execute their real STOP"
    );
}

#[test]
fn new_unknown_source_does_not_match_old_unknown_content() {
    let domain = fixtures::config().domain().unwrap();
    let mut first = fixtures::payload(domain);
    first.active_mut().returndata = ByteArray::unknown();
    let mut second = first.clone();
    second.active_mut().returndata = ByteArray::unknown();
    let mut alternatives = EntryAlternatives::new(&first);
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
        CheckResult::MayBeFeasible
    );
    let mut alternatives = EntryAlternatives::new(&old);
    let mut budget = WorkBudget::new(10_000_000);
    let joined = alternatives
        .join(&old, &incoming, domain, &mut budget)
        .unwrap()
        .expect("root leaf membership does not prove complete guarded coverage");
    let mut path = joined.relations.clone();
    assert_eq!(
        path.assume(&guard, false, &policy),
        CheckResult::MayBeFeasible
    );
    let word = joined
        .active()
        .memory
        .read_word_checked(&fixtures::word(0), domain)
        .unwrap();
    assert_eq!(
        path.can_equal(word.expression().unwrap(), U256::from(7), &policy),
        CheckResult::MayBeFeasible
    );
    assert_eq!(
        path.can_equal(word.expression().unwrap(), U256::from(9), &policy),
        CheckResult::MayBeFeasible
    );
}

#[test]
fn canonical_case_rebuild_keeps_previously_accepted_stack_root_pairs() {
    let domain = fixtures::config().domain().unwrap();
    let policy = domain.spec().relations();
    let mut seven = fixtures::payload(domain);
    seven.active_mut().stack = vec![fixtures::word(0)];
    seven
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    let mut nine = seven.clone();
    nine.active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(9), 4096, domain)
        .unwrap();
    let mut budget = WorkBudget::new(100_000_000);
    let cases = super::cases::prepare(
        &[],
        Some(&seven),
        &nine,
        &BTreeSet::new(),
        domain,
        &mut budget,
    )
    .unwrap();
    assert_eq!(cases.len(), 2);
    let mut entry = cases[0].join(&cases[1], domain).unwrap();
    let mut bound = cases[0].fresh_leaves();
    bound.extend(cases[1].fresh_leaves());
    let mut selectors: BTreeSet<_> = entry.fresh_leaves().difference(&bound).cloned().collect();
    let mut alternatives = EntryAlternatives {
        represented: cases,
        cycle_widened: true,
        loop_bound: None,
    };
    let mut incoming = entry.clone();
    incoming.active_mut().stack = vec![fixtures::word(1)];
    let widened = alternatives
        .join_cyclic(&entry, &incoming, domain, &mut budget, &selectors, true)
        .unwrap()
        .unwrap();
    let mut previous = entry.fresh_leaves();
    previous.extend(incoming.fresh_leaves());
    selectors.extend(widened.fresh_leaves().difference(&previous).cloned());
    entry = widened;
    let mut last = seven;
    last.active_mut().stack = vec![fixtures::word(5)];
    last.active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(11), 4096, domain)
        .unwrap();
    let result = alternatives
        .join_cyclic(&entry, &last, domain, &mut budget, &selectors, true)
        .unwrap()
        .unwrap();
    let stack = result.active().stack[0].symbolic_expression().unwrap();
    let memory = result
        .active()
        .memory
        .read_word_checked(&fixtures::word(0), domain)
        .unwrap()
        .symbolic_expression()
        .unwrap();
    for (counter, word, expected) in [
        (1, 7, CheckResult::MayBeFeasible),
        (1, 9, CheckResult::MayBeFeasible),
        (5, 11, CheckResult::MayBeFeasible),
        // These joint candidate queries add no original-F result guarantee.
        // The local rules retain the shared selector without assignment search.
        (1, 11, CheckResult::MayBeFeasible),
        (5, 7, CheckResult::MayBeFeasible),
    ] {
        let a = ExprId::operation(
            revm_bytecode::opcode::EQ,
            &[stack.clone(), ExprId::constant(U256::from(counter))],
            policy.expressions(),
        )
        .unwrap();
        let b = ExprId::operation(
            revm_bytecode::opcode::EQ,
            &[memory.clone(), ExprId::constant(U256::from(word))],
            policy.expressions(),
        )
        .unwrap();
        let both =
            ExprId::operation(revm_bytecode::opcode::AND, &[a, b], policy.expressions()).unwrap();
        assert_eq!(
            result.relations.can_equal(&both, U256::from(1), &policy),
            expected,
            "({counter},{word})"
        );
    }
}

#[test]
fn cycle_projection_never_enumerates_external_sources_or_manual_guards() {
    let domain = fixtures::config().domain().unwrap();
    let policy = domain.spec().relations();
    let manual = ExprId::fresh().unwrap();
    let observation = ExprId::fresh().unwrap();
    let gas = AbstractValue::unsigned_range(U256::ZERO, U256::from(u64::MAX))
        .unwrap()
        .with_expression(observation.clone());
    let mut left = fixtures::payload(domain);
    left.active_mut().calldata = ByteArray::unknown();
    let input_sources = left.active().calldata.fresh_leaves();
    let mut seven = left.active().memory.clone();
    seven
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    let mut nine = seven.clone();
    nine.write_word(&fixtures::word(0), &fixtures::word(9), 4096, domain)
        .unwrap();
    left.active_mut().memory = seven
        .join_guarded(&nine, domain, manual.clone(), policy.expressions())
        .unwrap();
    left.active_mut()
        .memory
        .write_word(&fixtures::word(32), &gas, 4096, domain)
        .unwrap();
    left.active_mut().stack = vec![fixtures::word(0), gas.clone()];
    let mut right = left.clone();
    right.active_mut().stack[0] = fixtures::word(1);
    let joined = left.join(&right, domain).unwrap();
    let mut inputs = left.fresh_leaves();
    inputs.extend(right.fresh_leaves());
    let selectors = joined
        .fresh_leaves()
        .difference(&inputs)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(selectors.len(), 1);
    assert!(!selectors.contains(&manual));
    assert!(!selectors.contains(&observation));
    let mut budget = WorkBudget::new(100_000_000);
    let cases =
        super::cases::prepare(&[], Some(&joined), &right, &selectors, domain, &mut budget).unwrap();
    for payload in cases {
        assert_eq!(payload.active().calldata.fresh_leaves(), input_sources);
        assert!(payload.fresh_leaves().contains(&manual));
        assert!(payload.fresh_leaves().contains(&observation));
        assert_eq!(payload.active().stack[1].numeric(), gas.numeric());
        assert_eq!(payload.active().stack[1].expression(), Some(&observation));
        let (_, binding) = payload
            .active()
            .memory
            .symbolic_facts()
            .find(|(key, _)| *key == &observation)
            .expect("external GAS binding stays in the model");
        assert_eq!(binding.numeric(), gas.numeric());
    }
}

#[test]
fn a_constrained_entry_from_outside_a_cycle_keeps_its_actual_guard() {
    let domain = fixtures::config().domain().unwrap();
    let policy = domain.spec().relations();
    let mut seven = fixtures::payload(domain);
    seven.active_mut().stack = vec![fixtures::word(0)];
    seven
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    let mut nine = seven.clone();
    nine.active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(9), 4096, domain)
        .unwrap();
    let mut alternatives = EntryAlternatives::new(&seven);
    let mut entry = seven.join(&nine, domain).unwrap();
    let mut original = seven.fresh_leaves();
    original.extend(nine.fresh_leaves());
    let mut selectors: BTreeSet<_> = entry
        .fresh_leaves()
        .difference(&original)
        .cloned()
        .collect();
    alternatives.retain(nine);
    let mut incoming = seven.clone();
    incoming.active_mut().stack[0] = fixtures::word(1);
    let mut budget = WorkBudget::new(100_000_000);
    let canonical = alternatives
        .join_cyclic(&entry, &incoming, domain, &mut budget, &selectors, true)
        .unwrap()
        .unwrap();
    assert!(alternatives.cycle_widened);
    original = entry.fresh_leaves();
    original.extend(incoming.fresh_leaves());
    selectors.extend(canonical.fresh_leaves().difference(&original).cloned());
    entry = canonical;
    let guard = ExprId::fresh().unwrap();
    let mut guarded = seven;
    guarded.active_mut().stack[0] = fixtures::word(5);
    guarded
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(11), 4096, domain)
        .unwrap();
    assert_eq!(
        guarded.relations.assume(&guard, true, &policy),
        CheckResult::MayBeFeasible
    );
    let result = alternatives
        .join_cyclic(&entry, &guarded, domain, &mut budget, &selectors, false)
        .unwrap()
        .unwrap();
    assert!(!alternatives.cycle_widened);
    alternatives.retain(guarded.clone());
    assert_eq!(alternatives.represented[0].as_ref(), &entry);
    assert_eq!(alternatives.represented[1].as_ref(), &guarded);
    let stack = result.active().stack[0].symbolic_expression().unwrap();
    let memory = result
        .active()
        .memory
        .read_word_checked(&fixtures::word(0), domain)
        .unwrap()
        .symbolic_expression()
        .unwrap();
    for (truth, counter, word, expected) in [
        (true, 5, 11, CheckResult::MayBeFeasible),
        (false, 5, 11, CheckResult::MayBeFeasible),
        (false, 1, 7, CheckResult::MayBeFeasible),
        (true, 5, 7, CheckResult::MayBeFeasible),
    ] {
        let mut path = result.relations.clone();
        assert_eq!(
            path.assume(&guard, truth, &policy),
            CheckResult::MayBeFeasible
        );
        let equals = [(stack.clone(), counter), (memory.clone(), word)].map(|(value, constant)| {
            ExprId::operation(
                revm_bytecode::opcode::EQ,
                &[value, ExprId::constant(U256::from(constant))],
                policy.expressions(),
            )
            .unwrap()
        });
        let both =
            ExprId::operation(revm_bytecode::opcode::AND, &equals, policy.expressions()).unwrap();
        assert_eq!(path.can_equal(&both, U256::from(1), &policy), expected);
    }
}
