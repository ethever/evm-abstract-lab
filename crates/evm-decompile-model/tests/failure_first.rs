//! Execute the published rendering plan independently of its summary algorithm.
//! Event equality checks source operations, decisions, simultaneous edge tuples,
//! and terminal kind; rendered string assertions alone cannot establish these.
use evm_decompile_model::{
    Block, Edge, EdgeKind, Effects, EntityId, ExitKind, Ir, LiftedBlock, LiftedControl,
    LiftedExpression, LiftedExpressionKind, LiftedFailure, LiftedGuard, LiftedGuardContinuation,
    LiftedGuardPayload, LiftedIfPlan, LiftedIfSuffix, LiftedProgram, LiftedRenderIndex, LiftedType,
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
                && let Some(end) = self.index.selected_sequence_end(owner, position, id(303))
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
                    && let LiftedIfPlan::Require(plan) = self.index.if_plan(source.id, id(303))
                {
                    let plan = plan.clone();
                    if self.decision == plan.pass_nonzero() {
                        return self.walk(self.index.node(plan.success_node()).unwrap());
                    }
                    if !self.walk(self.index.node(plan.failure_node()).unwrap()) {
                        return false;
                    }
                    if let LiftedIfSuffix::Absorb {
                        sequence,
                        start,
                        end,
                    } = self.index.if_plan(source.id, id(303)).suffix()
                    {
                        let StructuredKind::Sequence(children) =
                            &self.index.node(sequence).unwrap().kind
                        else {
                            unreachable!()
                        };
                        return self.sequence(sequence, children, start..end);
                    }
                    return true;
                }
                if self.projected
                    && let LiftedIfPlan::FailureFirst(plan) = self.index.if_plan(source.id, id(303))
                {
                    let plan = plan.clone();
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

fn certified_fixture(failure_nonzero: bool, suffix: bool) -> Ir {
    let mut ir = fixture(failure_nonzero, suffix);
    let mut revert = operation(1200, 0xfd);
    revert.inputs = vec![id(122), id(123)];
    revert.effects.memory_read = true;
    revert.effects.terminate = true;
    revert.memory_input = Some(id(124));
    ir.blocks[2].operations = vec![revert];
    let high = ir.lifted.as_mut().unwrap();
    high.expressions.push(LiftedExpression {
        id: id(3),
        block: id(120),
        ty: LiftedType::Word256,
        kind: LiftedExpressionKind::Word(evm_decompile_model::U256::ZERO),
    });
    high.blocks[2].exit_values = vec![id(3), id(3)];
    high.failures = vec![LiftedFailure {
        block: id(120),
        rank: 0,
        payload: LiftedGuardPayload::EmptyRevert,
    }];
    high.controls[0].guard = Some(LiftedGuard {
        block: id(100),
        pass_nonzero: !failure_nonzero,
        pass_edge: id(if failure_nonzero { 201 } else { 200 }),
        failure_edge: id(if failure_nonzero { 200 } else { 201 }),
        pass_node: id(if failure_nonzero { 302 } else { 301 }),
        failure_node: id(if failure_nonzero { 301 } else { 302 }),
        payload: LiftedGuardPayload::EmptyRevert,
        continuation: suffix.then_some(LiftedGuardContinuation {
            sequence: id(303),
            start: 2,
            end: 4,
            pass_falls_through: false,
        }),
    });
    ir
}

#[test]
fn selected_requires_execute_original_lazy_failure_and_pass_tuples_in_both_polarities() {
    for polarity in [false, true] {
        for suffix in [false, true] {
            let ir = certified_fixture(polarity, suffix);
            let index = ir.lifted.as_ref().unwrap().render_index(&ir);
            assert!(matches!(
                index.if_plan(id(300), id(303)),
                LiftedIfPlan::Require(_)
            ));
            assert_eq!(
                index.selected_sequence_end(id(303), 2, id(303)),
                suffix.then_some(4)
            );
            if suffix {
                assert_eq!(index.if_plan(id(300), id(300)), LiftedIfPlan::Original);
                assert_eq!(index.selected_sequence_end(id(303), 2, id(300)), None);
            }
            equivalent(&ir);
        }
    }
}

#[test]
fn a_stale_empty_payload_fact_cannot_absorb_failed_observations() {
    for polarity in [false, true] {
        let mut ir = certified_fixture(polarity, true);
        let mut read = operation(1210, 0x54);
        read.inputs = vec![id(122)];
        read.outputs = vec![id(125)];
        read.effects.storage_read = true;
        ir.blocks[2].operations.insert(0, read);
        let index = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert!(matches!(
            index.if_plan(id(300), id(303)),
            LiftedIfPlan::FailureFirst(_)
        ));
        assert!(matches!(
            index.if_plan(id(300), id(303)).suffix(),
            LiftedIfSuffix::Move { .. }
        ));
        equivalent(&ir);
    }
}

#[test]
fn require_absorption_cannot_bypass_a_successors_open_escape_before_a_lexical_stop() {
    let mut ir = certified_fixture(false, true);
    let mut pending = ir.blocks[1].clone();
    pending.id = id(130);
    pending.terminator = Terminator::Pending;
    ir.blocks.push(pending);
    let (success, _) = arms(&mut ir);
    let StructuredKind::Sequence(children) = &mut success.kind else {
        unreachable!()
    };
    children.insert(1, node(341, StructuredKind::PartialExit { block: id(130) }));
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    index.validate_guard_sources().unwrap();
    assert_eq!(index.if_plan(id(300), id(303)), LiftedIfPlan::Original);
    assert_eq!(index.selected_sequence_end(id(303), 2, id(303)), None);
}

#[test]
fn require_keeps_a_common_failure_suffix_reached_by_the_successful_arm() {
    let mut ir = certified_fixture(false, true);
    ir.blocks[1].terminator = Terminator::Fallthrough { edge: id(202) };
    ir.blocks[1].operations.pop();
    let mut edge = ir.edges[0].clone();
    edge.id = id(202);
    edge.from = id(110);
    edge.to = id(120);
    ir.edges.push(edge);
    let (success, _) = arms(&mut ir);
    let StructuredKind::Sequence(children) = &mut success.kind else {
        unreachable!()
    };
    children[2].kind = StructuredKind::Transfer { edge: id(202) };
    ir.lifted.as_mut().unwrap().controls[0]
        .guard
        .as_mut()
        .unwrap()
        .continuation
        .as_mut()
        .unwrap()
        .pass_falls_through = true;
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(matches!(
        index.if_plan(id(300), id(303)),
        LiftedIfPlan::Require(_)
    ));
    assert_eq!(
        index.if_plan(id(300), id(303)).suffix(),
        LiftedIfSuffix::Keep
    );
    assert_eq!(index.selected_sequence_end(id(303), 2, id(303)), None);
    equivalent(&ir);
}

#[test]
fn inherited_failure_suffix_stays_at_its_owner_and_cannot_escape_a_body_boundary() {
    let mut ir = certified_fixture(false, true);
    ir.lifted.as_mut().unwrap().controls[0]
        .guard
        .as_mut()
        .unwrap()
        .continuation = None;
    let StructuredKind::Sequence(children) = &mut ir.structured.as_mut().unwrap().root.kind else {
        unreachable!()
    };
    let conditional = children.remove(1);
    children.insert(1, node(350, StructuredKind::Sequence(vec![conditional])));
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(matches!(
        index.if_plan(id(300), id(303)),
        LiftedIfPlan::Require(_)
    ));
    assert_eq!(
        index.if_plan(id(300), id(303)).suffix(),
        LiftedIfSuffix::Keep
    );
    assert_eq!(index.if_plan(id(300), id(350)), LiftedIfPlan::Original);
    assert_eq!(index.selected_sequence_end(id(303), 2, id(303)), None);
    equivalent(&ir);
}

#[test]
fn require_absorption_uses_its_own_failure_direction_when_both_arms_terminate() {
    let mut ir = certified_fixture(false, true);
    // The true arm has an observable store, then its own REVERT. The false
    // arm has a certified empty REVERT in the following source suffix.
    ir.blocks[1].terminator = Terminator::Exit {
        kind: ExitKind::Revert,
        values: vec![id(122), id(123)],
    };
    ir.blocks[1].operations[1] = operation(1101, 0xfd);
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    assert!(
        index
            .failure_first(id(300), id(303))
            .unwrap()
            .failure_nonzero()
    );
    assert!(matches!(
        index.if_plan(id(300), id(303)),
        LiftedIfPlan::Require(_)
    ));
    assert!(matches!(
        index.if_plan(id(300), id(303)).suffix(),
        LiftedIfSuffix::Absorb { .. }
    ));
    assert_eq!(index.selected_sequence_end(id(303), 2, id(303)), Some(4));
    equivalent(&ir);
}

#[test]
fn malformed_empty_revert_witnesses_never_select_require_or_absorb_the_tail() {
    for mutation in ["arguments", "terminator", "empty", "effects"] {
        let mut ir = certified_fixture(false, true);
        let block = &mut ir.blocks[2];
        match mutation {
            "arguments" => block.operations[0].inputs.clear(),
            "terminator" => {
                block.terminator = Terminator::Exit {
                    kind: ExitKind::Revert,
                    values: vec![id(122), id(999)],
                }
            }
            "empty" => {
                block.terminator = Terminator::Exit {
                    kind: ExitKind::Revert,
                    values: vec![],
                }
            }
            "effects" => block.operations[0].effects.terminate = false,
            _ => unreachable!(),
        }
        let index = ir.lifted.as_ref().unwrap().render_index(&ir);
        assert!(
            !matches!(index.if_plan(id(300), id(303)), LiftedIfPlan::Require(_)),
            "{mutation}"
        );
        assert!(
            !matches!(
                index.if_plan(id(300), id(303)).suffix(),
                LiftedIfSuffix::Absorb { .. }
            ),
            "{mutation}"
        );
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
            assert!(condition.contains(if polarity { " != 0)" } else { " == 0)" }));
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
