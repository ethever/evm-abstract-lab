//! A small typed occurrence graph isolates retained wide-exit phi accounting.
use super::Flow;
use crate::{
    Block, Edge, EdgeKind, EntityId as I, ExitKind, Ir, LiftedBlock, LiftedControl,
    LiftedExpression, LiftedExpressionKind as X, LiftedProgram, LiftedTransfer, LiftedType,
    SharedProcedureError, SharedSourceIndex, StructuredKind as K, StructuredNode as N,
    StructuredProgram, Terminator as T, U256, Value, ValueFact, ValueKind as V,
};

#[test]
fn repeated_wide_while_exits_are_in_the_retained_event_budget() {
    let parameters: Vec<_> = (2000_u64..2032).map(I).collect();
    let mut values = vec![
        Value {
            id: I(300),
            kind: V::Constant(U256::ZERO),
            fact: ValueFact::Unknown,
        },
        Value {
            id: I(301),
            kind: V::Constant(U256::from(20)),
            fact: ValueFact::Unknown,
        },
    ];
    for (id, kind) in [
        (I(100), V::Memory { operation: None }),
        (I(101), V::Effect { operation: None }),
        (I(200), V::MemoryParameter { block: I(20) }),
        (I(201), V::EffectParameter { block: I(20) }),
    ] {
        values.push(Value {
            id,
            kind,
            fact: ValueFact::Unknown,
        });
    }
    values.extend(parameters.iter().enumerate().map(|(index, id)| Value {
        id: *id,
        kind: V::Parameter {
            block: I(20),
            index,
        },
        fact: ValueFact::Unknown,
    }));
    let mut controls = Vec::new();
    let mut children = Vec::new();
    for n in 0_u64..8 {
        let id = I(10_000 + n * 3);
        controls.push(LiftedControl {
            id: I(700 + n),
            node: id,
            condition: I(501),
            guard: None,
        });
        children.push(N {
            id,
            kind: K::While {
                header: Box::new(N {
                    id: I(id.0 + 1),
                    kind: K::Block { block: I(10) },
                }),
                condition: Some(I(300)),
                condition_nonzero: true,
                body: Box::new(N {
                    id: I(id.0 + 2),
                    kind: K::Sequence(Vec::new()),
                }),
                exit_edge: Some(I(30)),
            },
        });
    }
    let ir = Ir {
        entry: Some(I(10)),
        values,
        blocks: vec![
            Block {
                id: I(10),
                source: None,
                parameters: Vec::new(),
                memory_parameter: I(100),
                effect_parameter: I(101),
                operations: Vec::new(),
                terminator: T::Fallthrough { edge: I(30) },
            },
            Block {
                id: I(20),
                source: None,
                parameters: parameters.clone(),
                memory_parameter: I(200),
                effect_parameter: I(201),
                operations: Vec::new(),
                terminator: T::Exit {
                    kind: ExitKind::Stop,
                    values: Vec::new(),
                },
            },
        ],
        edges: vec![Edge {
            id: I(30),
            from: I(10),
            to: I(20),
            kind: EdgeKind::Fallthrough,
            arguments: vec![I(300); 32],
            memory: I(100),
            effect: I(101),
        }],
        structured: Some(StructuredProgram {
            root: N {
                id: I(50_000),
                kind: K::Sequence(children),
            },
        }),
        ..Ir::default()
    };
    let p = LiftedProgram {
        expressions: vec![
            LiftedExpression {
                id: I(500),
                block: I(10),
                ty: LiftedType::Word256,
                kind: X::Word(U256::ZERO),
            },
            LiftedExpression {
                id: I(501),
                block: I(10),
                ty: LiftedType::Predicate,
                kind: X::Predicate(false),
            },
            LiftedExpression {
                id: I(502),
                block: I(10),
                ty: LiftedType::Word256,
                kind: X::Word(U256::from(20)),
            },
        ],
        blocks: vec![
            LiftedBlock {
                id: I(600),
                block: I(10),
                parameters: Vec::new(),
                statements: Vec::new(),
                condition: Some(I(501)),
                target: Some(I(502)),
                exit_values: Vec::new(),
            },
            LiftedBlock {
                id: I(601),
                block: I(20),
                parameters,
                statements: Vec::new(),
                condition: None,
                target: None,
                exit_values: Vec::new(),
            },
        ],
        transfers: vec![LiftedTransfer {
            id: I(602),
            edge: I(30),
            arguments: vec![I(500); 32],
        }],
        controls,
        ..LiftedProgram::default()
    };
    let source = SharedSourceIndex::for_program(&ir, Some(&p), &mut |_| true).unwrap();
    assert!(Flow::new_with_slots(&source, &mut |_| true, 10_000).is_ok());
    assert!(
        matches!(
            Flow::new_with_slots(&source, &mut |_| true, 200),
            Err(SharedProcedureError::Capacity {
                resource: crate::SharedCapacity::FlowEventSlots,
                required,
                limit: 200,
            }) if required > 200
        ),
        "eight 32-word exit assignments must exceed 200 retained event slots"
    );
}
