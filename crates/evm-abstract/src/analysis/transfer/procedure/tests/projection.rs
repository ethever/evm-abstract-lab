use crate::domain::ProductAlgebra;
use crate::domain::EvmNumericAlgebra as _;

#[cfg(test)]
use crate::analysis::testing::TestValue;
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{address, payload, word},
    },
    domain::symbolic::ExprId,
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

#[test]
fn callee_subscription_projects_the_actual_target_guard_before_erasing_relations() {
    use crate::analysis::machine::fixtures::fixture;
    use crate::analysis::{
        ChildFrame, Continuation,
        engine::Counters,
        transfer::{
            StorageReadPolicy,
            procedure::{Table, instantiate},
        },
    };
    let (world, entry) = fixture(&[(0x101, "00")]);
    let config = ExecutionConfig::default();
    let mut config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut input = payload(domain);
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let parent = solver.procedure(input.clone()).unwrap();
    let from = solver.table.procedures[parent].first_body();
    let expression = ExprId::fresh().unwrap();
    input.store.write(
        address(0x101),
        &word(1),
        &TestValue::top().with_expression(expression.clone()),
        domain,
    );
    let mask = (U256::ONE << 160usize) - U256::ONE;
    let projected = ExprId::operation(
        opcode::AND,
        &[expression, ExprId::constant(mask)],
        config_numeric.relations.expressions(),
    )
    .unwrap();
    let equal = ExprId::operation(
        opcode::EQ,
        &[projected, ExprId::constant(U256::from(0x101))],
        config_numeric.relations.expressions(),
    )
    .unwrap();
    input
        .relations
        .assume_checked(&equal, true, &config_numeric.relations)
        .unwrap();
    let mut child = input.active().clone();
    child.key.caller = address(0x101).into();
    child.caller_operand = Some(TestValue::constant(U256::from(0x101)));
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
        &solver.table.nodes[solver.table.procedures[callee].first_body()].payload,
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
