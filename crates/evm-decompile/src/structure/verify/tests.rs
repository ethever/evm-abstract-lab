//! Mutation checks exercise the checker, never the planner that it admits.
use super::{Plan, PlanError, verify};
use crate::structure::plan::{PlanId, PlanNode, Region};
use crate::{
    Block, DispatchCase, DispatchTransition, Edge, EdgeKind, EntityId, ExitKind, IndirectArm, Ir,
    SourceSpan, StructuredKind as K, Terminator as T,
};

fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn node(plan: &mut Plan, kind: K<PlanId>) -> PlanId {
    let id = PlanId(plan.nodes.len());
    plan.nodes.push(PlanNode {
        kind,
        sources: vec![],
    });
    id
}
fn sequence(plan: &mut Plan, children: Vec<PlanId>) -> PlanId {
    node(plan, K::Sequence(children))
}
fn empty() -> Plan {
    Plan {
        nodes: vec![],
        root: PlanId(0),
        regions: vec![],
    }
}
fn block(n: u64, terminator: T) -> Block {
    Block {
        id: id(n),
        source: Some(SourceSpan {
            pc: n as usize,
            end_pc: n as usize + 1,
        }),
        parameters: vec![],
        memory_parameter: id(100 + n),
        effect_parameter: id(200 + n),
        operations: vec![],
        terminator,
    }
}
fn edge(n: u64, from: u64, to: u64, kind: EdgeKind) -> Edge {
    Edge {
        id: id(n),
        from: id(from),
        to: id(to),
        kind,
        arguments: vec![],
        memory: id(100 + from),
        effect: id(200 + from),
    }
}
fn branch() -> Ir {
    Ir {
        entry: Some(id(1)),
        blocks: vec![
            block(
                1,
                T::Branch {
                    condition: id(10),
                    target: id(11),
                    then_edges: vec![id(20)],
                    else_edge: Some(id(21)),
                    unresolved: false,
                    invalid_targets: vec![],
                },
            ),
            block(
                2,
                T::Exit {
                    kind: ExitKind::Return,
                    values: vec![],
                },
            ),
            block(
                3,
                T::Exit {
                    kind: ExitKind::Revert,
                    values: vec![],
                },
            ),
        ],
        edges: vec![
            edge(20, 1, 2, EdgeKind::True),
            edge(21, 1, 3, EdgeKind::False),
        ],
        ..Ir::default()
    }
}
fn terminal(plan: &mut Plan, block: u64) -> PlanId {
    let body = node(plan, K::Block { block: id(block) });
    let exit = node(plan, K::Exit { block: id(block) });
    sequence(plan, vec![body, exit])
}
fn path(plan: &mut Plan, edge: u64, block: u64) -> PlanId {
    let transfer = node(plan, K::Transfer { edge: id(edge) });
    let terminal = terminal(plan, block);
    sequence(plan, vec![transfer, terminal])
}
fn explicit() -> Plan {
    let mut plan = empty();
    let yes = path(&mut plan, 20, 2);
    let no = path(&mut plan, 21, 3);
    let choice = node(
        &mut plan,
        K::If {
            condition: id(10),
            then_branch: Box::new(yes),
            else_branch: Box::new(no),
        },
    );
    let body = node(&mut plan, K::Block { block: id(1) });
    plan.root = sequence(&mut plan, vec![body, choice]);
    plan.regions.push(Region {
        entry: id(1),
        continuation: None,
        loop_header: None,
        body: plan.root,
    });
    plan
}
fn checked(ir: &Ir, plan: &Plan) {
    assert_eq!(verify(ir, plan, &mut |_| true), Ok(()));
}
fn refused(ir: &Ir, plan: &Plan) {
    assert!(matches!(
        verify(ir, plan, &mut |_| true),
        Err(PlanError::Invalid(_))
    ));
}

