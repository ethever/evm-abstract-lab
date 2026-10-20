use super::{MachineError, Policy, Projection};
use crate::domain::ValueAlgebra;
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{address, payload, word},
    },
    domain::{
        AbstractValue, Domain, NumericValue, WordElement, WordObservation,
        relational::{RelationLimits, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn bounded_relations(expressions: &[ExprId], limits: &RelationLimits) -> RelationState {
    let mut relations = RelationState::default();
    for expression in expressions {
        relations
            .observe(
                expression,
                &NumericValue::unsigned_range(U256::from(5), U256::from(15)).unwrap(),
                limits,
            )
            .unwrap();
    }
    relations
}

// This fixture's retained scalar fact is exactly the range5..15. Derive its
// detached intersection independently, then use the two public quote phases;
// no Projection execution or measured total is used as the reference.
fn refinement_fees(
    relations: &RelationState,
    expression: &ExprId,
    value: &AbstractValue,
    domain: Domain,
) -> (usize, usize) {
    let algebra = ValueAlgebra::new(domain);
    let view = algebra.borrowed();
    let import = relations
        .refinement_import_work_with(expression, value, view)
        .unwrap();
    let imported = value
        .numeric()
        .constrain(&WordObservation::unsigned_range(U256::from(5), U256::from(15)).unwrap());
    (
        import,
        imported.map_or(0, |numeric| view.numeric_reduction_work(&numeric)),
    )
}

#[test]
fn summary_reuse_and_branch_feasibility_keep_their_distinct_query_contracts() {
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let raw = AbstractValue::top().with_expression(expression.clone());
    let (import, closure) = refinement_fees(&relations, &expression, &raw, domain);
    let fee = expression.work_size() + relations.work_size() + import + closure;
    for summary in [false, true] {
        let mut budget = WorkBudget::new(2 * fee);
        let mut diagnostics = Vec::new();
        let policy = if summary {
            Policy::Summary
        } else {
            Policy::Branch {
                diagnostics: &mut diagnostics,
                pc: 17,
            }
        };
        let mut projection =
            Projection::new(&relations, algebra.borrowed(), &limits, &mut budget, policy);
        for _ in 0..2 {
            let mut value = raw.clone();
            projection.value(&mut value);
            assert_eq!(
                value.interval().unsigned_bounds(),
                (U256::from(5), U256::from(15))
            );
            assert_eq!(value.expression(), raw.expression());
        }
        assert_eq!(projection.finish(), Ok(true));
        assert_eq!(budget.used(), if summary { fee } else { 2 * fee });
        assert!(diagnostics.is_empty());

        let mut contradictory = AbstractValue::unsigned_range(U256::from(20), U256::from(30))
            .unwrap()
            .with_expression(expression.clone());
        let before = contradictory.clone();
        let mut budget = WorkBudget::new(usize::MAX);
        let policy = if summary {
            Policy::Summary
        } else {
            Policy::Branch {
                diagnostics: &mut diagnostics,
                pc: 17,
            }
        };
        let mut projection =
            Projection::new(&relations, algebra.borrowed(), &limits, &mut budget, policy);
        projection.value(&mut contradictory);
        assert_eq!(projection.finish(), Ok(summary));
        assert_eq!(contradictory, before);
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn whole_projection_and_independent_replay_preserve_distinct_rollback_versions() {
    use crate::analysis::{ChildFrame, Continuation, machine::fixtures::fixture, state, transfer};
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input =
        transfer::initial(&world, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
    let expression = ExprId::fresh().unwrap();
    let value = AbstractValue::top().with_expression(expression.clone());
    let owner = address(0x101);
    input
        .store
        .write_checked(owner, &word(1), &value, domain)
        .unwrap();
    input.active_mut().stack = vec![value.clone()];
    input.active_mut().saved_store = input.store.snapshot();
    input
        .store
        .write_checked(owner, &word(1), &word(77), domain)
        .unwrap();
    let mut child = input.active().clone();
    child.key.caller = address(0x202).into();
    child.caller_operand = Some(word(0x202));
    child.saved_store = input.store.snapshot();
    input.call_stack.push_child(ChildFrame {
        state: child,
        continuation: Continuation {
            return_block: None,
            output_offset: value.clone(),
            output_size: word(32),
            creation: None,
        },
    });
    input
        .store
        .write_checked(owner, &word(1), &value, domain)
        .unwrap();
    input.relations = bounded_relations(&[expression], &domain.spec().relations());
    let original = input.clone();
    let limits = domain.spec().relations();
    let mut budget = WorkBudget::new(usize::MAX);
    let mut refined = input.clone();
    let mut projection = Projection::new(
        &input.relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    projection.payload(&mut refined).unwrap();
    assert_eq!(projection.finish(), Ok(true));
    assert_eq!(
        refined.store.slot_root(owner, false),
        original.store.slot_root(owner, false)
    );
    let bound = state::form(&refined, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
    let mut replay = state::instantiate(&bound, ValueAlgebra::new(domain).borrowed()).unwrap();
    let independent = state::instantiate(&bound, ValueAlgebra::new(domain).borrowed()).unwrap();
    assert_ne!(
        replay.store.slot_root(owner, false),
        independent.store.slot_root(owner, false)
    );
    assert_eq!(replay.active().key.caller, address(0x202).into());
    assert_eq!(
        replay
            .store
            .read(owner, &word(1), domain)
            .interval()
            .unsigned_bounds(),
        (U256::from(5), U256::from(15))
    );
    let child_saved = replay.active().saved_store.clone();
    replay.store.restore(child_saved);
    assert_eq!(
        replay.store.read(owner, &word(1), domain).singleton(),
        Some(U256::from(77))
    );
    let root_saved = replay.call_stack.root().state.saved_store.clone();
    replay.store.restore(root_saved);
    assert_eq!(
        replay
            .store
            .read(owner, &word(1), domain)
            .interval()
            .unsigned_bounds(),
        (U256::from(5), U256::from(15))
    );
    assert_eq!(input, original);
}

#[test]
fn one_relation_scan_preserves_numeric_results_and_each_uncached_fee() {
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let limits = RelationLimits::default();
    let expressions = [ExprId::fresh().unwrap(), ExprId::fresh().unwrap()];
    let relations = bounded_relations(&expressions, &limits);
    let relation_work = relations.work_size();
    let cases = [
        (0, AbstractValue::top(), 5),
        (
            0,
            AbstractValue::unsigned_range(U256::from(8), U256::from(100)).unwrap(),
            8,
        ),
        (1, AbstractValue::top(), 5),
    ];
    let mut budget = WorkBudget::new(usize::MAX);
    let mut projection = Projection::new(
        &relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    let mut expected_work = 0;
    for (index, value, lower) in cases {
        let original = value.with_expression(expressions[index].clone());
        // Every uncached request pays its own namespace and actual two-phase
        // refinement, even when only the numeric input changes.
        let (import, closure) = refinement_fees(&relations, &expressions[index], &original, domain);
        expected_work += expressions[index].work_size() + relation_work + import + closure;
        let mut value = original.clone();
        projection.value(&mut value);
        assert_eq!(
            value.numeric().interval().unsigned_bounds(),
            (U256::from(lower), U256::from(15))
        );
        assert_eq!(value.expression(), original.expression());
        assert_eq!(projection.budget.used(), expected_work);
        let mut repeated = original;
        projection.value(&mut repeated);
        assert_eq!(repeated, value);
        assert_eq!(projection.budget.used(), expected_work);
    }
    assert_eq!(projection.relations_work_size_scans, 1);
    projection.finish().unwrap();
    assert_eq!(budget.used(), expected_work);
    assert!(!budget.exhausted());
}

#[test]
fn lazy_size_keeps_the_exact_and_one_short_work_boundaries() {
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let original = AbstractValue::top().with_expression(expression.clone());
    let (import, closure) = refinement_fees(&relations, &expression, &original, domain);
    // A closure refusal retains both completed first-phase payments.
    let admission = expression.work_size() + relations.work_size() + import;
    let fee = admission + closure;
    for maximum in [fee, fee - 1] {
        let mut budget = WorkBudget::new(maximum);
        let mut projection = Projection::new(
            &relations,
            algebra.borrowed(),
            &limits,
            &mut budget,
            Policy::Summary,
        );
        let mut value = original.clone();
        projection.value(&mut value);
        assert_eq!(projection.relations_work_size_scans, 1);
        if maximum == fee {
            assert_eq!(
                value.numeric().interval().unsigned_bounds(),
                (U256::from(5), U256::from(15))
            );
            projection.finish().unwrap();
            assert_eq!(budget.used(), fee);
            assert!(!budget.exhausted());
        } else {
            assert_eq!(value, original);
            assert_eq!(projection.finish(), Err(MachineError::Work));
            assert_eq!(budget.used(), admission);
            assert!(budget.exhausted());
        }
    }
}

#[test]
fn concrete_values_and_empty_relations_do_not_scan() {
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let empty = RelationState::default();
    for relations in [&relations, &empty] {
        let mut budget = WorkBudget::new(0);
        let mut projection = Projection::new(
            relations,
            algebra.borrowed(),
            &limits,
            &mut budget,
            Policy::Summary,
        );
        assert_eq!(projection.relations_work_size_scans, 0);
        let mut values = vec![
            AbstractValue::constant(U256::ONE).with_expression(expression.clone()),
            AbstractValue::top(),
        ];
        if relations.is_empty() {
            values.push(AbstractValue::top().with_expression(expression.clone()));
        }
        let original = values.clone();
        for value in &mut values {
            projection.value(value);
        }
        assert_eq!(values, original);
        assert_eq!(projection.relations_work_size_scans, 0);
        projection.finish().unwrap();
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn call_guard_survives_a_stored_word_projection_without_fixing_high_bits() {
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let config = ExecutionConfig::default();
    let limits = &config.analysis.relations;
    let mut input = payload(domain);
    let original = ExprId::fresh().unwrap();
    let value = AbstractValue::top().with_expression(original.clone());
    input.store.write(address(0x101), &word(1), &value, domain);
    let read = input.store.read(address(0x101), &word(1), domain);
    assert_eq!(read.expression(), Some(&original));
    let mask = (U256::ONE << 160usize) - U256::ONE;
    let target = U256::from(0x101);
    let projected = ExprId::operation(
        opcode::AND,
        &[original, ExprId::constant(mask)],
        limits.expressions(),
    )
    .unwrap();
    let equal = ExprId::operation(
        opcode::EQ,
        &[projected, ExprId::constant(target)],
        limits.expressions(),
    )
    .unwrap();
    input
        .relations
        .assume_checked(&equal, true, limits)
        .unwrap();
    let mut refined = input.clone();
    let mut budget = WorkBudget::new(config.max_work);
    let mut projection = Projection::new(
        &input.relations,
        algebra.borrowed(),
        limits,
        &mut budget,
        Policy::Summary,
    );
    projection.payload(&mut refined).unwrap();
    projection.finish().unwrap();
    let summary = refined.store.summary_projection(domain).unwrap();
    let replay = summary.instantiate(domain).unwrap();
    let read = replay.read(address(0x101), &word(1), domain);
    assert!(read.contains(target));
    assert!(read.contains((U256::MAX & !mask) | target));
    assert!(!read.contains(target + U256::ONE));
    let target_value = domain.apply(opcode::AND, &[read, AbstractValue::constant(mask)]);
    assert_eq!(target_value.singleton(), Some(target));
    assert!(budget.used() > 0);
}

#[test]
fn failed_projection_preserves_original_path_and_roots() {
    let domain = Domain::default();
    let algebra = ValueAlgebra::new(domain);
    let config = ExecutionConfig::default();
    let mut input = payload(domain);
    let expression = ExprId::fresh().unwrap();
    input.store.write(
        address(0x101),
        &word(1),
        &AbstractValue::top().with_expression(expression.clone()),
        domain,
    );
    let equal = ExprId::operation(
        opcode::EQ,
        &[expression, ExprId::constant(U256::ONE)],
        config.analysis.relations.expressions(),
    )
    .unwrap();
    input
        .relations
        .assume_checked(&equal, true, &config.analysis.relations)
        .unwrap();
    let original = input.clone();
    let mut refined = input.clone();
    let mut budget = WorkBudget::new(0);
    let mut projection = Projection::new(
        &input.relations,
        algebra.borrowed(),
        &config.analysis.relations,
        &mut budget,
        Policy::Summary,
    );
    projection.payload(&mut refined).unwrap();
    assert!(projection.finish().is_err());
    assert_eq!(original, input);
    assert_eq!(
        original.store.slot_root(address(0x101), false),
        refined.store.slot_root(address(0x101), false)
    );
}
