use super::{MachineError, Policy, Projection};
#[cfg(test)]
use crate::analysis::testing::TestValue;
use crate::domain::{EvmNumericAlgebra, ValueAlgebra};
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{address, payload, word},
    },
    domain::{
        NumericValue, ProductAlgebra, WordElement, WordObservation,
        relational::{RelationLimits, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

mod native;

#[test]
fn optional_projection_does_not_discover_singletons_before_relevance() {
    use crate::analysis::state::query::tests::carrier::{Algebra, Probe, Word};
    use crate::domain::EvmValue;
    use std::rc::Rc;

    let algebra = ValueAlgebra::new(Algebra);
    let limits = RelationLimits::default();
    let unrelated = ExprId::fresh().unwrap();
    for fee in [61, usize::MAX] {
        for summary in [false, true] {
            for empty in [false, true] {
                let relations = if empty {
                    RelationState::default()
                } else {
                    bounded_relations(std::slice::from_ref(&unrelated), &limits)
                };
                let probe = Rc::new(Probe::new(fee));
                let mut value =
                    EvmValue::from_numeric(Word::constant(U256::from(7)).watched(&probe))
                        .with_expression(ExprId::fresh().unwrap())
                        .with_symbolic_limit();
                let before = value.clone();
                let mut budget = WorkBudget::new(10_000);
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
                projection.value(&mut value);
                assert_eq!(projection.finish(), Ok(true));
                assert_eq!(value, before);
                assert_eq!(probe.counts(), (0, 0, 0));
                assert!(diagnostics.is_empty());
            }
        }
    }
}

#[test]
fn relevant_projection_keeps_the_obligation_when_refinement_refuses() {
    use crate::analysis::state::query::tests::carrier::{Algebra, Probe, Word};
    use crate::domain::EvmValue;
    use std::rc::Rc;

    let algebra = ValueAlgebra::new(Algebra);
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    for summary in [false, true] {
        let probe = Rc::new(Probe::new(usize::MAX));
        let mut value = EvmValue::from_numeric(Word::constant(U256::from(7)).watched(&probe))
            .with_expression(expression.clone());
        let before = value.clone();
        let mut budget = WorkBudget::new(usize::MAX);
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
        projection.value(&mut value);
        assert_eq!(projection.finish(), Err(MachineError::Work));
        assert_eq!(value, before);
        assert_eq!(probe.counts(), (0, 0, 0));
        assert!(diagnostics.is_empty());
    }
}

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
    value: &TestValue,
    domain: crate::domain::AlgebraRef<'_, ProductAlgebra>,
) -> (usize, usize) {
    let algebra = domain;
    let view = algebra.borrowed();
    let import = relations
        .refinement_import_work(expression, value, view)
        .unwrap();
    let imported = value
        .numeric()
        .constrain(&WordObservation::unsigned_range(U256::from(5), U256::from(15)).unwrap());
    (
        import,
        imported.map_or(0, |numeric| view.numeric_reduction_work(&numeric)),
    )
}

fn cache_copy_fees(
    value: &TestValue,
    domain: crate::domain::AlgebraRef<'_, ProductAlgebra>,
) -> (usize, usize) {
    let imported = value
        .numeric()
        .constrain(&WordObservation::unsigned_range(U256::from(5), U256::from(15)).unwrap())
        .unwrap();
    (
        value.numeric().clone_work() + imported.clone_work(),
        imported.clone_work().saturating_add(
            domain
                .numeric_algebra()
                .summary_equal_work(value.numeric(), value.numeric()),
        ),
    )
}

#[test]
fn summary_reuse_and_branch_feasibility_keep_their_distinct_query_contracts() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let raw = TestValue::top().with_expression(expression.clone());
    let (import, closure) = refinement_fees(&relations, &expression, &raw, domain);
    let fee = expression.work_size() + relations.work_size() + import + closure;
    let (cache_insert, cache_hit) = cache_copy_fees(&raw, domain);
    for summary in [false, true] {
        let expected = if summary {
            fee + cache_insert + cache_hit
        } else {
            2 * fee
        };
        let mut budget = WorkBudget::new(expected);
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
        assert_eq!(budget.used(), expected);
        assert!(diagnostics.is_empty());

        let mut contradictory = TestValue::unsigned_range(U256::from(20), U256::from(30))
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
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = transfer::initial(&world, &entry, domain).unwrap();
    let expression = ExprId::fresh().unwrap();
    let value = TestValue::top().with_expression(expression.clone());
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
    let bound = state::form(&refined, &entry, domain).unwrap();
    let mut replay = state::instantiate(&bound, domain).unwrap();
    let independent = state::instantiate(&bound, domain).unwrap();
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
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let limits = RelationLimits::default();
    let expressions = [ExprId::fresh().unwrap(), ExprId::fresh().unwrap()];
    let relations = bounded_relations(&expressions, &limits);
    let relation_work = relations.work_size();
    let first = TestValue::top();
    let first_bound = TestValue::unsigned_range(U256::from(5), U256::from(15)).unwrap();
    let growing = TestValue::unsigned_range(U256::from(8), U256::from(100)).unwrap();
    // The second input passes both unequal bounds in the first cached pair,
    // once on its miss and again before reaching its own subsequently cached key.
    let prefix_eq = domain
        .numeric_algebra()
        .summary_equal_work(first.numeric(), growing.numeric())
        + domain
            .numeric_algebra()
            .summary_equal_work(first_bound.numeric(), growing.numeric());
    let cases = [
        (0, first, 5, 0),
        (0, growing, 8, prefix_eq),
        (1, TestValue::top(), 5, 0),
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
    for (index, value, lower, prefix_eq) in cases {
        let original = value.with_expression(expressions[index].clone());
        // Every uncached request pays its own namespace and actual two-phase
        // refinement, even when only the numeric input changes.
        let (import, closure) = refinement_fees(&relations, &expressions[index], &original, domain);
        let (cache_insert, cache_hit) = cache_copy_fees(&original, domain);
        expected_work += expressions[index].work_size()
            + relation_work
            + import
            + closure
            + cache_insert
            + prefix_eq;
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
        expected_work += prefix_eq + cache_hit;
        assert_eq!(projection.budget.used(), expected_work);
    }
    assert_eq!(projection.relations_work_size_scans, 1);
    projection.finish().unwrap();
    assert_eq!(budget.used(), expected_work);
    assert!(!budget.exhausted());
}

#[test]
fn lazy_size_keeps_the_exact_and_one_short_work_boundaries() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let original = TestValue::top().with_expression(expression.clone());
    let (import, closure) = refinement_fees(&relations, &expression, &original, domain);
    // A closure refusal retains both completed first-phase payments.
    let admission = expression.work_size() + relations.work_size() + import;
    let (cache_insert, _) = cache_copy_fees(&original, domain);
    let query_fee = admission + closure;
    let fee = query_fee + cache_insert;
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
            assert_eq!(budget.used(), query_fee);
            assert!(budget.exhausted());
        }
    }
}