#[test]
fn exact_branches_are_admitted_and_control_mutations_are_rejected() {
    let ir = branch();
    checked(&ir, &explicit());
    for mutation in 0..6 {
        let mut plan = explicit();
        match mutation {
            0 => {
                for n in &mut plan.nodes {
                    if let K::If { condition, .. } = &mut n.kind {
                        *condition = id(11);
                    }
                }
            }
            1 => {
                for n in &mut plan.nodes {
                    if let K::If {
                        then_branch,
                        else_branch,
                        ..
                    } = &mut n.kind
                    {
                        std::mem::swap(then_branch, else_branch);
                    }
                }
            }
            2 => {
                for n in &mut plan.nodes {
                    if let K::Exit { block } = &mut n.kind
                        && *block == id(2)
                    {
                        *block = id(3);
                    }
                }
            }
            3 => {
                for n in &mut plan.nodes {
                    if let K::Block { block } = &mut n.kind
                        && *block == id(2)
                    {
                        *block = id(3);
                    }
                }
            }
            4 => {
                for n in &mut plan.nodes {
                    if let K::Transfer { edge } = &mut n.kind
                        && *edge == id(21)
                    {
                        *edge = id(20);
                    }
                }
            }
            5 => {
                let root = plan.root.0;
                let K::Sequence(children) = &mut plan.nodes[root].kind else {
                    unreachable!()
                };
                children.insert(1, children[0]);
            }
            _ => unreachable!(),
        }
        refused(&ir, &plan);
    }
}

#[test]
fn a_body_cannot_be_omitted_or_a_branch_bypassed_despite_complete_coverage() {
    let ir = branch();
    let mut plan = explicit();
    for node in &mut plan.nodes {
        if let K::Sequence(children) = &mut node.kind
            && children.len() == 2
            && children[0] == PlanId(1)
        {
            children.remove(0);
        }
    }
    refused(&ir, &plan);
    let mut plan = empty();
    let body = node(&mut plan, K::Block { block: id(1) });
    let yes = path(&mut plan, 20, 2);
    let no = path(&mut plan, 21, 3);
    plan.root = sequence(&mut plan, vec![body, yes, no]);
    refused(&ir, &plan);
}

#[test]
fn pure_epsilon_loops_never_certify_an_empty_or_terminal_source() {
    for ir in [
        Ir::default(),
        Ir {
            entry: Some(id(1)),
            blocks: vec![block(
                1,
                T::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                },
            )],
            ..Ir::default()
        },
    ] {
        let mut plan = empty();
        let empty = sequence(&mut plan, vec![]);
        plan.root = node(
            &mut plan,
            K::While {
                header: Box::new(empty),
                condition: None,
                condition_nonzero: true,
                body: Box::new(empty),
                exit_edge: None,
            },
        );
        refused(&ir, &plan);
    }
}

fn dispatched(ir: &Ir) -> Plan {
    let mut plan = empty();
    let cases = ir
        .blocks
        .iter()
        .map(|b| DispatchCase {
            block: b.id,
            transitions: b
                .terminator
                .edges()
                .iter()
                .map(|e| DispatchTransition {
                    id: id(0),
                    edge: *e,
                    target: ir.edge(*e).unwrap().to,
                })
                .collect(),
        })
        .collect();
    plan.root = node(
        &mut plan,
        K::Dispatcher {
            entry: ir.entry.unwrap(),
            cases,
        },
    );
    plan
}

#[test]
fn dispatch_preserves_exact_cases_edge_identity_and_target_assignment() {
    let ir = branch();
    checked(&ir, &dispatched(&ir));
    for mutation in 0..5 {
        let mut plan = dispatched(&ir);
        let K::Dispatcher { entry, cases } = &mut plan.nodes[0].kind else {
            unreachable!()
        };
        match mutation {
            0 => *entry = id(2),
            1 => {
                cases[0].transitions.pop();
            }
            2 => cases[0].transitions[0].target = id(3),
            3 => cases.push(cases[0].clone()),
            4 => cases[0].transitions[0].edge = id(21),
            _ => unreachable!(),
        }
        refused(&ir, &plan);
    }
}

#[test]
fn unknown_and_known_invalid_boundaries_stay_distinct_even_without_edges() {
    for unresolved in [false, true] {
        let ir = Ir {
            entry: Some(id(1)),
            blocks: vec![block(
                1,
                T::Jump {
                    target: id(10),
                    edges: vec![],
                    unresolved,
                    invalid_targets: vec![],
                },
            )],
            ..Ir::default()
        };
        let mut plan = empty();
        let body = node(&mut plan, K::Block { block: id(1) });
        let exit = node(
            &mut plan,
            if unresolved {
                K::PartialExit { block: id(1) }
            } else {
                K::Exit { block: id(1) }
            },
        );
        plan.root = sequence(&mut plan, vec![body, exit]);
        checked(&ir, &plan);
        plan.nodes[exit.0].kind = if unresolved {
            K::Exit { block: id(1) }
        } else {
            K::PartialExit { block: id(1) }
        };
        refused(&ir, &plan);
        checked(&ir, &dispatched(&ir));
    }
}

