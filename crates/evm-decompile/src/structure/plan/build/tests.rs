//! Temporary metadata refusal rolls back proposals before compact retry.
use super::*;
use crate::{Cancellation, Config, Fork, Input, Phase, StageKind, decompile_with_progress};

fn source() -> (Input, Ir) {
    let bytes = evm_bytecode::parse_hex(
        "5f356100115760403561001d57610021565b60203561001d57610021565b5f5ffd5b60775f5500",
    )
    .unwrap();
    let input = Input::new(bytes, Fork::Osaka);
    let cancel = Cancellation::new();
    let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |p| {
        if p.phase == Phase::Structure {
            cancel.cancel();
        }
    });
    assert_eq!(artifact.current().unwrap().kind, StageKind::Simplified);
    (input, artifact.current().unwrap().ir.clone())
}

#[test]
fn temporary_sources_and_children_refusal_retries_an_admitted_source_dispatcher() {
    let (input, ir) = source();
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    // This source dispatcher needs 2*7 block slots + 3*8 transition slots.
    // The natural plan's temporary ancestry/children exceed this small budget.
    let plan = build_with_slot_limit(&ir, &facts, &mut owner, 48).unwrap();
    assert!(!owner.stopped);
    assert!(matches!(
        plan.nodes[plan.root.0].kind,
        StructuredKind::Dispatcher { .. }
    ));
    assert!(
        plan.nodes.len() < 8,
        "rolled-back candidates do not remain retained"
    );
    super::super::super::verify::verify(&ir, &plan, &mut |n| owner.pay(n)).unwrap();
    assert!(owner.artifact.stages.is_empty());
}

#[test]
fn even_the_compact_plan_must_fit_the_temporary_storage_admission() {
    let (input, ir) = source();
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert!(build_with_slot_limit(&ir, &facts, &mut owner, 37).is_none());
    assert_eq!(owner.artifact.completion, Completion::Limited);
    assert_eq!(owner.id(), EntityId(1));
    assert!(owner.artifact.stages.is_empty());
}

