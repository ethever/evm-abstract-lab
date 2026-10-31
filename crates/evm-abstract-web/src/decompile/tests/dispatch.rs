//! Presentation fixtures cover scope routing; browser cases obtain the actual
//! irreducible A/B and common-end artifact from the real decompiler.
use super::fixture;
use crate::decompile::structured;
use evm_abstract_protocol::decompile_model::{
    Artifact, DispatchCase, DispatchTransition, EdgeKind, EntityId, Stage, Strategy,
    StructuredKind, StructuredNode, StructuredProgram, Terminator, Value, ValueFact, ValueKind,
};

fn fixture_dispatch() -> Artifact {
    let mut artifact = fixture::artifact(Strategy::Dispatcher);
    let ir = &mut artifact.stages[1].ir;
    ir.blocks[0].terminator = Terminator::Branch {
        condition: EntityId(3),
        target: EntityId(3),
        then_edges: vec![EntityId(9)],
        else_edge: Some(EntityId(10)),
        unresolved: false,
        invalid_targets: vec![],
    };
    ir.blocks[1].terminator = Terminator::Branch {
        condition: EntityId(3),
        target: EntityId(3),
        then_edges: vec![EntityId(11)],
        else_edge: Some(EntityId(12)),
        unresolved: false,
        invalid_targets: vec![],
    };
    ir.blocks[2].parameters = vec![EntityId(60)];
    ir.values.push(Value {
        id: EntityId(60),
        kind: ValueKind::Parameter {
            block: EntityId(18),
            index: 0,
        },
        fact: ValueFact::Unknown,
    });
    let template = ir.edges[0].clone();
    ir.edges = [
        (9, 1, 8, EdgeKind::True),
        (10, 1, 18, EdgeKind::False),
        (11, 8, 1, EdgeKind::True),
        (12, 8, 18, EdgeKind::False),
    ]
    .into_iter()
    .map(|(id, from, to, kind)| {
        let mut edge = template.clone();
        edge.id = EntityId(id);
        edge.from = EntityId(from);
        edge.to = EntityId(to);
        edge.kind = kind;
        edge.arguments = if to == 18 { vec![EntityId(3)] } else { vec![] };
        if from == 8 {
            edge.memory = EntityId(10);
            edge.effect = EntityId(11);
        }
        edge
    })
    .collect();
    let case = |block, a, b, first, last| DispatchCase {
        block: EntityId(block),
        transitions: vec![
            DispatchTransition {
                id: EntityId(first),
                edge: EntityId(a),
                target: EntityId(if block == 1 { 8 } else { 1 }),
            },
            DispatchTransition {
                id: EntityId(last),
                edge: EntityId(b),
                target: EntityId(18),
            },
        ],
    };
    ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(42),
            kind: StructuredKind::Sequence(vec![
                StructuredNode {
                    id: EntityId(40),
                    kind: StructuredKind::Dispatcher {
                        entry: EntityId(1),
                        cases: vec![case(1, 9, 10, 51, 52), case(8, 11, 12, 53, 54)],
                    },
                },
                StructuredNode {
                    id: EntityId(41),
                    kind: StructuredKind::Exit {
                        block: EntityId(18),
                    },
                },
            ]),
        },
    });
    artifact
}

fn entity_text(stage: &Stage, entity: u64) -> String {
    structured::lines(stage)
        .into_iter()
        .filter(|line| line.entity == EntityId(entity))
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn dispatcher_guards_internal_edges_and_leaves_the_case_set_after_parallel_assignment() {
    let artifact = fixture_dispatch();
    let stage = &artifact.stages[1];
    let all = structured::lines(stage)
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("if (v3 != 0)"));
    assert!(all.contains("match v3"));
    assert!(all.contains("'dispatch40: while (true)"));
    assert!(!all.contains("is active"));
    let internal = entity_text(stage, 51);
    assert!(
        internal.contains("dispatch_state_40 = b8; continue 'dispatch40"),
        "{internal}"
    );
    let external = entity_text(stage, 52);
    assert!(
        external.contains("break 'dispatch40; // resume b18"),
        "{external}"
    );
    assert!(!external.contains("continue 'dispatch40"));
    assert!(external.find("(v60) = (v3)").unwrap() < external.find("break 'dispatch40").unwrap());
    assert!(all.find("break 'dispatch40").unwrap() < all.rfind("evm.stop();").unwrap());
}

#[test]
fn dispatcher_escape_to_natural_loop_header_continues_and_other_escape_breaks_that_loop() {
    let mut artifact = fixture_dispatch();
    let stage = &mut artifact.stages[1];
    let dispatcher = StructuredNode {
        id: EntityId(40),
        kind: StructuredKind::Dispatcher {
            entry: EntityId(8),
            cases: vec![DispatchCase {
                block: EntityId(8),
                transitions: vec![
                    DispatchTransition {
                        id: EntityId(53),
                        edge: EntityId(11),
                        target: EntityId(1),
                    },
                    DispatchTransition {
                        id: EntityId(54),
                        edge: EntityId(12),
                        target: EntityId(18),
                    },
                ],
            }],
        },
    };
    stage.ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(42),
            kind: StructuredKind::Sequence(vec![
                StructuredNode {
                    id: EntityId(55),
                    kind: StructuredKind::While {
                        header: Box::new(StructuredNode {
                            id: EntityId(56),
                            kind: StructuredKind::Block { block: EntityId(1) },
                        }),
                        condition: Some(EntityId(3)),
                        condition_nonzero: true,
                        body: Box::new(dispatcher),
                        exit_edge: Some(EntityId(10)),
                    },
                },
                StructuredNode {
                    id: EntityId(41),
                    kind: StructuredKind::Exit {
                        block: EntityId(18),
                    },
                },
            ]),
        },
    });
    let back = entity_text(stage, 53);
    let exit = entity_text(stage, 54);
    assert!(back.contains("continue 'loop55; // resume b1"), "{back}");
    assert!(exit.contains("break 'loop55; // resume b18"), "{exit}");
    assert!(exit.find("(v60) = (v3)").unwrap() < exit.find("break 'loop55").unwrap());
}