#[test]
fn arena_cycles_missing_references_and_work_refusals_are_safe() {
    let ir = branch();
    let plan = explicit();
    assert_eq!(verify(&ir, &plan, &mut |_| false), Err(PlanError::Limit));
    let mut malformed = explicit();
    malformed.nodes[0].kind = K::Sequence(vec![PlanId(0)]);
    refused(&ir, &malformed);
    malformed = explicit();
    malformed.root = PlanId(usize::MAX);
    refused(&ir, &malformed);
}

fn loop_source() -> Ir {
    let mut ir = branch();
    ir.blocks[1].terminator = T::Fallthrough { edge: id(22) };
    ir.blocks[2].terminator = T::Exit {
        kind: ExitKind::Stop,
        values: vec![],
    };
    ir.edges.push(edge(22, 2, 1, EdgeKind::Jump));
    ir
}
fn while_plan() -> Plan {
    let mut plan = empty();
    let header = node(&mut plan, K::Block { block: id(1) });
    let inside = node(&mut plan, K::Transfer { edge: id(20) });
    let body = node(&mut plan, K::Block { block: id(2) });
    let back = node(&mut plan, K::Transfer { edge: id(22) });
    let repeat = node(&mut plan, K::Continue);
    let body = sequence(&mut plan, vec![inside, body, back, repeat]);
    let repeat = node(
        &mut plan,
        K::While {
            header: Box::new(header),
            condition: Some(id(10)),
            condition_nonzero: true,
            body: Box::new(body),
            exit_edge: Some(id(21)),
        },
    );
    let following = terminal(&mut plan, 3);
    plan.root = sequence(&mut plan, vec![repeat, following]);
    plan
}

#[test]
fn while_requires_the_original_test_polarity_exit_edge_and_iteration_transfer() {
    let ir = loop_source();
    checked(&ir, &while_plan());
    for mutation in 0..4 {
        let mut plan = while_plan();
        for node in &mut plan.nodes {
            match &mut node.kind {
                K::While {
                    condition_nonzero,
                    exit_edge,
                    ..
                } => match mutation {
                    0 => *condition_nonzero = false,
                    1 => *exit_edge = Some(id(20)),
                    _ => {}
                },
                K::Continue if mutation == 2 => node.kind = K::Break,
                K::Transfer { edge } if mutation == 3 && *edge == id(22) => {
                    node.kind = K::Sequence(vec![])
                }
                _ => {}
            }
        }
        refused(&ir, &plan);
    }
    let mut outside = empty();
    outside.root = node(&mut outside, K::Continue);
    refused(&ir, &outside);
}

#[test]
fn dispatch_escape_into_loop_header_and_after_loop_is_checked_contextually() {
    let ir = loop_source();
    let mut plan = while_plan();
    let K::Sequence(root) = &plan.nodes[plan.root.0].kind else {
        unreachable!()
    };
    let while_id = root[0];
    let K::While { body, .. } = &plan.nodes[while_id.0].kind else {
        unreachable!()
    };
    let body = **body;
    let K::Sequence(children) = &plan.nodes[body.0].kind else {
        unreachable!()
    };
    let transfer = children[0];
    // Replace the iteration's block/back-edge/continue with a source dispatcher.
    // New arena nodes precede the new enclosing While, preserving DAG order.
    let compact = node(
        &mut plan,
        K::Dispatcher {
            entry: id(2),
            cases: vec![DispatchCase {
                block: id(2),
                transitions: vec![DispatchTransition {
                    id: id(0),
                    edge: id(22),
                    target: id(1),
                }],
            }],
        },
    );
    let new_body = sequence(&mut plan, vec![transfer, compact]);
    let K::While {
        header,
        condition,
        condition_nonzero,
        exit_edge,
        ..
    } = plan.nodes[while_id.0].kind.clone()
    else {
        unreachable!()
    };
    let new_loop = node(
        &mut plan,
        K::While {
            header,
            condition,
            condition_nonzero,
            body: Box::new(new_body),
            exit_edge,
        },
    );
    let terminal = terminal(&mut plan, 3);
    plan.root = sequence(&mut plan, vec![new_loop, terminal]);
    checked(&ir, &plan);
    let mut malformed = plan;
    if let K::Dispatcher { cases, .. } = &mut malformed.nodes[compact.0].kind {
        cases[0].transitions[0].target = id(3);
    }
    refused(&ir, &malformed);
}

