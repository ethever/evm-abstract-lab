//! Whole-entry admission, canonical joint witnesses and failed publication.

use crate::domain::ValueAlgebra;

use super::*;
use crate::{
    analysis::{FrameCode, MachinePayload},
    domain::{
        facts::FactError,
        relational::{Constraint, QueryReason, RelationLimits, RelationPathKind, RelationState},
        symbolic::{ExprError, StateError},
    },
    world::{RangeError, Store, StoreError},
};
use revm_bytecode::opcode;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn promotion_admits_only_typed_complete_precision_refusals() {
    let expression = ExprError::Limit { nodes: 9, depth: 3 };
    let state = StateError::Limit { nodes: 9, depth: 3 };
    let mut admitted = vec![
        MachineError::Alternatives { limit: 2 },
        MachineError::Expression(expression.clone()),
        MachineError::Store(StoreError::Expression(expression.clone())),
        MachineError::Store(StoreError::State(state.clone())),
        MachineError::Relations(QueryReason::ExpressionLimit),
        MachineError::Relations(QueryReason::ConstraintLimit),
    ];
    for bytes in [RangeError::Expression(expression), RangeError::State(state)] {
        admitted.push(MachineError::Bytes(bytes.clone()));
        admitted.push(MachineError::Store(StoreError::Bytes(bytes)));
    }
    for error in admitted {
        assert!(error.permits_bounded_promotion(), "{error:?}");
    }

    let mut rejected = vec![
        MachineError::Work,
        MachineError::States,
        MachineError::Shape,
        MachineError::IdentityExhausted,
    ];
    for reason in [
        QueryReason::Cancelled,
        QueryReason::Disabled,
        QueryReason::Configuration,
        QueryReason::ScalarFactLimit,
        QueryReason::ScalarFactError(FactError::UnsupportedOperation(0xfe)),
    ] {
        rejected.push(MachineError::Relations(reason));
    }
    let mut bytes = vec![
        RangeError::UnknownSize,
        RangeError::OffsetTooLarge(U256::MAX),
        RangeError::Overflow {
            offset: usize::MAX,
            size: 1,
        },
        RangeError::TooLarge {
            requested: U256::from(33),
            limit: 32,
        },
    ];
    for error in [
        ExprError::Cancelled,
        ExprError::Unsupported(0xfe),
        ExprError::Arity {
            opcode: 1,
            expected: 2,
            actual: 1,
        },
        // This has the same frontier classification as Limit, but is not a
        // refusal to retain a complete executable machine state.
        ExprError::ExportBytes {
            bytes: 33,
            limit: 32,
        },
    ] {
        rejected.push(MachineError::Expression(error.clone()));
        rejected.push(MachineError::Store(StoreError::Expression(error.clone())));
        bytes.push(RangeError::Expression(error));
    }
    for error in [StateError::Cancelled, StateError::IdentityExhausted] {
        rejected.push(MachineError::Store(StoreError::State(error.clone())));
        bytes.push(RangeError::State(error));
    }
    for error in bytes {
        rejected.push(MachineError::Bytes(error.clone()));
        rejected.push(MachineError::Store(StoreError::Bytes(error)));
    }
    for error in rejected {
        assert!(!error.permits_bounded_promotion(), "{error:?}");
    }
}

fn word_payload(base: &MachinePayload, word: u64, domain: Domain) -> MachinePayload {
    let mut payload = base.clone();
    payload.active_mut().stack = vec![fixtures::word(word)];
    payload.active_mut().memory = ByteArray::memory();
    payload
        .active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(word), 4096, domain)
        .unwrap();
    payload.normalize();
    payload
}

/// Inspect the existing canonical independent-cell representation, rather than
/// accepting two marginal bounds or evaluating arbitrary symbolic expressions.
/// The same assignment must satisfy the numeric stack and the complete memory,
/// with no surviving path, guard, input alias or external use of its sources.
pub(super) fn assert_joint_word_witness(payload: &MachinePayload, word: u64, domain: Domain) {
    let (assigned, _) = select_canonical_arm(payload, domain);
    let _ = canonical_word_assignment(&assigned, word, domain);
    let memory = &assigned.active().memory;
    assert!(
        memory
            .guard_conditions(domain.spec().relations().expressions())
            .unwrap()
            .is_empty()
    );
    for (expression, value) in memory.symbolic_facts() {
        let constant = expression
            .as_constant()
            .expect("canonical facts contain only literal range operands");
        assert!(value.contains(constant));
    }
    for (_, value) in memory.stored_bytes() {
        assert!(value.expression().is_none());
    }
}