#[test]
fn metadata_refusal_preserves_code_duplication_policy_and_cancellation() {
    let (input, ir) = source();
    let config = Config {
        strategy: Strategy::CodeDuplication,
        ..Config::default()
    };
    let mut owner = Builder::new(&input, &config, &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert!(build_with_slot_limit(&ir, &facts, &mut owner, 48).is_none());
    assert_eq!(owner.artifact.completion, Completion::Limited);
    assert_eq!(owner.artifact.config, config);
    let cancel = Cancellation::new();
    let mut owner = Builder::new(&input, &Config::default(), &cancel, |_| {});
    cancel.cancel();
    assert!(build_with_slot_limit(&ir, &facts, &mut owner, 48).is_none());
    assert_eq!(owner.artifact.completion, Completion::Cancelled);
    assert!(owner.artifact.stages.is_empty());
}

#[test]
fn cumulative_natural_attempt_limit_and_pressure_survive_an_arena_rollback() {
    let (input, ir) = source();
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    planner.natural_limit = 1;
    let checkpoint = planner.checkpoint();
    planner
        .build(ir.entry.unwrap(), None, None, 0, &BTreeSet::new())
        .unwrap();
    assert!(planner.compact_pressure);
    assert_eq!(planner.natural_attempts, 1);
    planner.rollback(checkpoint);
    assert!(
        planner.compact_pressure,
        "rollback cannot restart natural planning"
    );
    assert_eq!(planner.natural_attempts, 1);
    let root = planner
        .build(ir.entry.unwrap(), None, None, 0, &BTreeSet::new())
        .unwrap();
    assert_eq!(
        planner.natural_attempts, 1,
        "the same region does not naturally replan"
    );
    assert_eq!(planner.nodes.len(), 1);
    let plan = Plan {
        nodes: planner.nodes,
        root,
        regions: planner.regions,
    };
    super::super::super::verify::verify(&ir, &plan, &mut |n| planner.owner.pay(n)).unwrap();
}

#[test]
fn source_join_memo_including_negative_answers_survives_proposal_rollback() {
    let (input, ir) = source();
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    let entry = ir.block(ir.entry.unwrap()).unwrap();
    let Terminator::Branch {
        then_edges,
        else_edge: Some(other),
        ..
    } = &entry.terminator
    else {
        panic!("branch fixture")
    };
    let left = ir.edge(then_edges[0]).unwrap().to;
    let right = ir.edge(*other).unwrap().to;
    let tail = ir
        .blocks
        .iter()
        .find(|b| b.source.is_some_and(|s| s.pc == 33))
        .unwrap()
        .id;
    let failed = ir
        .blocks
        .iter()
        .find(|b| b.source.is_some_and(|s| s.pc == 29))
        .unwrap()
        .id;
    let checkpoint = planner.checkpoint();
    let before = planner.owner.budget.used();
    assert_eq!(planner.common_join(left, right, None, None), Ok(Some(tail)));
    assert!(planner.owner.budget.used() - before > 8);
    planner.rollback(checkpoint);
    let before = planner.owner.budget.used();
    assert_eq!(planner.common_join(left, right, None, None), Ok(Some(tail)));
    assert_eq!(
        planner.owner.budget.used() - before,
        8,
        "the source fact requires no second graph walk"
    );
    let before = planner.owner.budget.used();
    assert_eq!(planner.common_join(failed, tail, None, None), Ok(None));
    assert_eq!(
        planner.owner.budget.used() - before,
        8,
        "an empty candidate set performs no distance scan"
    );
    planner.rollback(checkpoint);
    assert_eq!(planner.join_cache.len(), 2);
    assert_eq!(planner.common_join(failed, tail, None, None), Ok(None));
}

#[test]
fn scoped_join_memo_keeps_distinct_stops_and_survives_proposal_rollback() {
    let input = Input::new(
        evm_bytecode::parse_hex(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/structure/regions/loop-normal-join.hex"
        )))
        .unwrap(),
        Fork::Osaka,
    );
    let cancel = Cancellation::new();
    let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Structure {
            cancel.cancel();
        }
    });
    let ir = &artifact.current().unwrap().ir;
    let at_pc = |pc| {
        ir.blocks
            .iter()
            .find(|block| block.source.is_some_and(|source| source.pc == pc))
            .unwrap()
            .id
    };
    let header = at_pc(8);
    let join = at_pc(43);
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(ir, &mut owner).unwrap();
    let facts = Facts::new(ir, &dom, &mut owner).unwrap();
    let info = &facts.loops[&header];
    let local = facts
        .normal_region(
            ir,
            RegionScope {
                continuation: Some(header),
                loop_header: Some(header),
            },
            &mut |n| owner.pay(n),
        )
        .unwrap();
    let (left, right) = ir
        .blocks
        .iter()
        .find_map(|block| {
            let Terminator::Branch {
                then_edges,
                else_edge: Some(other),
                ..
            } = &block.terminator
            else {
                return None;
            };
            if then_edges.len() != 1 {
                return None;
            }
            let left = ir.edge(then_edges[0])?.to;
            let right = ir.edge(*other)?.to;
            (local.postdom.get(&left)?.contains(&join)
                && local.postdom.get(&right)?.contains(&join))
            .then_some((left, right))
        })
        .unwrap();
    assert!(!facts.finite.contains(&left));
    assert!(!facts.finite.contains(&right));
    let mut planner = Planner::new(ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    assert_eq!(
        planner.common_join(left, right, Some(header), Some(info)),
        Ok(Some(join))
    );
    let checkpoint = planner.checkpoint();
    assert_eq!(
        planner.common_join(left, right, Some(left), Some(info)),
        Ok(None)
    );
    planner.rollback(checkpoint);
    let before = planner.owner.budget.used();
    assert_eq!(
        planner.common_join(left, right, Some(header), Some(info)),
        Ok(Some(join))
    );
    assert_eq!(planner.owner.budget.used() - before, 8);
    assert_eq!(planner.join_cache.len(), 2);
    assert_eq!(
        planner.normal_region.as_ref().unwrap().0.continuation,
        Some(left)
    );
}

