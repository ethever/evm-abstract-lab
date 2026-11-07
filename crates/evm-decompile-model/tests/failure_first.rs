//! Execute the published rendering plan independently of its summary algorithm.
//! Event equality checks source operations, decisions, simultaneous edge tuples,
//! and terminal kind; rendered string assertions alone cannot establish these.
use evm_decompile_model::{
    Block, Edge, EdgeKind, Effects, EntityId, ExitKind, Ir, LiftedBlock, LiftedControl,
    LiftedExpression, LiftedExpressionKind, LiftedProgram, LiftedRenderIndex, LiftedType,
    Operation, SharedInvocation, StructuredKind, StructuredNode, StructuredProgram, Terminator,
    Value, ValueFact, ValueKind,
};

fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn node(n: u64, kind: StructuredKind) -> StructuredNode {
    StructuredNode { id: id(n), kind }
}
fn operation(n: u64, opcode: u8) -> Operation {
    Operation {
        id: id(n),
        source: None,
        opcode,
        name: format!("OP_{opcode:02x}"),
        inputs: vec![],
        outputs: vec![],
        effects: Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    }
}
fn fixture(failure_nonzero: bool, suffix: bool) -> Ir {
    let block = |n, terminator, opcodes: &[u8]| Block {
        id: id(n),
        source: None,
        parameters: vec![],
        memory_parameter: id(n + 3),
        effect_parameter: id(n + 4),
        operations: opcodes
            .iter()
            .enumerate()
            .map(|(i, opcode)| operation(n * 10 + i as u64, *opcode))
            .collect(),
        terminator,
    };
    let mut ir = Ir {
        blocks: vec![
            block(
                100,
                Terminator::Branch {
                    condition: id(101),
                    target: id(102),
                    then_edges: vec![id(200)],
                    else_edge: Some(id(201)),
                    unresolved: false,
                    invalid_targets: vec![],
                },
                &[0x54, 0x57],
            ),
            block(
                110,
                Terminator::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                },
                &[0x55, 0x00],
            ),
            block(
                120,
                Terminator::Exit {
                    kind: ExitKind::Revert,
                    values: vec![id(122), id(123)],
                },
                &[0x54, 0xf1, 0x52, 0xfd],
            ),
        ],
        values: vec![Value {
            id: id(101),
            kind: ValueKind::Parameter {
                block: id(100),
                index: 0,
            },
            fact: ValueFact::Unknown,
        }],
        edges: [
            (200, if failure_nonzero { 120 } else { 110 }, EdgeKind::True),
            (
                201,
                if failure_nonzero { 110 } else { 120 },
                EdgeKind::False,
            ),
        ]
        .into_iter()
        .map(|(n, to, kind)| Edge {
            id: id(n),
            from: id(100),
            to: id(to),
            kind,
            arguments: vec![id(101)],
            memory: id(103),
            effect: id(104),
        })
        .collect(),
        ..Ir::default()
    };
    let arm = |n: u64, failed: bool, edge: u64| {
        let target = if failed { 120 } else { 110 };
        let mut children = vec![node(n + 10, StructuredKind::Transfer { edge: id(edge) })];
        if !failed || !suffix {
            children.push(node(n + 20, StructuredKind::Block { block: id(target) }));
            children.push(node(n + 30, StructuredKind::Exit { block: id(target) }));
        }
        node(n, StructuredKind::Sequence(children))
    };
    let mut children = vec![
        node(307, StructuredKind::Block { block: id(100) }),
        node(
            300,
            StructuredKind::If {
                condition: id(101),
                then_branch: Box::new(arm(301, failure_nonzero, 200)),
                else_branch: Box::new(arm(302, !failure_nonzero, 201)),
            },
        ),
    ];
    if suffix {
        children.extend([
            node(308, StructuredKind::Block { block: id(120) }),
            node(309, StructuredKind::Exit { block: id(120) }),
        ]);
    }
    ir.structured = Some(StructuredProgram {
        root: node(303, StructuredKind::Sequence(children)),
    });
    ir.lifted = Some(LiftedProgram {
        expressions: vec![
            LiftedExpression {
                id: id(1),
                block: id(100),
                ty: LiftedType::Predicate,
                kind: LiftedExpressionKind::NonZero(id(2)),
            },
            LiftedExpression {
                id: id(2),
                block: id(100),
                ty: LiftedType::Word256,
                kind: LiftedExpressionKind::Value(id(101)),
            },
        ],
        blocks: ir
            .blocks
            .iter()
            .map(|b| LiftedBlock {
                id: id(b.id.0 + 400),
                block: b.id,
                parameters: vec![],
                statements: vec![],
                condition: (b.id == id(100)).then_some(id(1)),
                target: None,
                exit_values: vec![],
            })
            .collect(),
        controls: vec![LiftedControl {
            id: id(10),
            node: id(300),
            condition: id(1),
            guard: None,
        }],
        ..LiftedProgram::default()
    });
    ir
}
fn arms(ir: &mut Ir) -> (&mut StructuredNode, &mut StructuredNode) {
    let StructuredKind::Sequence(v) = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    let StructuredKind::If {
        then_branch,
        else_branch,
        ..
    } = &mut v[1].kind
    else {
        unreachable!()
    };
    (then_branch, else_branch)
}
#[derive(Debug, PartialEq, Eq)]
enum Event {
    Operation(EntityId, u8),
    Condition(EntityId, bool),
    Transfer(EntityId, Vec<EntityId>, EntityId, EntityId),
    Terminal(EntityId, String),
}
struct Walker<'a> {
    ir: &'a Ir,
    index: LiftedRenderIndex<'a>,
    decision: bool,
    projected: bool,
    events: Vec<Event>,
}
impl Walker<'_> {
    fn sequence(
        &mut self,
        owner: EntityId,
        children: &[StructuredNode],
        range: std::ops::Range<usize>,
    ) -> bool {
        let mut position = range.start;
        while position < range.end {
            if self.projected
                && position > range.start
                && let Some(end) = self
                    .index
                    .failure_first_sequence_end(owner, position, id(303))
            {
                assert!(end > position && end <= range.end);
                position = end;
                continue;
            }
            if !self.walk(&children[position]) {
                return false;
            }
            position += 1;
        }
        true
    }
    fn walk(&mut self, source: &StructuredNode) -> bool {
        match &source.kind {
            StructuredKind::Sequence(children) => {
                self.sequence(source.id, children, 0..children.len())
            }
            StructuredKind::Block { block } => {
                for operation in &self.ir.block(*block).unwrap().operations {
                    self.events
                        .push(Event::Operation(operation.id, operation.opcode));
                }
                true
            }
            StructuredKind::Transfer { edge } => {
                let e = self.ir.edge(*edge).unwrap();
                self.events.push(Event::Transfer(
                    e.id,
                    e.arguments.clone(),
                    e.memory,
                    e.effect,
                ));
                true
            }
            StructuredKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.events
                    .push(Event::Condition(*condition, self.decision));
                if self.projected
                    && let Some(plan) = self.index.failure_first(source.id, id(303)).cloned()
                {
                    if self.decision == plan.failure_nonzero() {
                        if !self.walk(self.index.node(plan.failure_node()).unwrap()) {
                            return false;
                        }
                        if let Some((sequence, range)) =
                            self.index.failure_first_suffix_range(&plan)
                        {
                            let StructuredKind::Sequence(children) =
                                &self.index.node(sequence).unwrap().kind
                            else {
                                unreachable!()
                            };
                            if !self.sequence(sequence, children, range) {
                                return false;
                            }
                        }
                        panic!("admitted failed arm reached the success body");
                    }
                    self.walk(self.index.node(plan.success_node()).unwrap())
                } else {
                    self.walk(if self.decision {
                        then_branch
                    } else {
                        else_branch
                    })
                }
            }
            StructuredKind::Exit { block } => {
                self.events.push(Event::Terminal(
                    *block,
                    format!("{:?}", self.ir.block(*block).unwrap().terminator),
                ));
                false
            }
            _ => panic!("finite fixture has an unsupported source node"),
        }
    }
}
fn events(ir: &Ir, decision: bool, projected: bool) -> Vec<Event> {
    let mut walker = Walker {
        ir,
        index: ir.lifted.as_ref().unwrap().render_index(ir),
        decision,
        projected,
        events: vec![],
    };
    assert!(!walker.walk(&ir.structured.as_ref().unwrap().root));
    walker.events
}
fn equivalent(ir: &Ir) {
    for decision in [false, true] {
        assert_eq!(events(ir, decision, false), events(ir, decision, true));
    }
}

