use crate::domain::ValueAlgebra;
mod bounded_promotion;
mod byte_roots;
mod conditional_normalization;

use super::{EntryAlternatives, MachineError};
use crate::{
    analysis::{
        Continuation, OutcomeKind, RootFrame, Status, engine::policy::RecentDistinctJumps,
        machine::fixtures,
    },
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
        let mut right =
            transfer::initial(&world, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
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
            let joined = a.join(b, ValueAlgebra::new(domain).borrowed()).unwrap();
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
                ValueAlgebra::new(domain).borrowed(),
                &RecentDistinctJumps,
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
        frame.address_operand = Some(fixtures::word(marker + 0x100));
        frame.caller_operand = Some(fixtures::word(marker + 0x200));
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
    let joined = left
        .join(&right, ValueAlgebra::new(domain).borrowed())
        .unwrap();
    let selector = scalar_guard(&joined.active().stack[0]);
    for frame in joined.call_stack.iter() {
        assert_eq!(scalar_guard(&frame.stack[0]), selector);
        assert_eq!(scalar_guard(&frame.call_value), selector);
        assert_eq!(
            scalar_guard(frame.address_operand.as_ref().unwrap()),
            selector
        );
        assert_eq!(
            scalar_guard(frame.caller_operand.as_ref().unwrap()),
            selector
        );
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
    let failure = left
        .join(
            &right,
            ValueAlgebra::new(config.domain().unwrap()).borrowed(),
        )
        .unwrap_err();
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
        .join(
            &first,
            &second,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
        )
        .unwrap()
        .unwrap();
    alternatives.retain(second.clone());
    let identities = joined.fresh_leaves();
    assert!(
        alternatives
            .join(
                &joined,
                &second,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(joined.fresh_leaves(), identities);
    assert_eq!(
        first
            .join(&first, ValueAlgebra::new(domain).borrowed())
            .unwrap(),
        first
    );
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
    assert_eq!(
        binding.numeric,
        crate::domain::WordObservation::from(memory.len().numeric())
    );
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
        assert!(binding.numeric.contains(U256::from(bytes[index])));
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
    for (actual, input) in [
        (
            header.entry.active().saved_store.state(),
            initial.active().saved_store.state(),
        ),
        (&header.entry.store, &initial.store),
    ] {
        let summary = actual.summary_projection(domain).unwrap();
        // initial <= header: covers joins every numerical/default, lifecycle
        // and log field. Must-independence joins by AND, so the header may
        // forget true -> false when LoopBound projects to its fixed layout.
        assert!(summary.covers(&input.summary_projection(domain).unwrap(), domain));

        // Bound that relaxation to must-independence: all public transaction
        // coordinates remain exact, including unlisted-account defaults.
        let owner = fixtures::address(0x101);
        let unlisted = fixtures::address(0x202);
        assert_eq!(input.addresses(), BTreeSet::from([owner]));
        assert_eq!(actual.addresses(), input.addresses());
        assert!(input.slots().is_empty() && input.transient_slots().is_empty());
        assert_eq!(actual.slots(), input.slots());
        assert_eq!(actual.transient_slots(), input.transient_slots());
        assert_eq!(
            actual.global_storage_default().numeric(),
            input.global_storage_default().numeric()
        );
        assert_eq!(
            actual.global_transient_default().numeric(),
            input.global_transient_default().numeric()
        );
        assert_eq!(
            actual.balance_default().numeric(),
            input.balance_default().numeric()
        );
        for address in [owner, unlisted] {
            assert_eq!(
                actual.storage_default(address).numeric(),
                input.storage_default(address).numeric()
            );
            assert_eq!(
                actual.transient_default(address).numeric(),
                input.transient_default(address).numeric()
            );
            assert_eq!(
                actual.read_balance(address).numeric(),
                input.read_balance(address).numeric()
            );
            assert_eq!(
                actual.nonce(address).numeric(),
                input.nonce(address).numeric()
            );
            assert_eq!(actual.code(address), input.code(address));
            assert_eq!(actual.existence(address), input.existence(address));
            assert_eq!(
                actual.created_in_transaction(address),
                input.created_in_transaction(address)
            );
            assert_eq!(
                actual.pending_destruction(address),
                input.pending_destruction(address)
            );
            assert_eq!(
                actual.touched_in_transaction(address),
                input.touched_in_transaction(address)
            );
            assert_eq!(
                actual.slot_root(address, true).unwrap().kind(),
                &SlotStateKind::Zero
            );
        }
        assert_eq!(actual.touched_default(), input.touched_default());
        assert!(input.possible_logs().is_empty() && !input.logs_unknown());
        assert_eq!(actual.possible_logs(), input.possible_logs());
        assert_eq!(actual.logs_unknown(), input.logs_unknown());
        assert_eq!(
            actual.slot_root(owner, false).unwrap().kind(),
            &SlotStateKind::Zero
        );
        let mut unlisted_source = None;
        for address in [fixtures::address(0), unlisted] {
            let root = actual.slot_root(address, false).unwrap();
            let SlotStateKind::Input { source, bindings } = root.kind() else {
                panic!("unlisted persistent accounts retain an unconstrained input");
            };
            assert!(bindings.is_empty());
            assert!(matches!(source.token().kind(), ExprKind::Fresh(_)));
            assert_eq!(
                source.namespace(),
                U256::from_be_slice(address.as_slice()) << 8
            );
            if let Some(token) = &unlisted_source {
                assert_eq!(source.token(), token);
            } else {
                unlisted_source = Some(source.token().clone());
            }
        }
        assert!(actual.symbolic_facts(false).next().is_none());
        assert!(actual.symbolic_facts(true).next().is_none());

        // Each alternative starts at the original captured store, changing
        // one transaction coordinate. Full coverage must still reject it.
        let reject = |alternative: crate::world::Store| {
            assert!(!summary.covers(&alternative.summary_projection(domain).unwrap(), domain));
        };
        let mut alternative = input.clone();
        alternative
            .write_checked(owner, &fixtures::word(0), &fixtures::word(1), domain)
            .unwrap();
        reject(alternative);
        for address in [owner, unlisted] {
            let mut alternative = input.clone();
            alternative
                .write_transient_checked(address, &fixtures::word(0), &fixtures::word(1), domain)
                .unwrap();
            reject(alternative);
        }
        let mut alternative = input.clone();
        alternative.write_balance(owner, fixtures::word(1_000_001));
        reject(alternative);
        let mut alternative = input.clone();
        alternative.write_nonce(owner, fixtures::word(1));
        reject(alternative);
        let mut alternative = input.clone();
        alternative.deploy_code(owner, crate::bytecode::Program::from_hex("00").unwrap());
        reject(alternative);
        let mut alternative = input.clone();
        alternative.touch_account(unlisted);
        reject(alternative);
        let mut alternative = input.clone();
        alternative.may_touch_all();
        reject(alternative);
        let mut alternative = input.clone();
        alternative
            .emit_log(
                crate::world::LogKey {
                    address: owner,
                    code_address: owner,
                    code_hash: header.entry.active().key.code_hash,
                    source_kind: crate::world::LogSourceKind::Runtime,
                    pc: 0,
                },
                crate::world::AbstractLog {
                    topics: vec![fixtures::word(1)],
                    data: ByteArray::empty(),
                },
                domain,
            )
            .unwrap();
        reject(alternative);
    }
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
    // Changing memory rules out scalar-only widening. The third complete
    // input promotes the whole entry, then really jumps to PC 43 and returns.
    let code = "348015601457600114601e576003805f526027565b506001805f526027565b6002805f526027565b602b565b60205ff3";
    let (world, mut entry) = fixtures::fixture(&[(0x101, code)]);
    entry.environment.value = AbstractValue::top();
    let analysis = crate::analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    assert!(analysis.states().len() <= 8);
    assert!(analysis.transfers() <= 16);
    let at_pc = |pc| {
        analysis
            .states()
            .iter()
            .find(|state| {
                state.program().unwrap().blocks()[state.active().basic_block_index].start_pc == pc
            })
            .unwrap()
    };
    let covered = at_pc(39);
    let successor = at_pc(43);
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.from == covered.id && edge.to == successor.id)
    );
    assert!(
        analysis
            .edges()
            .iter()
            .filter(|edge| edge.to == covered.id)
            .all(|edge| {
                let state = &analysis.states()[edge.from];
                matches!(
                    state.program().unwrap().blocks()[state.active().basic_block_index].start_pc,
                    12 | 20 | 30
                )
            })
    );
    let exit = successor.exit.as_ref().expect("known successor executed");
    // One final outcome must cover ALL three concrete joint assignments.
    // The entry is canonical; exact successors and RETURN also carry refreshed
    // presentation expressions and selected, scoped operand guarantees.
    let outcome = analysis
        .outcomes()
        .iter()
        .rev()
        .find(|outcome| outcome.state == successor.id && outcome.kind == OutcomeKind::Return)
        .expect("real RETURN from the known successor");
    let mut returned = exit.clone();
    returned.active_mut().memory = outcome.data.clone();
    returned.store = outcome.store.clone();
    // Entry and exit retain the entered transaction's current Store. Only the
    // actual root RETURN commits it; saved_store remains pre-entry rollback history.
    let initial_current_store = &analysis.states()[0].entry.store;
    let mut finalized_store = initial_current_store.clone();
    finalized_store.finalize_transaction(domain).unwrap();
    for (word, call_value) in [(1, 0), (2, 1), (3, 2)] {
        bounded_promotion::assert_joint_word_witness(&covered.entry, word, domain);
        for (stage, payload, expected_store) in [
            ("successor-entry", &successor.entry, initial_current_store),
            ("successor-exit", exit, initial_current_store),
            ("actual-return", &returned, &finalized_store),
        ] {
            bounded_promotion::assert_poststep_word_witness(
                payload,
                stage,
                word,
                call_value,
                expected_store,
                domain,
            );
        }
        for payload in [&covered.entry, &successor.entry, exit, &returned] {
            let value = &payload.active().call_value;
            assert_eq!(value, &AbstractValue::from_numeric(value.numeric().clone()));
            assert!(value.contains(U256::from(call_value)));
            assert!(payload.work_size() < 8192);
        }
    }
    assert!(
        outcome
            .store
            .summary_projection(domain)
            .unwrap()
            .covers(&finalized_store.summary_projection(domain).unwrap(), domain,)
    );
    crate::ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
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
            .join(
                &first,
                &second,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget
            )
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
fn scalar_counter_widening_covers_immutable_input_with_independent_roots() {
    let mut config = fixtures::config();
    config.analysis.relations = crate::domain::relational::RelationLimits::default();
    config.max_work = crate::analysis::ExecutionConfig::default().max_work;
    let domain = config.domain().unwrap();
    // Original CALLVALUE is below an increasing counter throughout the loop.
    let (world, mut entry) = fixtures::fixture(&[(0x101, "345f5b600101600256")]);
    entry.environment.value = AbstractValue::top();
    let analysis = crate::analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    // One entry transfer, then header inputs {0}, {0,1}, {0,1,2},
    // [0,2^255-1], and Top. EntryAlternatives retains two inputs before
    // LoopBound; widen_procedure retains at most widening_after_updates()+1
    // (three) constants. The fourth constant widens the positive signed hull;
    // the next ADD crosses the sign boundary and widens to the full word.
    // The final Top transfer is the cyclic postfixpoint check: update returns
    // None on semantic equality, so Engine does not queue another transfer.
    assert_eq!(domain.spec().widening_after_updates(), 2);
    assert!(analysis.transfers() <= 6);
    assert_eq!(analysis.states().len(), 2);
    let initial = analysis.states()[0].exit.as_ref().unwrap();
    let header = analysis
        .states()
        .iter()
        .find(|state| state.entry.active().stack.len() == 2)
        .unwrap();
    let original = &initial.active().stack[0];
    let preserved = &header.entry.active().stack[0];
    assert_eq!(preserved.numeric(), original.numeric());
    assert!(original.numeric().is_top());
    let original_input = original
        .identity()
        .input()
        .expect("immutable CALLVALUE input");
    assert_eq!(original_input.symbol(), crate::world::Symbol::CallValue);
    let original_expression = original.expression().expect("original scoped input");
    let ExprKind::Input(atom) = original_expression.kind() else {
        panic!("the original CALLVALUE must retain its input metadata");
    };
    assert_eq!(atom.scope, original_input.scope());
    assert_eq!(atom.symbol, original_input.symbol());
    assert_eq!(
        original.provenance(),
        &crate::domain::provenance::Provenance::source(
            crate::domain::provenance::Origin::CallValue
        )
    );
    assert!(original.same_identity(&initial.active().call_value));

    // Generic LoopBound independently instantiates scalar roots. Preserve the
    // concrete CALLVALUE=7, counter=3 assignment without claiming a must-alias
    // between the new scalar and the historical immutable input.
    let counter = &header.entry.active().stack[1];
    let mut independent = BTreeSet::new();
    for value in [preserved, counter] {
        let expression = value.expression().expect("independent scalar root");
        assert!(matches!(expression.kind(), ExprKind::Fresh(_)));
        assert!(expression.inputs().is_empty());
        assert_eq!(
            value,
            &AbstractValue::top().with_expression(expression.clone())
        );
        assert!(!value.same_identity(original));
        assert!(independent.insert(expression.clone()));
    }
    assert!(original.contains(U256::from(7)));
    assert!(preserved.contains(U256::from(7)));
    assert!(counter.contains(U256::from(3)));
    assert!(header.entry.relations.is_empty());
    assert_eq!(header.entry.call_stack.depth(), 1);
    assert_eq!(header.entry.active().stack.len(), 2);
    assert_eq!(
        header.program().unwrap().blocks()[header.active().basic_block_index].start_pc,
        2
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.from == header.id && edge.to == header.id)
    );
    let mut outside_stack = header.entry.clone();
    outside_stack.active_mut().stack.clear();
    // This traversal includes frame scalars, byte sources, current/saved stores,
    // all continuations and the path. Neither assigned Fresh occurs elsewhere,
    // so no retained equality, guard or numeric root can rule out the pair.
    assert!(outside_stack.fresh_leaves().is_disjoint(&independent));
    assert_eq!(
        header.entry.active().call_value.numeric(),
        original.numeric()
    );
    let call_value = &header.entry.active().call_value;
    let call_value_expression = call_value
        .expression()
        .expect("independent frame CALLVALUE");
    assert!(matches!(call_value_expression.kind(), ExprKind::Fresh(_)));
    assert_eq!(
        call_value,
        &AbstractValue::top().with_expression(call_value_expression.clone())
    );
    for (operand, value) in [
        (&header.entry.active().address_operand, 0x101u64),
        (&header.entry.active().caller_operand, 0x900),
    ] {
        let operand = operand.as_ref().expect("canonical frame operand");
        assert_eq!(operand.singleton(), Some(U256::from(value)));
        if let Some(expression) = operand.expression() {
            assert!(!independent.contains(expression));
            if let Some(literal) = expression.as_constant() {
                assert_eq!(literal, U256::from(value));
            }
        }
    }
    assert!(header.entry.active().stack[1].contains(U256::ZERO));
    assert!(header.entry.active().stack[1].contains(U256::MAX));
    assert_eq!(
        header.entry.active().memory.state_root(),
        initial.active().memory.state_root()
    );
    for bytes in [
        &header.entry.active().memory,
        &header.entry.active().calldata,
        &header.entry.active().returndata,
    ] {
        assert_eq!(bytes.len().singleton(), Some(U256::ZERO));
        assert_eq!(bytes.state_root().kind(), &ByteStateKind::Zero);
        assert!(
            bytes
                .guard_conditions(domain.spec().relations().expressions())
                .unwrap()
                .is_empty()
        );
    }
    for (actual, input) in [
        (
            header.entry.active().saved_store.state(),
            initial.active().saved_store.state(),
        ),
        (&header.entry.store, &initial.store),
    ] {
        let summary = actual.summary_projection(domain).unwrap();
        // input <= header, including the saved rollback content; independently
        // sampled roots may forget must-independence, whose join is AND.
        assert!(summary.covers(&input.summary_projection(domain).unwrap(), domain));
        let owner = fixtures::address(0x101);
        assert_eq!(actual.addresses(), BTreeSet::from([owner]));
        assert_eq!(
            actual.slot_root(owner, false).unwrap().kind(),
            &SlotStateKind::Zero
        );
        let unlisted = fixtures::address(0x202);
        for address in [owner, unlisted] {
            assert_eq!(
                actual.slot_root(address, true).unwrap().kind(),
                &SlotStateKind::Zero
            );
        }
        let mut unlisted_source = None;
        for address in [fixtures::address(0), unlisted] {
            let root = actual.slot_root(address, false).unwrap();
            let SlotStateKind::Input { source, bindings } = root.kind() else {
                panic!("unlisted persistent accounts retain an unconstrained input");
            };
            assert!(bindings.is_empty());
            assert!(matches!(source.token().kind(), ExprKind::Fresh(_)));
            assert_eq!(
                source.namespace(),
                U256::from_be_slice(address.as_slice()) << 8
            );
            if let Some(token) = &unlisted_source {
                assert_eq!(source.token(), token);
            } else {
                unlisted_source = Some(source.token().clone());
            }
        }
        assert!(actual.symbolic_facts(false).next().is_none());
        assert!(actual.symbolic_facts(true).next().is_none());
        let mut alternative = input.clone();
        alternative
            .write_checked(owner, &fixtures::word(0), &fixtures::word(1), domain)
            .unwrap();
        assert!(!summary.covers(&alternative.summary_projection(domain).unwrap(), domain));
        let mut alternative = input.clone();
        alternative.write_balance(owner, fixtures::word(1_000_001));
        assert!(!summary.covers(&alternative.summary_projection(domain).unwrap(), domain));
        let mut alternative = input.clone();
        alternative.touch_account(fixtures::address(0x202));
        assert!(!summary.covers(&alternative.summary_projection(domain).unwrap(), domain));
    }
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
        .join(
            &old,
            &incoming,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
        )
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
        ValueAlgebra::new(domain).borrowed(),
        &mut budget,
    )
    .unwrap();
    assert_eq!(cases.len(), 2);
    let mut entry = cases[0]
        .join(&cases[1], ValueAlgebra::new(domain).borrowed())
        .unwrap();
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
        .join_cyclic(
            &entry,
            &incoming,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &selectors,
            true,
        )
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
        .join_cyclic(
            &entry,
            &last,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &selectors,
            true,
        )
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
    let joined = left
        .join(&right, ValueAlgebra::new(domain).borrowed())
        .unwrap();
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
    let cases = super::cases::prepare(
        &[],
        Some(&joined),
        &right,
        &selectors,
        ValueAlgebra::new(domain).borrowed(),
        &mut budget,
    )
    .unwrap();
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
    let mut entry = seven
        .join(&nine, ValueAlgebra::new(domain).borrowed())
        .unwrap();
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
        .join_cyclic(
            &entry,
            &incoming,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &selectors,
            true,
        )
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
        .join_cyclic(
            &entry,
            &guarded,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &selectors,
            false,
        )
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