#[test]
fn literal_expressions_and_empty_relations_do_not_scan() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
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
            TestValue::constant(U256::ONE).with_expression(ExprId::constant(U256::ONE)),
            TestValue::top(),
        ];
        if relations.is_empty() {
            values.push(TestValue::top().with_expression(expression.clone()));
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
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let config = ExecutionConfig::default();
    let limits = &domain.spec().relations();
    let mut input = payload(domain);
    let original = ExprId::fresh().unwrap();
    let value = TestValue::top().with_expression(original.clone());
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
        .assume_checked(&equal, true, limits, domain)
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
    let target_value = domain.apply(opcode::AND, &[read, TestValue::constant(mask)]);
    assert_eq!(target_value.singleton(), Some(target));
    assert!(budget.used() > 0);
}

#[test]
fn failed_projection_preserves_original_path_and_roots() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let algebra = domain;
    let limits = domain.spec().relations();
    let mut input = payload(domain);
    let expression = ExprId::fresh().unwrap();
    input.store.write(
        address(0x101),
        &word(1),
        &TestValue::top().with_expression(expression.clone()),
        domain,
    );
    let equal = ExprId::operation(
        opcode::EQ,
        &[expression, ExprId::constant(U256::ONE)],
        limits.expressions(),
    )
    .unwrap();
    input
        .relations
        .assume_checked(&equal, true, &limits, domain)
        .unwrap();
    let original = input.clone();
    let mut refined = input.clone();
    let mut budget = WorkBudget::new(0);
    let mut projection = Projection::new(
        &input.relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    assert_eq!(projection.payload(&mut refined), Err(MachineError::Work));
    assert!(projection.finish().is_err());
    assert_eq!(original, input);
    assert_eq!(
        original.store.slot_root(address(0x101), false),
        refined.store.slot_root(address(0x101), false)
    );
}

#[test]
fn branch_unknown_infeasible_work_and_cancellation_remain_distinct() {
    use crate::domain::relational::QueryReason;
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let raw = TestValue::top().with_expression(expression.clone());
    let project = |mut value: TestValue, limits: &RelationLimits, budget: &mut WorkBudget| {
        let mut diagnostics = Vec::new();
        let mut projection = Projection::new(
            &relations,
            algebra.borrowed(),
            limits,
            budget,
            Policy::Branch {
                diagnostics: &mut diagnostics,
                pc: 9,
            },
        );
        projection.value(&mut value);
        (projection.finish(), value, diagnostics)
    };
    let disabled = RelationLimits {
        enabled: false,
        ..limits
    };
    let (outcome, value, diagnostics) =
        project(raw.clone(), &disabled, &mut WorkBudget::new(usize::MAX));
    assert_eq!(outcome, Ok(true));
    assert_eq!(value, raw);
    assert_eq!(
        diagnostics,
        vec![(
            9,
            crate::analysis::DiagnosticKind::RelationalPrecisionLost(QueryReason::Disabled)
        )]
    );
    let contradictory = TestValue::unsigned_range(U256::from(20), U256::from(30))
        .unwrap()
        .with_expression(expression);
    assert_eq!(
        project(contradictory, &limits, &mut WorkBudget::new(usize::MAX)).0,
        Ok(false)
    );
    assert_eq!(
        project(raw.clone(), &limits, &mut WorkBudget::new(0)).0,
        Err(MachineError::Work)
    );
    let payer = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&payer, || WorkBudget::new(usize::MAX));
    payer.cancel();
    let (outcome, value, diagnostics) = project(raw.clone(), &limits, &mut budget);
    assert_eq!(
        outcome,
        Err(MachineError::Relations(QueryReason::Cancelled))
    );
    assert_eq!(value, raw);
    assert!(diagnostics.is_empty());
}