fn select_canonical_arm(
    payload: &MachinePayload,
    domain: Domain,
) -> (MachinePayload, BTreeMap<ExprId, ExprId>) {
    // A later acyclic successor can retain an exact old/canonical disjunction.
    // Choose its incoming canonical arm using the production whole-payload
    // substitution, including all numeric/conditional caches. This supplies a
    // concrete selector assignment without an arbitrary expression evaluator.
    assert!(payload.relations.is_empty());
    let mut assigned = payload.clone();
    let mut selectors = BTreeMap::new();
    loop {
        let mut root = assigned.active().memory.state_root();
        while let ByteStateKind::Slice { source } = root.kind() {
            assert_eq!(source.offset.as_constant(), Some(U256::ZERO));
            assert_eq!(source.length.as_constant(), Some(U256::from(32)));
            root = &source.state;
        }
        let ByteStateKind::Ite { condition, .. } = root.kind() else {
            break;
        };
        assert!(matches!(condition.kind(), ExprKind::Fresh(_)));
        selectors.insert(condition.clone(), ExprId::constant(U256::ZERO));
        let assignment = BTreeMap::from([(condition.clone(), ExprId::constant(U256::ZERO))]);
        assigned
            .rename_fresh(&assignment, domain.spec().relations().expressions())
            .unwrap();
    }
    (assigned, selectors)
}

fn canonical_word_assignment(
    payload: &MachinePayload,
    word: u64,
    domain: Domain,
) -> BTreeMap<ExprId, ExprId> {
    let word = U256::from(word);
    assert!(payload.relations.is_empty());
    assert_eq!(payload.call_stack.depth(), 1);
    assert_eq!(payload.active().stack.len(), 1);
    let scalar = &payload.active().stack[0];
    assert_eq!(
        scalar,
        &AbstractValue::from_numeric(scalar.numeric().clone())
    );
    assert!(scalar.contains(word));
    let memory = &payload.active().memory;
    assert_eq!(memory.len().singleton(), Some(U256::from(32)));
    if let Some(expression) = memory.len().expression() {
        assert_eq!(expression.as_constant(), Some(U256::from(32)));
    }
    let bytes = word.to_be_bytes::<32>();
    let mut root = memory.state_root();
    while let ByteStateKind::Slice { source } = root.kind() {
        assert_eq!(source.offset.as_constant(), Some(U256::ZERO));
        assert_eq!(source.length.as_constant(), Some(U256::from(32)));
        root = &source.state;
    }
    // Canonical constant chunks impose only these literal bytes; they contain
    // no historical read/selector whose assignment could exclude this word.
    while let ByteStateKind::Copy {
        previous,
        destination,
        source,
        length,
    } = root.kind()
    {
        let start = usize::try_from(destination.as_constant().unwrap()).unwrap();
        let size = usize::try_from(length.as_constant().unwrap()).unwrap();
        assert_eq!(source.offset.as_constant(), Some(U256::ZERO));
        assert_eq!(&source.length, length);
        let ByteStateKind::Exact(chunk) = source.state.kind() else {
            panic!("canonical overlay must contain literal bytes");
        };
        assert_eq!(chunk.as_ref(), &bytes[start..start + size]);
        root = previous;
    }
    let ByteStateKind::Input {
        source,
        length,
        bindings,
    } = root.kind()
    else {
        panic!("bounded memory must have independent canonical cells");
    };
    assert_eq!(length.as_constant(), Some(U256::from(32)));
    assert!(matches!(source.token().kind(), ExprKind::Fresh(_)));
    let mut independent = BTreeSet::from([source.token().clone()]);
    let mut assignment = BTreeMap::new();
    for binding in bindings {
        let index = usize::try_from(binding.index).unwrap();
        assert!(matches!(binding.value.kind(), ExprKind::Fresh(_)));
        assert!(independent.insert(binding.value.clone()));
        assert!(binding.numeric.contains(U256::from(bytes[index])));
        assignment.insert(
            binding.value.clone(),
            ExprId::constant(U256::from(bytes[index])),
        );
    }
    assert_eq!(memory.fresh_leaves(), independent);
    let mut outside = payload.clone();
    outside.active_mut().memory = ByteArray::memory();
    assert!(outside.fresh_leaves().is_disjoint(&independent));
    for (index, value) in memory.stored_bytes() {
        assert!(value.contains(U256::from(bytes[index])));
    }
    let concrete = if memory.is_memory() {
        let mut concrete = ByteArray::memory();
        concrete
            .write_word(
                &fixtures::word(0),
                &AbstractValue::constant(word),
                4096,
                domain,
            )
            .unwrap();
        concrete
    } else {
        ByteArray::exact(&bytes)
    };
    assert!(
        memory
            .summary_projection(domain)
            .unwrap()
            .covers(&concrete.summary_projection(domain).unwrap(), domain,)
    );
    assignment
}

