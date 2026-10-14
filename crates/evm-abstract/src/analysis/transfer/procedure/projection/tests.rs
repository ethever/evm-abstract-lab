use super::Projection;
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{address, payload, word},
    },
    domain::{AbstractValue, Domain, symbolic::ExprId},
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

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
