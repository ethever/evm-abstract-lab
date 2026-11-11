//! A lexical binding must have a real raw definition and retained declaration.
use super::*;
use crate::{
    Block, Effects, Ir, LiftedBlock, LiftedExpression, LiftedGuard, LiftedGuardPayload,
    LiftedProgram, LiftedStatement, LiftedType, Operation, Terminator, Value, ValueFact, ValueKind,
};
fn id(n: u64) -> EntityId {
    EntityId(n)
}
#[test]
fn real_preloads_need_no_guard_scoped_leaf_and_unused_ghosts_cannot_bind_failure_loads() {
    let operation = |n, opcode, inputs: Vec<EntityId>, outputs: Vec<EntityId>| Operation {
        id: id(n),
        source: None,
        opcode,
        name: "source".into(),
        inputs,
        outputs,
        effects: Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    };
    let mut ir = Ir {
        entry: Some(id(10)),
        blocks: vec![
            Block {
                id: id(10),
                source: None,
                parameters: vec![id(1002)],
                memory_parameter: id(2000),
                effect_parameter: id(2001),
                operations: vec![
                    operation(11, 0x35, vec![id(1000)], vec![id(600)]),
                    operation(12, 0x57, vec![id(1001), id(1002)], vec![]),
                ],
                terminator: Terminator::Branch {
                    condition: id(1002),
                    target: id(1001),
                    then_edges: vec![],
                    else_edge: None,
                    unresolved: true,
                    invalid_targets: vec![],
                },
            },
            Block {
                id: id(100),
                source: None,
                parameters: vec![],
                memory_parameter: id(3000),
                effect_parameter: id(3001),
                operations: vec![operation(101, 0x51, vec![id(600)], vec![id(700)])],
                terminator: Terminator::Pending,
            },
        ],
        values: vec![
            Value {
                id: id(1000),
                kind: ValueKind::Constant(crate::U256::from(32)),
                fact: ValueFact::Unknown,
            },
            Value {
                id: id(1002),
                kind: ValueKind::Parameter {
                    block: id(10),
                    index: 0,
                },
                fact: ValueFact::Unknown,
            },
            Value {
                id: id(600),
                kind: ValueKind::Operation {
                    operation: id(11),
                    index: 0,
                },
                fact: ValueFact::Unknown,
            },
            Value {
                id: id(700),
                kind: ValueKind::Operation {
                    operation: id(101),
                    index: 0,
                },
                fact: ValueFact::Unknown,
            },
        ],
        ..Ir::default()
    };
    let expression = |n, block, kind| LiftedExpression {
        id: id(n),
        block: id(block),
        ty: LiftedType::Word256,
        kind,
    };
    let high = |n, block, parameters, statement| LiftedBlock {
        id: id(n),
        block: id(block),
        parameters,
        statements: vec![statement],
        condition: None,
        target: None,
        exit_values: vec![],
    };
    ir.lifted = Some(LiftedProgram {
        expressions: vec![
            expression(4000, 10, X::Word(crate::U256::from(32))),
            expression(4001, 100, X::Value(id(600))),
            expression(4002, 100, X::Value(id(700))),
            // This unused source claim must never create a declaration of v700.
            expression(4003, 10, X::Value(id(700))),
        ],
        blocks: vec![
            high(
                5000,
                10,
                vec![id(1002)],
                LiftedStatement {
                    id: id(5001),
                    operation: id(11),
                    inputs: vec![id(4000)],
                    outputs: vec![id(600)],
                },
            ),
            high(
                5002,
                100,
                vec![],
                LiftedStatement {
                    id: id(5003),
                    operation: id(101),
                    inputs: vec![id(4001)],
                    outputs: vec![id(700)],
                },
            ),
        ],
        ..LiftedProgram::default()
    });
    let renderer = ir
        .lifted
        .as_ref()
        .unwrap()
        .render_base_index_with_budget(&ir, |_| true);
    let control = LiftedControl {
        id: id(9000),
        node: id(9001),
        condition: id(9002),
        guard: Some(LiftedGuard {
            block: id(10),
            pass_nonzero: true,
            pass_edge: id(80),
            failure_edge: id(81),
            pass_node: id(90),
            failure_node: id(91),
            payload: LiftedGuardPayload::EmptyRevert,
            continuation: None,
        }),
    };
    assert_eq!(
        renderer.guard_expression(id(4001), &control, &mut |_| true),
        Ok(Some(id(4001)))
    );
    assert_eq!(
        renderer.guard_expression(id(4002), &control, &mut |_| true),
        Ok(None)
    );
    assert_eq!(
        renderer.guard_expression(id(4003), &control, &mut |_| true),
        Ok(None)
    );
}