fn exact_local_value(expression: &ExprId, limits: &RelationLimits) -> U256 {
    match RelationState::default().local_value(expression, limits) {
        ValueQuery::Exact(value) => value,
        result => panic!(
            "grounded local query must be exact: {result:?}; expression={}",
            serde_json::to_string(expression).unwrap()
        ),
    }
}

fn assert_assigned_cache(
    value: &AbstractValue,
    expected: U256,
    assignment: &BTreeMap<ExprId, ExprId>,
    limits: &RelationLimits,
) {
    assert!(value.contains(expected));
    if let Some(expression) = value.expression() {
        assert_eq!(
            exact_local_value(
                &expression
                    .rename_fresh(assignment, limits.expressions())
                    .unwrap(),
                limits,
            ),
            expected
        );
    }
}

/// Exact successors refresh presentation cells and retain selected operand
/// facts under literal true. Prove the original metadata under one assignment
/// before ByteArray renaming can normalize constant-key numeric guarantees.
pub(super) fn assert_poststep_word_witness(
    payload: &MachinePayload,
    stage: &str,
    word: u64,
    call_value: u64,
    expected_store: &Store,
    domain: Domain,
) {
    // Only the read-only proof uses enabled local queries. The execution's
    // disabled relations, two-alternative policy and all budgets stay frozen.
    let limits = fixtures::config().analysis.relations;
    let memory = &payload.active().memory;
    let original_relations = memory.symbolic_relations(&limits).unwrap();
    let original_guards = memory.guard_conditions(limits.expressions()).unwrap();
    let (mut assigned, mut assignment) = select_canonical_arm(payload, domain);
    let selected_store = assigned.store.clone();
    for guard in &original_guards {
        match guard.kind() {
            ExprKind::Fresh(_) => {
                // Inactive historical selectors also receive a fixed value.
                assignment.insert(guard.clone(), ExprId::constant(U256::ZERO));
            }
            ExprKind::Constant(value) => assert_eq!(*value, U256::ONE),
            _ => panic!(
                "unexpected retained guard: {}",
                serde_json::to_string(guard).unwrap()
            ),
        }
    }
    for (cell, value) in canonical_word_assignment(&assigned, word, domain) {
        assert!(assignment.insert(cell, value).is_none());
    }
    let guards = assigned
        .active()
        .memory
        .guard_conditions(limits.expressions())
        .unwrap();
    assert_eq!(guards, BTreeSet::from([ExprId::constant(U256::ONE)]));
    let guard = guards.first().unwrap();
    assert_eq!(guard.nodes(), 1);
    assert_eq!(guard.depth(), 1);
    for guard in original_guards {
        let expected = if matches!(guard.kind(), ExprKind::Fresh(_)) {
            U256::ZERO
        } else {
            U256::ONE
        };
        assert_eq!(
            exact_local_value(
                &guard
                    .rename_fresh(&assignment, limits.expressions())
                    .unwrap(),
                &limits
            ),
            expected
        );
    }

    let word_value = U256::from(word);
    let bytes = word_value.to_be_bytes::<32>();
    assert_assigned_cache(memory.len(), U256::from(32), &assignment, &limits);
    assert_assigned_cache(memory.default_byte(), U256::ZERO, &assignment, &limits);
    for (index, value) in memory.stored_bytes() {
        assert_assigned_cache(value, U256::from(bytes[index]), &assignment, &limits);
    }
    for (expression, value) in memory.symbolic_facts() {
        let constant = exact_local_value(
            &expression
                .rename_fresh(&assignment, limits.expressions())
                .unwrap(),
            &limits,
        );
        assert_assigned_cache(value, constant, &assignment, &limits);
    }
    // These are exact reads from the original root under the SAME cell/selector
    // assignment, including every byte absent from the presentation cache.
    for (index, byte) in bytes.into_iter().enumerate() {
        let read = ExprId::byte_read(
            memory.state_root().clone(),
            ExprId::constant(U256::from(index)),
            limits.expressions(),
        )
        .unwrap()
        .rename_fresh(&assignment, limits.expressions())
        .unwrap();
        assert_eq!(exact_local_value(&read, &limits), U256::from(byte));
    }

    let retained = original_relations
        .rename_fresh(&assignment, limits.expressions())
        .unwrap();
    let mut expressions: Vec<_> = retained
        .constraints()
        .flat_map(Constraint::expressions)
        .collect();
    let mut pending = vec![retained.path()];
    while let Some(path) = pending.pop() {
        match path.kind() {
            RelationPathKind::True | RelationPathKind::False => {}
            RelationPathKind::Atoms(atoms) => {
                expressions.extend(atoms.iter().flat_map(Constraint::expressions));
            }
            RelationPathKind::And(children) => pending.extend(children),
            RelationPathKind::Ite {
                selector,
                then_path,
                else_path,
            } => {
                pending.push(if exact_local_value(selector, &limits) == U256::ZERO {
                    else_path
                } else {
                    then_path
                });
            }
        }
    }
    // Feasibility alone supplies no witness. Every active original atom and
    // selector must first have a determined, assumption-free local value.
    for expression in expressions {
        let _ = exact_local_value(expression, &limits);
    }
    assert_eq!(retained.check(&limits), CheckResult::MayBeFeasible);

    for (value, expected) in [
        (&payload.active().stack[0], word_value),
        (&payload.active().call_value, U256::from(call_value)),
    ] {
        assert_assigned_cache(value, expected, &assignment, &limits);
    }
    let value = &assigned.active().call_value;
    assert_eq!(value, &AbstractValue::from_numeric(value.numeric().clone()));
    assert!(value.contains(U256::from(call_value)));
    assigned
        .rename_fresh(&assignment, limits.expressions())
        .unwrap();
    // Numeric-only scalar cells have no aliases to the independent byte source.
    assigned.active_mut().stack[0] = AbstractValue::constant(word_value);
    assigned.active_mut().call_value = fixtures::word(call_value);
    // The complete selected Store is the witness for this SAME word/call-value
    // assignment; byte cells and retained historical guards cannot narrow it.
    assert_eq!(
        assigned.store, selected_store,
        "{stage}: joint assignment changed the Store witness"
    );
    let actual = assigned.store.summary_projection(domain).unwrap();
    let expected = expected_store.summary_projection(domain).unwrap();
    assert!(
        actual.covers(&expected, domain),
        "{stage}: word={word}, call_value={call_value}, is_memory={}, block={}; actual={actual:#?}\nexpected={expected:#?}",
        payload.active().memory.is_memory(),
        payload.active().key.basic_block_index,
    );
}