#[test]
fn failed_guard_chain_has_shallow_reading_cost_under_every_profile() {
    let mut bytes = Vec::new();
    for offset in 0_u16..16 {
        bytes.push(0x61);
        bytes.extend_from_slice(&(offset * 32).to_be_bytes());
        bytes.push(0x35);
        bytes.push(0x61);
        let target = bytes.len();
        bytes.extend([0, 0, 0x57, 0x5f, 0x5f, 0xfd]);
        let next = u16::try_from(bytes.len()).unwrap();
        bytes[target..target + 2].copy_from_slice(&next.to_be_bytes());
        bytes.push(0x5b);
    }
    bytes.push(0x00);
    let input = Input::new(bytes, Fork::Osaka);
    let cancel = Cancellation::new();
    let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Structure {
            cancel.cancel();
        }
    });
    let ir = &artifact.current().unwrap().ir;
    for profile in [
        crate::ReadabilityProfile::Balanced,
        crate::ReadabilityProfile::Linear,
        crate::ReadabilityProfile::Compact,
    ] {
        let config = Config {
            readability: crate::ReadabilityConfig {
                profile,
                ..Default::default()
            },
            ..Config::default()
        };
        let mut owner = Builder::new(&input, &config, &Cancellation::new(), |_| {});
        let dom = crate::transform::dominators(ir, &mut owner).unwrap();
        let facts = Facts::new(ir, &dom, &mut owner).unwrap();
        let plan = build(ir, &facts, &mut owner).unwrap();
        super::super::super::verify::verify(ir, &plan, &mut |work| owner.pay(work)).unwrap();
        let decisions = plan.decisions(&mut |work| owner.pay(work)).unwrap();
        let root = decisions
            .iter()
            .find(|decision| {
                decision.region.entry == ir.entry.unwrap() && decision.region.continuation.is_none()
            })
            .unwrap();
        let metrics = root.natural.unwrap();
        assert_eq!(root.selected, StructureForm::Natural);
        assert!(metrics.conditions >= 16);
        assert!(metrics.max_nesting <= 2, "{profile:?}: {metrics:?}");
        assert!(!plan.has_dispatch(&mut |work| owner.pay(work)).unwrap());
    }
}

fn residual_with_suffix(open: bool) -> Ir {
    use crate::{Block, Edge, EdgeKind, ExitKind, SourceSpan, Value, ValueFact, ValueKind};
    let id = EntityId;
    let block = |n, terminator| Block {
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
    };
    let branch = |yes, no| Terminator::Branch {
        condition: id(10),
        target: id(11),
        then_edges: vec![id(yes)],
        else_edge: Some(id(no)),
        unresolved: false,
        invalid_targets: vec![],
    };
    let mut ir = Ir {
        entry: Some(id(1)),
        blocks: vec![
            block(
                1,
                Terminator::Jump {
                    target: id(11),
                    edges: vec![id(20), id(21)],
                    unresolved: false,
                    invalid_targets: vec![],
                },
            ),
            block(2, branch(22, 23)),
            block(3, branch(24, 25)),
            block(4, Terminator::Fallthrough { edge: id(26) }),
            block(
                5,
                Terminator::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                },
            ),
        ],
        values: [10, 11]
            .into_iter()
            .map(|n| Value {
                id: id(n),
                kind: ValueKind::Constant(crate::U256::ZERO),
                fact: ValueFact::Unknown,
            })
            .collect(),
        edges: [
            (20, 1, 2, EdgeKind::Jump),
            (21, 1, 3, EdgeKind::Jump),
            (22, 2, 3, EdgeKind::True),
            (23, 2, 4, EdgeKind::False),
            (24, 3, 2, EdgeKind::True),
            (25, 3, 4, EdgeKind::False),
            (26, 4, 5, EdgeKind::Fallthrough),
        ]
        .into_iter()
        .map(|(n, from, to, kind)| Edge {
            id: id(n),
            from: id(from),
            to: id(to),
            kind,
            arguments: vec![],
            memory: id(100 + from),
            effect: id(200 + from),
        })
        .collect(),
        ..Ir::default()
    };
    if open && let Terminator::Branch { unresolved, .. } = &mut ir.blocks[1].terminator {
        *unresolved = true;
    }
    ir
}

#[test]
fn residual_cycle_keeps_a_proved_shared_suffix_outside_each_dispatcher() {
    let ir = residual_with_suffix(false);
    let mut owner = Builder::new(
        &Input::new(vec![], Fork::Osaka),
        &Config::default(),
        &Cancellation::new(),
        |_| {},
    );
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert_eq!(
        facts.residual_continuation(&ir, EntityId(2), &mut |work| owner.pay(work)),
        Some(Some(EntityId(4)))
    );
    let plan = build(&ir, &facts, &mut owner).unwrap();
    super::super::super::verify::verify(&ir, &plan, &mut |work| owner.pay(work)).unwrap();
    let mut dispatched = 0;
    for node in &plan.nodes {
        if let StructuredKind::Dispatcher { cases, .. } = &node.kind {
            dispatched += 1;
            assert!(
                cases
                    .iter()
                    .all(|case| [EntityId(2), EntityId(3)].contains(&case.block))
            );
        }
    }
    assert!(dispatched > 0);
    let decisions = plan.decisions(&mut |work| owner.pay(work)).unwrap();
    assert!(
        decisions
            .iter()
            .any(|decision| decision.selected == StructureForm::Dispatcher
                && decision.region.continuation == Some(EntityId(4)))
    );
}

