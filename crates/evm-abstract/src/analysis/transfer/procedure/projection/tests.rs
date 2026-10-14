use super::{MachineError, Projection};
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{address, payload, word},
    },
    domain::{
        AbstractValue, Domain, NumericValue,
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

#[test]
fn one_relation_scan_preserves_numeric_results_and_each_uncached_fee() {
    let domain = Domain::default();
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
    let mut projection = Projection::new(&relations, domain, &limits, &mut budget);
    let mut expected_work = 0;
    for (index, value, lower) in cases {
        let original = value.with_expression(expressions[index].clone());
        // These are the existing admission and refinement charges, even when
        // only the old numeric bound (rather than the expression) changes.
        expected_work += expressions[index].work_size()
            + relation_work
            + domain.refinement_work(&original, relations.len());
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
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let original = AbstractValue::top().with_expression(expression.clone());
    let admission = expression.work_size() + relations.work_size();
    let fee = admission + domain.refinement_work(&original, relations.len());
    for maximum in [fee, fee - 1] {
        let mut budget = WorkBudget::new(maximum);
        let mut projection = Projection::new(&relations, domain, &limits, &mut budget);
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
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let empty = RelationState::default();
    for relations in [&relations, &empty] {
        let mut budget = WorkBudget::new(0);
        let mut projection = Projection::new(relations, domain, &limits, &mut budget);
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
    let mut projection = Projection::new(&input.relations, domain, limits, &mut budget);
    projection.payload(&mut refined);
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
        domain,
        &config.analysis.relations,
        &mut budget,
    );
    projection.payload(&mut refined);
    assert!(projection.finish().is_err());
    assert_eq!(original, input);
    assert_eq!(
        original.store.slot_root(address(0x101), false),
        refined.store.slot_root(address(0x101), false)
    );
}

#[test]
fn callee_subscription_projects_the_actual_target_guard_before_erasing_relations() {
    use crate::analysis::machine::fixtures::fixture;
    use crate::analysis::{
        ChildFrame, Continuation,
        engine::Counters,
        transfer::{
            StorageReadPolicy,
            procedure::{Solver, Table, payload::instantiate},
        },
    };
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let mut input = payload(domain);
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = Solver {
        table: &mut table,
        world: &world,
        entry: &entry,
        config: &config,
        domain,
        budget: &mut budget,
        counters: &mut counters,
        storage_reads: StorageReadPolicy::Abstract,
    };
    let parent = solver.procedure(input.clone()).unwrap();
    let from = solver.table.procedures[parent].entry;
    let expression = ExprId::fresh().unwrap();
    input.store.write(
        address(0x101),
        &word(1),
        &AbstractValue::top().with_expression(expression.clone()),
        domain,
    );
    let mask = (U256::ONE << 160usize) - U256::ONE;
    let projected = ExprId::operation(
        opcode::AND,
        &[expression, ExprId::constant(mask)],
        config.analysis.relations.expressions(),
    )
    .unwrap();
    let equal = ExprId::operation(
        opcode::EQ,
        &[projected, ExprId::constant(U256::from(0x101))],
        config.analysis.relations.expressions(),
    )
    .unwrap();
    input
        .relations
        .assume_checked(&equal, true, &config.analysis.relations)
        .unwrap();
    let mut child = input.active().clone();
    child.key.caller = address(0x101).into();
    child.caller_operand = Some(AbstractValue::constant(U256::from(0x101)));
    input.call_stack.push_child(ChildFrame {
        state: child,
        continuation: Continuation {
            return_block: None,
            output_offset: word(0),
            output_size: word(0),
            creation: None,
        },
    });
    solver.subscribe(from, input).unwrap();
    let callee = solver.table.subscribers[0].callee;
    assert_ne!(callee, parent);
    let replay = instantiate(
        &solver.table.nodes[solver.table.procedures[callee].entry].payload,
        domain,
    )
    .unwrap();
    let read = replay.store.read(address(0x101), &word(1), domain);
    assert!(read.contains((U256::MAX & !mask) | U256::from(0x101)));
    assert_eq!(
        domain.address_projection(&read).singleton(),
        Some(U256::from(0x101))
    );
    assert!(
        replay.relations.is_empty(),
        "only the proved numeric consequence is replayed"
    );
}