#[test]
fn joint_selector_witness_rejects_a_tuple_with_only_valid_marginals() {
    let domain = fixtures::config().domain().unwrap();
    let limits = fixtures::config().analysis.relations;
    let base = fixtures::payload(domain);
    let owner = fixtures::address(0x101);
    let [mut first, mut second] = [1, 2].map(|word| word_payload(&base, word, domain));
    for (payload, word) in [(&mut first, 1), (&mut second, 2)] {
        payload
            .store
            .write_checked(owner, &fixtures::word(0), &fixtures::word(word), domain)
            .unwrap();
    }
    let joined = first
        .join(&second, ValueAlgebra::new(domain).borrowed())
        .unwrap();
    let ByteStateKind::Ite { condition, .. } = joined.active().memory.state_root().kind() else {
        panic!("the complete join must share one selector");
    };
    let stack = joined.active().stack[0].symbolic_expression().unwrap();
    let memory = joined
        .active()
        .memory
        .read_word_checked(&fixtures::word(0), domain)
        .unwrap()
        .symbolic_expression()
        .unwrap();
    let slot = joined
        .store
        .read_checked(owner, &fixtures::word(0), domain)
        .unwrap()
        .symbolic_expression()
        .unwrap();
    for (selector, selected) in [(1u64, [1u64, 1, 1]), (0, [2, 2, 2])] {
        let assignment =
            BTreeMap::from([(condition.clone(), ExprId::constant(U256::from(selector)))]);
        for tuple in [[1, 1, 1], [2, 2, 2], [1, 2, 1]] {
            let equalities = [stack.clone(), memory.clone(), slot.clone()]
                .into_iter()
                .zip(tuple)
                .map(|(expression, value)| {
                    ExprId::operation(
                        opcode::EQ,
                        &[expression, ExprId::constant(U256::from(value))],
                        limits.expressions(),
                    )
                    .unwrap()
                });
            let joint = equalities
                .reduce(|left, right| {
                    ExprId::operation(opcode::AND, &[left, right], limits.expressions()).unwrap()
                })
                .unwrap()
                .rename_fresh(&assignment, limits.expressions())
                .unwrap();
            assert_eq!(
                exact_local_value(&joint, &limits),
                U256::from(u8::from(tuple == selected))
            );
        }
    }
}