#[test]
fn unknown_exit_does_not_get_a_false_local_dispatch_continuation() {
    let ir = residual_with_suffix(true);
    let mut owner = Builder::new(
        &Input::new(vec![], Fork::Osaka),
        &Config::default(),
        &Cancellation::new(),
        |_| {},
    );
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert_eq!(
        facts.residual_continuation(&ir, EntityId(2), &mut |work| owner.pay(work)),
        Some(None)
    );
    let plan = build(&ir, &facts, &mut owner).unwrap();
    super::super::super::verify::verify(&ir, &plan, &mut |work| owner.pay(work)).unwrap();
}

#[test]
fn automatic_profiles_choose_different_verified_forms_for_a_repeated_source_suffix() {
    use crate::{
        Block, Edge, EdgeKind, Effects, ExitKind, Operation, ReadabilityProfile, SourceSpan, Value,
        ValueFact, ValueKind,
    };
    let id = EntityId;
    let mut ir = Ir {
        entry: Some(id(1)),
        values: vec![Value {
            id: id(5000),
            kind: ValueKind::Parameter {
                block: id(1),
                index: 0,
            },
            fact: ValueFact::Unknown,
        }],
        ..Ir::default()
    };
    for n in 1..=80 {
        let terminator = match n {
            1 => Terminator::Jump {
                target: id(5000),
                edges: vec![id(10000), id(10001)],
                unresolved: false,
                invalid_targets: vec![],
            },
            80 => Terminator::Exit {
                kind: ExitKind::Stop,
                values: vec![],
            },
            _ => Terminator::Fallthrough {
                edge: id(10000 + n),
            },
        };
        ir.blocks.push(Block {
            id: id(n),
            source: Some(SourceSpan {
                pc: n as usize,
                end_pc: n as usize + 1,
            }),
            parameters: if n == 1 { vec![id(5000)] } else { vec![] },
            memory_parameter: id(1000 + n),
            effect_parameter: id(2000 + n),
            operations: vec![Operation {
                id: id(3000 + n),
                source: None,
                opcode: 0x5b,
                name: "JUMPDEST".into(),
                inputs: vec![],
                outputs: vec![],
                effects: Effects::default(),
                memory_input: None,
                memory_output: None,
                replacement: None,
                effect_input: None,
                effect_output: None,
            }],
            terminator,
        });
    }
    let edge = |n, from, to, kind| Edge {
        id: id(n),
        from: id(from),
        to: id(to),
        kind,
        arguments: vec![],
        memory: id(1000 + from),
        effect: id(2000 + from),
    };
    ir.edges.extend([
        edge(10000, 1, 2, EdgeKind::Jump),
        edge(10001, 1, 3, EdgeKind::Jump),
    ]);
    for from in 2..80 {
        ir.edges.push(edge(
            10000 + from,
            from,
            if from < 4 { 4 } else { from + 1 },
            EdgeKind::Fallthrough,
        ));
    }
    for (profile, expected) in [
        (ReadabilityProfile::Balanced, StructureForm::Natural),
        (ReadabilityProfile::Linear, StructureForm::Natural),
        (ReadabilityProfile::Compact, StructureForm::Dispatcher),
    ] {
        let config = Config {
            readability: crate::ReadabilityConfig {
                profile,
                ..Default::default()
            },
            ..Config::default()
        };
        let mut owner = Builder::new(
            &Input::new(vec![], Fork::Osaka),
            &config,
            &Cancellation::new(),
            |_| {},
        );
        let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
        let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
        let plan = build(&ir, &facts, &mut owner).unwrap();
        // This checks all represented source choices and their exact transfers;
        // it does not assume that a favorable cost makes the candidate valid.
        super::super::super::verify::verify(&ir, &plan, &mut |work| owner.pay(work)).unwrap();
        let decisions = plan.decisions(&mut |work| owner.pay(work)).unwrap();
        let root = decisions
            .iter()
            .find(|decision| {
                decision.region.entry == id(1) && decision.region.continuation.is_none()
            })
            .unwrap();
        assert_eq!(root.selected, expected, "{profile:?}: {root:?}");
        assert!(
            root.natural
                .is_some_and(|metrics| metrics.duplicated_operations > 0)
        );
        if profile == ReadabilityProfile::Compact {
            assert!(
                root.dispatcher
                    .is_some_and(|metrics| metrics.dispatcher_blocks == 80)
            );
            assert_eq!(root.reason, ReadabilityReason::LowerReadingCost);
        } else {
            assert!(
                root.dispatcher.is_none(),
                "an excluded representation family need not be measured"
            );
        }
    }
}