#[test]
fn both_polarities_keep_complete_failed_observations_transfers_and_condition_once() {
    for polarity in [false, true] {
        for suffix in [false, true] {
            let ir = fixture(polarity, suffix);
            let index = ir.lifted.as_ref().unwrap().render_index(&ir);
            let plan = index.failure_first(id(300), id(303)).unwrap();
            assert_eq!(plan.failure_nonzero(), polarity);
            assert_eq!(index.failure_first_suffix(plan).is_some(), suffix);
            let condition = index
                .render_failure_first_condition(plan, |_| true)
                .unwrap();
            assert_eq!(condition.starts_with("!("), !polarity);
            assert_eq!(condition.matches("v101").count(), 1);
            equivalent(&ir);
            let failure = events(&ir, polarity, true);
            assert_eq!(
                failure
                    .iter()
                    .filter(|e| matches!(e, Event::Condition(..)))
                    .count(),
                1
            );
            assert_eq!(
                failure
                    .iter()
                    .filter_map(|e| match e {
                        Event::Operation(_, opcode) => Some(*opcode),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                [0x54, 0x57, 0x54, 0xf1, 0x52, 0xfd]
            );
        }
    }
}

#[test]
fn invalid_is_finite_exceptional_failure_and_never_becomes_revert() {
    let mut ir = fixture(false, true);
    ir.blocks[2].terminator = Terminator::Trap {
        reason: "INVALID opcode".into(),
    };
    ir.blocks[2].operations.last_mut().unwrap().opcode = 0xfe;
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(index.failure_first(id(300), id(303)).is_some());
    equivalent(&ir);
    let failed = events(&ir, false, true);
    assert!(
        matches!(failed.last(), Some(Event::Terminal(_, terminal)) if terminal.contains("Trap"))
    );
    assert!(failed.contains(&Event::Operation(id(1203), 0xfe)));
}

#[test]
fn a_local_suffix_cannot_escape_a_procedure_or_subtree_boundary() {
    let ir = fixture(false, true);
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(index.failure_first(id(300), id(303)).is_some());
    assert!(index.failure_first(id(300), id(300)).is_none());
    assert_eq!(
        index.failure_first_sequence_end(id(303), 2, id(303)),
        Some(4)
    );
    assert_eq!(index.failure_first_sequence_end(id(303), 2, id(300)), None);
}

#[test]
fn a_success_arm_that_falls_through_cannot_move_the_shared_failure_suffix() {
    let mut ir = fixture(false, true);
    let (success, _) = arms(&mut ir);
    let StructuredKind::Sequence(v) = &mut success.kind else {
        unreachable!()
    };
    v.pop();
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(index.failure_first(id(300), id(303)).is_none());
    assert_eq!(index.failure_first_sequence_end(id(303), 2, id(303)), None);
    equivalent(&ir);
}

#[test]
fn normal_unknown_and_loop_exits_are_not_finite_failure_evidence() {
    for terminator in [
        Terminator::Exit {
            kind: ExitKind::Stop,
            values: vec![],
        },
        Terminator::Exit {
            kind: ExitKind::Return,
            values: vec![],
        },
        Terminator::Exit {
            kind: ExitKind::SelfDestruct,
            values: vec![],
        },
        Terminator::Exit {
            kind: ExitKind::EndOfCode,
            values: vec![],
        },
        Terminator::Pending,
        Terminator::Jump {
            target: id(102),
            edges: vec![],
            unresolved: true,
            invalid_targets: vec![],
        },
    ] {
        let mut ir = fixture(false, true);
        ir.blocks[2].terminator = terminator;
        let index = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert!(index.failure_first(id(300), id(303)).is_none());
        assert_eq!(index.failure_first_sequence_end(id(303), 2, id(303)), None);
    }
    let mut ir = fixture(false, false);
    let (_, failed) = arms(&mut ir);
    let StructuredKind::Sequence(v) = &mut failed.kind else {
        unreachable!()
    };
    v[1] = node(
        322,
        StructuredKind::While {
            header: Box::new(node(340, StructuredKind::Sequence(vec![]))),
            condition: None,
            condition_nonzero: true,
            body: Box::new(node(341, StructuredKind::Sequence(vec![]))),
            exit_edge: None,
        },
    );
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(index.failure_first(id(300), id(303)).is_none());
}

#[test]
fn missing_moved_or_mismatched_terminal_bodies_cannot_admit_failure() {
    for mutation in ["missing", "sibling", "wrong", "transfer"] {
        let mut ir = fixture(false, false);
        let (success, failed) = arms(&mut ir);
        let StructuredKind::Sequence(v) = &mut failed.kind else {
            unreachable!()
        };
        match mutation {
            "missing" => {
                v.remove(1);
            }
            "sibling" => {
                let body = v.remove(1);
                let StructuredKind::Sequence(other) = &mut success.kind else {
                    unreachable!()
                };
                other.insert(1, body);
            }
            "wrong" => v[1].kind = StructuredKind::Block { block: id(110) },
            "transfer" => v.insert(2, node(345, StructuredKind::Transfer { edge: id(200) })),
            _ => unreachable!(),
        }
        let index = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert!(
            index.failure_first(id(300), id(303)).is_none(),
            "{mutation}"
        );
        assert_eq!(index.failure_first_sequence_end(id(303), 2, id(303)), None);
    }
}

#[test]
fn wrong_condition_or_edge_bindings_refuse_the_plan() {
    for mutation in ["condition", "polarity", "source", "target"] {
        let mut ir = fixture(false, true);
        match mutation {
            "condition" => ir.lifted.as_mut().unwrap().controls[0].condition = id(2),
            "polarity" => ir.edges[0].kind = EdgeKind::False,
            "source" => ir.edges[0].from = id(110),
            "target" => ir.edges[1].to = id(110),
            _ => unreachable!(),
        }
        let index = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert!(
            index.failure_first(id(300), id(303)).is_none(),
            "{mutation}"
        );
        assert_eq!(index.failure_first_sequence_end(id(303), 2, id(303)), None);
    }
}

#[test]
fn invocation_substitution_and_refused_work_publish_no_suffix_cut() {
    let mut ir = fixture(false, true);
    ir.lifted
        .as_mut()
        .unwrap()
        .invocations
        .push(SharedInvocation {
            id: id(600),
            procedure: id(601),
            source: id(300),
            owner: Some(id(303)),
            position: 1,
            continuation: Some(id(120)),
            bindings: vec![],
        });
    let program = ir.lifted.as_ref().unwrap();
    let index = program.render_base_index_with_budget(&ir, |_| true);
    assert!(index.failure_first(id(300), id(303)).is_none());
    assert_eq!(index.failure_first_sequence_end(id(303), 2, id(303)), None);
    ir.lifted.as_mut().unwrap().invocations.clear();
    let program = ir.lifted.as_ref().unwrap();
    let mut work = 0_usize;
    let _ = program.render_index_with_budget(&ir, |n| {
        work += n;
        true
    });
    for limit in [0, work / 2, work - 1] {
        let mut used = 0_usize;
        let index = program.render_index_with_budget(&ir, |n| {
            if used.saturating_add(n) > limit {
                false
            } else {
                used += n;
                true
            }
        });
        assert!(index.validate_failure_first_sources().is_err());
        assert!(index.failure_first(id(300), id(303)).is_none());
        assert_eq!(index.failure_first_sequence_end(id(303), 2, id(303)), None);
    }
}

#[test]
fn a_loop_before_a_later_stop_proves_no_fallthrough_without_proving_finite_failure() {
    let mut ir = fixture(false, true);
    let (success, _) = arms(&mut ir);
    let StructuredKind::Sequence(v) = &mut success.kind else {
        unreachable!()
    };
    v.insert(
        1,
        node(
            340,
            StructuredKind::While {
                header: Box::new(node(341, StructuredKind::Sequence(vec![]))),
                condition: None,
                condition_nonzero: true,
                body: Box::new(node(342, StructuredKind::Sequence(vec![]))),
                exit_edge: None,
            },
        ),
    );
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(index.failure_first(id(300), id(303)).is_some());
    assert_eq!(
        index.failure_first_sequence_end(id(303), 2, id(303)),
        Some(4)
    );
    // The same loop in the failed path could run forever before REVERT.
    // It therefore cannot justify moving the successful body out of an else.
    let mut failed_loop = fixture(false, false);
    let (_, failure) = arms(&mut failed_loop);
    let StructuredKind::Sequence(v) = &mut failure.kind else {
        unreachable!()
    };
    v.insert(
        1,
        node(
            340,
            StructuredKind::While {
                header: Box::new(node(341, StructuredKind::Sequence(vec![]))),
                condition: None,
                condition_nonzero: true,
                body: Box::new(node(342, StructuredKind::Sequence(vec![]))),
                exit_edge: None,
            },
        ),
    );
    let index = failed_loop
        .lifted
        .as_ref()
        .unwrap()
        .render_index(&failed_loop);
    assert!(index.failure_first(id(300), id(303)).is_none());
}

#[test]
fn a_true_partial_successor_cannot_be_closed_by_a_later_lexical_stop() {
    for unknown in ["partial", "pending", "indirect", "loop"] {
        let mut ir = fixture(false, true);
        let mut partial = ir.blocks[1].clone();
        partial.id = id(130);
        partial.terminator = if unknown == "pending" {
            Terminator::Pending
        } else {
            Terminator::Jump {
                target: id(102),
                edges: vec![],
                unresolved: true,
                invalid_targets: vec![],
            }
        };
        ir.blocks.push(partial);
        let escape = node(341, StructuredKind::PartialExit { block: id(130) });
        let unknown_node = match unknown {
            "partial" | "pending" => escape,
            "indirect" => node(
                341,
                StructuredKind::Indirect {
                    block: id(130),
                    arms: vec![],
                    unresolved: true,
                },
            ),
            "loop" => node(
                340,
                StructuredKind::While {
                    header: Box::new(node(342, StructuredKind::Sequence(vec![]))),
                    condition: None,
                    condition_nonzero: true,
                    body: Box::new(escape),
                    exit_edge: None,
                },
            ),
            _ => unreachable!(),
        };
        let (success, _) = arms(&mut ir);
        let StructuredKind::Sequence(v) = &mut success.kind else {
            unreachable!()
        };
        v.insert(1, unknown_node);
        let index = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert!(index.failure_first(id(300), id(303)).is_none(), "{unknown}");
        assert_eq!(
            index.failure_first_sequence_end(id(303), 2, id(303)),
            None,
            "{unknown}"
        );
    }
}