#[test]
fn exact_retention_precedes_alternatives_promotion_of_both_complete_operands() {
    let mut config = fixtures::config();
    config.analysis.relations.enabled = false;
    config.analysis.relations.max_constraints = 2;
    let domain = config.domain().unwrap();
    let (world, environment) = fixtures::fixture(&[(0x101, "00")]);
    let base = crate::analysis::transfer::initial(
        &world,
        &environment,
        ValueAlgebra::new(domain).borrowed(),
    )
    .unwrap();
    let [first, second, third] = [1, 2, 3].map(|n| word_payload(&base, n, domain));
    let originals = [first.clone(), second.clone(), third.clone()];
    let mut alternatives = EntryAlternatives::new(&first);
    let mut budget = WorkBudget::new(100_000_000);
    let selectors = BTreeSet::new();
    let exact = alternatives
        .join_entry(
            &first,
            &second,
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &selectors,
            false,
        )
        .unwrap()
        .unwrap();
    assert!(!alternatives.has_loop_bound());
    assert_eq!(alternatives.represented.len(), 1);
    assert!(matches!(
        exact.active().memory.state_root().kind(),
        ByteStateKind::Ite { .. }
    ));
    alternatives.retain(second.clone());
    assert!(
        alternatives
            .join_entry(
                &exact,
                &second,
                &environment,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget,
                &selectors,
                false,
            )
            .unwrap()
            .is_none()
    );
    assert!(!alternatives.has_loop_bound());
    let promoted = alternatives
        .join_entry(
            &exact,
            &third,
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &selectors,
            false,
        )
        .unwrap()
        .unwrap();
    assert!(alternatives.has_loop_bound());
    assert!(alternatives.represented.is_empty());
    assert!(!alternatives.cycle_widened);
    for word in [1, 2, 3] {
        assert_joint_word_witness(&promoted, word, domain);
    }
    // Existing bounds use semantic coverage, without new roots per duplicate.
    let roots = promoted.fresh_leaves();
    for input in [&first, &second, &third, &promoted] {
        assert!(
            alternatives
                .join_entry(
                    &promoted,
                    input,
                    &environment,
                    ValueAlgebra::new(domain).borrowed(),
                    &mut budget,
                    &selectors,
                    false,
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(promoted.fresh_leaves(), roots);
    }
    assert_eq!([first, second, third], originals);
}

fn assert_unpublished(alternatives: &EntryAlternatives, before: &EntryAlternatives) {
    assert_eq!(alternatives.represented, before.represented);
    for (after, before) in alternatives.represented.iter().zip(&before.represented) {
        assert!(Arc::ptr_eq(after, before));
    }
    assert_eq!(alternatives.cycle_widened, before.cycle_widened);
    assert_eq!(alternatives.has_loop_bound(), before.has_loop_bound());
    assert_eq!(alternatives.work_size(), before.work_size());
}

#[test]
fn rejected_projection_or_materialization_is_atomic_and_keeps_paid_work() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 1;
    let domain = config.domain().unwrap();
    let (world, environment) = fixtures::fixture(&[(0x101, "00")]);
    let base = crate::analysis::transfer::initial(
        &world,
        &environment,
        ValueAlgebra::new(domain).borrowed(),
    )
    .unwrap();
    let old = word_payload(&base, 1, domain);
    let incoming = word_payload(&base, 2, domain);
    let inputs = (old.clone(), incoming.clone());
    let alternatives = EntryAlternatives::new(&old);
    let selectors = BTreeSet::new();
    let mut exact_work = WorkBudget::new(100_000_000);
    assert_eq!(
        alternatives.clone().join_cyclic(
            &old,
            &incoming,
            ValueAlgebra::new(domain).borrowed(),
            &mut exact_work,
            &selectors,
            false,
        ),
        Err(MachineError::Alternatives { limit: 1 })
    );
    let mut complete_work = WorkBudget::new(100_000_000);
    alternatives
        .clone()
        .join_entry(
            &old,
            &incoming,
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut complete_work,
            &selectors,
            false,
        )
        .unwrap()
        .unwrap();
    assert!(complete_work.used() > exact_work.used());
    for maximum in [0, exact_work.used(), complete_work.used() - 1] {
        let mut attempted = alternatives.clone();
        let mut budget = WorkBudget::new(maximum);
        assert_eq!(
            attempted.join_entry(
                &old,
                &incoming,
                &environment,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget,
                &selectors,
                false,
            ),
            Err(MachineError::Work)
        );
        assert!(budget.exhausted());
        if maximum > 0 {
            assert!(budget.used() >= exact_work.used());
        }
        assert_unpublished(&attempted, &alternatives);
        assert_eq!((&old, &incoming), (&inputs.0, &inputs.1));
    }
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut attempted = alternatives.clone();
        let mut budget = WorkBudget::new(100_000_000);
        token.cancel();
        assert_eq!(
            attempted.join_entry(
                &old,
                &incoming,
                &environment,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget,
                &selectors,
                false,
            ),
            Err(MachineError::Relations(QueryReason::Cancelled))
        );
        assert_unpublished(&attempted, &alternatives);
    });
    // A real bounded materialization refusal is still explicit after its one
    // paid coarse retry; no old entry or alternative proof is installed in part.
    config.analysis.relations.max_depth = 1;
    let mut attempted = alternatives.clone();
    let mut budget = WorkBudget::new(100_000_000);
    let error = attempted
        .join_entry(
            &old,
            &incoming,
            &environment,
            ValueAlgebra::new(config.domain().unwrap()).borrowed(),
            &mut budget,
            &selectors,
            false,
        )
        .unwrap_err();
    assert!(error.permits_bounded_promotion(), "{error:?}");
    assert!(budget.used() > exact_work.used());
    assert_unpublished(&attempted, &alternatives);
    assert_eq!((old, incoming), inputs);
}