#[test]
fn indirect_target_arms_and_unknown_boundary_are_exact() {
    let mut ir = branch();
    ir.blocks[0].terminator = T::Jump {
        target: id(11),
        edges: vec![id(20), id(21)],
        unresolved: true,
        invalid_targets: vec![],
    };
    for edge in &mut ir.edges {
        edge.kind = EdgeKind::Jump;
    }
    let mut plan = empty();
    let left = path(&mut plan, 20, 2);
    let right = path(&mut plan, 21, 3);
    let choice = node(
        &mut plan,
        K::Indirect {
            block: id(1),
            arms: vec![
                IndirectArm {
                    target_pc: 2,
                    body: left,
                },
                IndirectArm {
                    target_pc: 3,
                    body: right,
                },
            ],
            unresolved: true,
        },
    );
    let body = node(&mut plan, K::Block { block: id(1) });
    plan.root = sequence(&mut plan, vec![body, choice]);
    checked(&ir, &plan);
    for mutation in 0..3 {
        let mut bad = Plan {
            nodes: plan
                .nodes
                .iter()
                .map(|node| PlanNode {
                    kind: node.kind.clone(),
                    sources: node.sources.clone(),
                })
                .collect(),
            root: plan.root,
            regions: vec![],
        };
        let K::Indirect {
            arms, unresolved, ..
        } = &mut bad.nodes[choice.0].kind
        else {
            unreachable!()
        };
        match mutation {
            0 => *unresolved = false,
            1 => arms[0].target_pc = 3,
            2 => {
                arms.pop();
            }
            _ => unreachable!(),
        }
        refused(&ir, &bad);
    }
    // A sole known edge does not authorize deleting an unresolved alternative.
    ir.blocks[0].terminator = T::Jump {
        target: id(11),
        edges: vec![id(20)],
        unresolved: true,
        invalid_targets: vec![],
    };
    let mut bad = empty();
    let body = node(&mut bad, K::Block { block: id(1) });
    let known = path(&mut bad, 20, 2);
    bad.root = sequence(&mut bad, vec![body, known]);
    refused(&ir, &bad);
}

#[test]
fn dispatcher_escape_must_follow_actual_sequence_anchors() {
    let mut ir = branch();
    ir.blocks.remove(2);
    ir.blocks[0].terminator = T::Branch {
        condition: id(10),
        target: id(11),
        then_edges: vec![id(20)],
        else_edge: Some(id(21)),
        unresolved: false,
        invalid_targets: vec![],
    };
    ir.edges[0].to = id(1);
    ir.edges[1].to = id(2);
    let mut plan = empty();
    let dispatch = node(
        &mut plan,
        K::Dispatcher {
            entry: id(1),
            cases: vec![DispatchCase {
                block: id(1),
                transitions: vec![
                    DispatchTransition {
                        id: id(0),
                        edge: id(20),
                        target: id(1),
                    },
                    DispatchTransition {
                        id: id(0),
                        edge: id(21),
                        target: id(2),
                    },
                ],
            }],
        },
    );
    let terminal = terminal(&mut plan, 2);
    plan.root = sequence(&mut plan, vec![dispatch, terminal]);
    checked(&ir, &plan);
    // Empty shared containers have exponentially many syntax paths, but no
    // emitted children. Admission follows the materialized flat Sequence.
    let mut empty_sequence = sequence(&mut plan, vec![]);
    for _ in 0..32 {
        empty_sequence = sequence(&mut plan, vec![empty_sequence, empty_sequence]);
    }
    let nested = sequence(&mut plan, vec![empty_sequence, terminal]);
    plan.root = sequence(&mut plan, vec![dispatch, nested]);
    let mut work = 0;
    assert_eq!(
        verify(&ir, &plan, &mut |amount| {
            work += amount;
            work < 10_000
        }),
        Ok(())
    );
    // An If has no entry anchor. AST execution propagates Escaped(2) rather
    // than entering it, even if both branches contain block 2 somewhere.
    let fake_follow = node(
        &mut plan,
        K::If {
            condition: id(10),
            then_branch: Box::new(terminal),
            else_branch: Box::new(terminal),
        },
    );
    plan.root = sequence(&mut plan, vec![dispatch, fake_follow]);
    refused(&ir, &plan);
}