#[test]
fn incompatible_and_unknown_frames_cannot_publish_a_bound() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 1;
    let domain = config.domain().unwrap();
    let (_, environment) = fixtures::fixture(&[]);
    let base = fixtures::payload(domain);
    for unknown in [false, true] {
        let mut old = word_payload(&base, 1, domain);
        let mut incoming = word_payload(&base, 2, domain);
        incoming.active_mut().program = None;
        if unknown {
            // Matching Unknown roles are rejected by projection itself, rather
            // than merely relying on a Runtime/Unknown shape mismatch.
            old.active_mut().code = FrameCode::Unknown;
            old.active_mut().program = None;
            incoming.active_mut().code = FrameCode::Unknown;
        }
        assert_eq!(old.key(), incoming.key());
        let inputs = (old.clone(), incoming.clone());
        let mut alternatives = EntryAlternatives::new(&old);
        let before = alternatives.clone();
        let mut budget = WorkBudget::new(100_000_000);
        assert_eq!(
            alternatives.join_entry(
                &old,
                &incoming,
                &environment,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget,
                &BTreeSet::new(),
                false,
            ),
            Err(MachineError::Shape)
        );
        assert_unpublished(&alternatives, &before);
        assert_eq!((old, incoming), inputs);
        assert!(budget.used() > 0);
    }
}

#[test]
fn an_existing_promoted_entry_keeps_its_old_proof_when_an_update_is_interrupted() {
    let mut config = fixtures::config();
    config.analysis.relations.enabled = false;
    config.analysis.relations.max_constraints = 1;
    let domain = config.domain().unwrap();
    let (_, environment) = fixtures::fixture(&[]);
    let base = fixtures::payload(domain);
    let [old, incoming, later] = [1, 2, 200].map(|n| word_payload(&base, n, domain));
    let mut alternatives = EntryAlternatives::new(&old);
    let mut budget = WorkBudget::new(100_000_000);
    let promoted = alternatives
        .join_entry(
            &old,
            &incoming,
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &BTreeSet::new(),
            false,
        )
        .unwrap()
        .unwrap();
    let before = alternatives.clone();
    let roots = promoted.fresh_leaves();
    for cancelled in [false, true] {
        let token = analysis_control::Cancellation::new();
        analysis_control::with_cancellation(&token, || {
            let mut budget = WorkBudget::new(if cancelled { 100_000_000 } else { 0 });
            if cancelled {
                token.cancel();
            }
            let expected = if cancelled {
                MachineError::Relations(QueryReason::Cancelled)
            } else {
                MachineError::Work
            };
            assert_eq!(
                alternatives.join_entry(
                    &promoted,
                    &later,
                    &environment,
                    ValueAlgebra::new(domain).borrowed(),
                    &mut budget,
                    &BTreeSet::new(),
                    false,
                ),
                Err(expected)
            );
            assert_unpublished(&alternatives, &before);
            assert_eq!(promoted.fresh_leaves(), roots);
        });
    }
    // The interrupted update did not replace the proof used for either old
    // operand or publish another executable materialization.
    for input in [&old, &incoming] {
        assert!(
            alternatives
                .join_entry(
                    &promoted,
                    input,
                    &environment,
                    ValueAlgebra::new(domain).borrowed(),
                    &mut budget,
                    &BTreeSet::new(),
                    false,
                )
                .unwrap()
                .is_none()
        );
    }
    assert_joint_word_witness(&promoted, 1, domain);
    assert_joint_word_witness(&promoted, 2, domain);
}

#[test]
fn expression_and_byte_state_retention_refusals_promote_at_the_actual_join() {
    let construction = fixtures::config().domain().unwrap();
    let (world, environment) = fixtures::fixture(&[(0x101, "00")]);
    let base = crate::analysis::transfer::initial(
        &world,
        &environment,
        ValueAlgebra::new(construction).borrowed(),
    )
    .unwrap();
    for scalar_expression in [false, true] {
        let mut old = word_payload(&base, 1, construction);
        let mut incoming = word_payload(&base, 2, construction);
        if scalar_expression {
            for payload in [&mut old, &mut incoming] {
                let expression = ExprId::operation(
                    revm_bytecode::opcode::ADD,
                    &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
                    construction.spec().relations().expressions(),
                )
                .unwrap();
                payload.active_mut().stack[0] = AbstractValue::top().with_expression(expression);
            }
        }
        let inputs = (old.clone(), incoming.clone());
        let mut config = fixtures::config();
        config.analysis.relations.max_depth = old.active().memory.state_root().depth();
        let domain = config.domain().unwrap();
        let mut alternatives = EntryAlternatives::new(&old);
        let mut budget = WorkBudget::new(100_000_000);
        let error = alternatives
            .clone()
            .join_cyclic(
                &old,
                &incoming,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget,
                &BTreeSet::new(),
                false,
            )
            .unwrap_err();
        if scalar_expression {
            assert!(
                matches!(error, MachineError::Expression(ExprError::Limit { .. })),
                "{error:?}"
            );
        } else {
            // check_existing retains the MSTORE cache's ByteRead expressions
            // before it constructs a combined byte root. Those reads are one
            // level deeper than the root used to freeze this fixture's limit.
            assert!(
                matches!(
                    error,
                    MachineError::Bytes(RangeError::Expression(ExprError::Limit { .. }))
                ),
                "{error:?}"
            );
        }
        assert!(error.permits_bounded_promotion());
        assert_eq!((&old, &incoming), (&inputs.0, &inputs.1));
        let promoted = alternatives
            .join_entry(
                &old,
                &incoming,
                &environment,
                ValueAlgebra::new(domain).borrowed(),
                &mut budget,
                &BTreeSet::new(),
                false,
            )
            .unwrap()
            .unwrap();
        assert!(alternatives.has_loop_bound());
        assert!(promoted.relations.is_empty());
        for input in [&old, &incoming] {
            assert!(
                promoted
                    .active()
                    .memory
                    .summary_projection(domain)
                    .unwrap()
                    .covers(
                        &input.active().memory.summary_projection(domain).unwrap(),
                        domain,
                    )
            );
        }
        assert!(
            promoted.active().memory.state_root().depth() <= config.analysis.relations.max_depth
        );
        assert!(promoted.fresh_leaves().is_disjoint(&old.fresh_leaves()));
        assert!(
            promoted
                .fresh_leaves()
                .is_disjoint(&incoming.fresh_leaves())
        );
        assert_eq!((old, incoming), inputs);
    }
}

#[test]
fn complete_path_constraint_refusal_can_forget_the_whole_path() {
    let construction = fixtures::config().domain().unwrap();
    let (_, environment) = fixtures::fixture(&[]);
    let base = fixtures::payload(construction);
    let mut old = word_payload(&base, 1, construction);
    let mut incoming = word_payload(&base, 2, construction);
    for payload in [&mut old, &mut incoming] {
        for _ in 0..2 {
            assert_eq!(
                payload.relations.assume(
                    &ExprId::fresh().unwrap(),
                    true,
                    &construction.spec().relations(),
                ),
                CheckResult::MayBeFeasible
            );
        }
    }
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 2;
    let domain = config.domain().unwrap();
    let mut alternatives = EntryAlternatives::new(&old);
    let mut budget = WorkBudget::new(100_000_000);
    assert_eq!(
        alternatives.clone().join_cyclic(
            &old,
            &incoming,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &BTreeSet::new(),
            false,
        ),
        Err(MachineError::Relations(QueryReason::ConstraintLimit))
    );
    let promoted = alternatives
        .join_entry(
            &old,
            &incoming,
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &BTreeSet::new(),
            false,
        )
        .unwrap()
        .unwrap();
    assert!(alternatives.has_loop_bound());
    assert!(promoted.relations.is_empty());
    assert!(promoted.fresh_leaves().is_disjoint(&old.fresh_leaves()));
    assert!(
        promoted
            .fresh_leaves()
            .is_disjoint(&incoming.fresh_leaves())
    );
    for input in [&old, &incoming] {
        assert!(
            promoted
                .active()
                .memory
                .summary_projection(domain)
                .unwrap()
                .covers(
                    &input.active().memory.summary_projection(domain).unwrap(),
                    domain,
                )
        );
        assert!(promoted.active().stack[0].contains(input.active().stack[0].singleton().unwrap()));
    }
}

#[test]
fn same_byte_roots_still_require_the_complete_old_metadata_proof() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 1;
    let domain = config.domain().unwrap();
    let (_, environment) = fixtures::fixture(&[]);
    let old = fixtures::payload(domain);
    let mut incoming = old.clone();
    let owner = fixtures::address(0x101);
    incoming.store.write_balance(owner, fixtures::word(42));
    let mut alternatives = EntryAlternatives::new(&old);
    let mut budget = WorkBudget::new(100_000_000);
    assert!(super::super::coverage::same_byte_roots(&old, &incoming, &mut budget).unwrap());
    let promoted = alternatives
        .join_entry(
            &old,
            &incoming,
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &BTreeSet::new(),
            false,
        )
        .unwrap()
        .expect("same roots do not cover different balances");
    assert!(alternatives.has_loop_bound());
    assert!(promoted.relations.is_empty());
    for input in [&old, &incoming] {
        assert!(
            promoted
                .store
                .summary_projection(domain)
                .unwrap()
                .covers(&input.store.summary_projection(domain).unwrap(), domain,)
        );
        assert!(
            promoted
                .active()
                .saved_store
                .state()
                .summary_projection(domain)
                .unwrap()
                .covers(
                    &input
                        .active()
                        .saved_store
                        .state()
                        .summary_projection(domain)
                        .unwrap(),
                    domain,
                )
        );
    }
}

#[test]
fn promotion_keeps_current_every_saved_frame_and_continuation_independent() {
    let mut config = fixtures::config();
    config.analysis.relations.max_constraints = 1;
    let domain = config.domain().unwrap();
    let (_, environment) = fixtures::fixture(&[]);
    let owner = environment.address;
    let base = fixtures::payload(domain);
    let mut inputs = Vec::new();
    for n in [1, 2] {
        let mut payload = word_payload(&base, n, domain);
        payload
            .store
            .write_checked(owner, &fixtures::word(0), &fixtures::word(n + 10), domain)
            .unwrap();
        let mut saved = base.store.clone();
        saved
            .write_checked(owner, &fixtures::word(0), &fixtures::word(n + 20), domain)
            .unwrap();
        payload.active_mut().saved_store = saved.snapshot();
        payload.active_mut().call_value = fixtures::word(n + 30);
        payload.active_mut().calldata = ByteArray::exact(&[n as u8]);
        payload.active_mut().returndata = ByteArray::exact(&[n as u8 + 1]);
        let mut child = RootFrame {
            state: payload.active().clone(),
        }
        .into_child(Continuation {
            return_block: Some(0),
            output_offset: fixtures::word(n + 40),
            output_size: fixtures::word(n + 50),
            creation: None,
        });
        saved
            .write_checked(owner, &fixtures::word(0), &fixtures::word(n + 60), domain)
            .unwrap();
        child.state.saved_store = saved.snapshot();
        payload.call_stack.push_child(child);
        payload.normalize();
        inputs.push(payload);
    }
    let mut alternatives = EntryAlternatives::new(&inputs[0]);
    let mut budget = WorkBudget::new(100_000_000);
    let promoted = alternatives
        .join_entry(
            &inputs[0],
            &inputs[1],
            &environment,
            ValueAlgebra::new(domain).borrowed(),
            &mut budget,
            &BTreeSet::new(),
            false,
        )
        .unwrap()
        .unwrap();
    assert!(alternatives.has_loop_bound());
    assert_eq!(promoted.key(), inputs[0].key());
    assert!(promoted.relations.is_empty());
    let mut roots = promoted.store.fresh_leaves();
    assert!(!roots.is_empty());
    for frame in promoted.call_stack.iter() {
        let saved = frame.saved_store.state().fresh_leaves();
        assert!(!saved.is_empty());
        assert!(saved.is_disjoint(&roots));
        roots.extend(saved);
    }
    for input in &inputs {
        assert!(
            promoted
                .store
                .summary_projection(domain)
                .unwrap()
                .covers(&input.store.summary_projection(domain).unwrap(), domain,)
        );
        for (frame, original) in promoted.call_stack.iter().zip(input.call_stack.iter()) {
            assert_eq!(frame.program, original.program);
            assert_eq!(frame.code, original.code);
            assert!(
                frame
                    .saved_store
                    .state()
                    .summary_projection(domain)
                    .unwrap()
                    .covers(
                        &original
                            .saved_store
                            .state()
                            .summary_projection(domain)
                            .unwrap(),
                        domain,
                    )
            );
            for (bytes, original) in [
                (&frame.memory, &original.memory),
                (&frame.calldata, &original.calldata),
                (&frame.returndata, &original.returndata),
            ] {
                assert!(
                    bytes
                        .summary_projection(domain)
                        .unwrap()
                        .covers(&original.summary_projection(domain).unwrap(), domain)
                );
                assert!(bytes.fresh_leaves().is_disjoint(&roots));
            }
            assert!(
                frame
                    .call_value
                    .contains(original.call_value.singleton().unwrap())
            );
        }
        let continuation = &promoted.call_stack.active_child().unwrap().continuation;
        let original = &input.call_stack.active_child().unwrap().continuation;
        assert_eq!(continuation.return_block, original.return_block);
        assert_eq!(continuation.creation, original.creation);
        for (value, original) in [
            (&continuation.output_offset, &original.output_offset),
            (&continuation.output_size, &original.output_size),
        ] {
            assert_eq!(value, &AbstractValue::from_numeric(value.numeric().clone()));
            assert!(value.contains(original.singleton().unwrap()));
        }
    }
}
