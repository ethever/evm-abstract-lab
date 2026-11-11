//! Rationale follows observed planner control, including abandoned proposals.
use super::*;

#[test]
fn a_declined_single_entry_loop_reports_a_residual_cycle_even_with_natural_preference() {
    let mut ir = residual_with_suffix(false);
    let Terminator::Jump { edges, .. } = &mut ir.blocks[0].terminator else {
        unreachable!()
    };
    edges.retain(|edge| *edge != EntityId(21));
    ir.edges.retain(|edge| edge.id != EntityId(21));
    let input = Input::new(vec![], Fork::Osaka);
    let mut config = Config::default();
    config.readability.regions.push(crate::RegionPreference {
        region: RegionAnchor {
            source: crate::readability_source(&input, &config),
            entry: EntityId(2),
            continuation: Some(EntityId(4)),
            loop_header: None,
        },
        structure: StructurePreference::Natural,
    });
    let mut owner = Builder::new(&input, &config, &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    assert!(crate::transform::irreducible(&ir, &dom).is_none());
    let mut facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert!(facts.loops.contains_key(&EntityId(2)));
    // Simulate a loop shape outside the recognizer's admitted set. Reducibility
    // alone must not turn its fallback receipt into a multiple-entry claim.
    facts.loops.clear();
    let plan = build(&ir, &facts, &mut owner).unwrap();
    crate::structure::verify::verify(&ir, &plan, &mut |n| owner.pay(n)).unwrap();
    let decisions = plan.decisions(&mut |n| owner.pay(n)).unwrap();
    let decision = decisions
        .iter()
        .find(|decision| {
            decision.region.entry == EntityId(2)
                && decision.region.continuation == Some(EntityId(4))
        })
        .unwrap();
    assert_eq!(decision.preference, StructurePreference::Natural);
    assert_eq!(decision.selected, StructureForm::Dispatcher);
    assert_eq!(decision.reason, ReadabilityReason::ResidualCycle);
    assert!(decision.natural.is_none());
    assert!(decisions.iter().all(|decision| {
        !matches!(
            decision.reason,
            ReadabilityReason::MultiEntryControl | ReadabilityReason::PreferenceFallback
        )
    }));
}

#[test]
fn path_reentry_retains_its_trigger_across_cache_reuse_and_arena_rollback() {
    let (input, ir) = source();
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    let start = ir.entry.unwrap();
    let checkpoint = planner.checkpoint();
    let root = planner
        .build(start, None, None, 0, &BTreeSet::from([start]))
        .unwrap();
    assert_eq!(
        planner.dispatch_reasons[root.0],
        Some(ReadabilityReason::ControlReentry)
    );
    let cached = planner
        .build(start, None, None, 0, &BTreeSet::new())
        .unwrap();
    assert_eq!(cached, root);
    assert_eq!(
        planner.dispatch_reasons[cached.0],
        Some(ReadabilityReason::ControlReentry)
    );
    planner.rollback(checkpoint);
    assert!(planner.dispatch_reasons.is_empty() && planner.cache.is_empty());
    let root = planner
        .build(start, None, None, 0, &BTreeSet::new())
        .unwrap();
    assert!(planner.dispatch_reasons[root.0].is_none());
    assert_eq!(planner.dispatch_reasons.len(), planner.nodes.len());
    assert_eq!(
        planner
            .regions
            .iter()
            .find(|region| region.body == root)
            .and_then(|region| region.decision.as_ref())
            .unwrap()
            .reason,
        ReadabilityReason::NaturalControl
    );
}

#[test]
fn memo_pressure_activated_during_planning_reports_the_actual_child_fallback() {
    let (input, ir) = source();
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    planner.memo_limit = 0;
    assert!(!planner.compact_pressure);
    let root = planner
        .build(ir.entry.unwrap(), None, None, 0, &BTreeSet::new())
        .unwrap();
    assert!(planner.compact_pressure);
    let plan = Plan {
        nodes: planner.nodes,
        root,
        regions: planner.regions,
    };
    crate::structure::verify::verify(&ir, &plan, &mut |n| planner.owner.pay(n)).unwrap();
    let decisions = plan.decisions(&mut |n| planner.owner.pay(n)).unwrap();
    let dispatchers: Vec<_> = decisions
        .iter()
        .filter(|decision| decision.selected == StructureForm::Dispatcher)
        .collect();
    assert!(!dispatchers.is_empty());
    assert!(
        dispatchers
            .iter()
            .all(|decision| decision.reason == ReadabilityReason::PlanningFallback)
    );
}

#[test]
fn a_natural_preference_keeps_the_depth_cause_instead_of_a_generic_preference_fallback() {
    let (input, ir) = source();
    let mut config = Config::default();
    config.limits.max_structuring_depth = 0;
    config.readability.regions.push(crate::RegionPreference {
        region: RegionAnchor {
            source: crate::readability_source(&input, &config),
            entry: ir.entry.unwrap(),
            continuation: None,
            loop_header: None,
        },
        structure: StructurePreference::Natural,
    });
    let mut owner = Builder::new(&input, &config, &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    let root = planner
        .build(ir.entry.unwrap(), None, None, 1, &BTreeSet::new())
        .unwrap();
    let plan = Plan {
        nodes: planner.nodes,
        root,
        regions: planner.regions,
    };
    crate::structure::verify::verify(&ir, &plan, &mut |n| planner.owner.pay(n)).unwrap();
    let decisions = plan.decisions(&mut |n| planner.owner.pay(n)).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].preference, StructurePreference::Natural);
    assert_eq!(decisions[0].reason, ReadabilityReason::DepthFallback);
}

#[test]
fn rejected_structure_retries_a_verified_dispatcher_with_a_validation_cause() {
    let (input, ir) = source();
    let mut config = Config::default();
    config.readability.regions.push(crate::RegionPreference {
        region: RegionAnchor {
            source: crate::readability_source(&input, &config),
            entry: ir.entry.unwrap(),
            continuation: None,
            loop_header: None,
        },
        structure: StructurePreference::Natural,
    });
    let mut owner = Builder::new(&input, &config, &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    let mut rejected = build(&ir, &facts, &mut owner).unwrap();
    rejected.nodes[rejected.root.0].kind = StructuredKind::Continue;
    assert!(matches!(
        crate::structure::verify::verify(&ir, &rejected, &mut |n| owner.pay(n)),
        Err(PlanError::Invalid(_))
    ));
    let fallback = compact_fallback(&ir, &facts, &mut owner).unwrap();
    crate::structure::verify::verify(&ir, &fallback, &mut |n| owner.pay(n)).unwrap();
    let decisions = fallback.decisions(&mut |n| owner.pay(n)).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].reason, ReadabilityReason::ValidationFallback);
    assert_eq!(decisions[0].preference, StructurePreference::Natural);
}

fn expanding_suffix() -> Ir {
    let block = |n: u64, terminator| crate::Block {
        id: EntityId(n),
        source: Some(crate::SourceSpan {
            pc: n as usize,
            end_pc: n as usize + 1,
        }),
        parameters: vec![],
        memory_parameter: EntityId(1000 + n),
        effect_parameter: EntityId(2000 + n),
        operations: vec![],
        terminator,
    };
    let mut ir = Ir {
        entry: Some(EntityId(1)),
        values: vec![crate::Value {
            id: EntityId(5000),
            kind: crate::ValueKind::Parameter {
                block: EntityId(1),
                index: 0,
            },
            fact: crate::ValueFact::Unknown,
        }],
        ..Ir::default()
    };
    // Indirect alternatives share the next suffix. The source and plan arena
    // grow linearly while their expanded natural occurrences grow exponentially.
    for level in 0..11 {
        let n = 1 + level * 3;
        let e = 10000 + level * 4;
        ir.blocks.push(block(
            n,
            Terminator::Jump {
                target: EntityId(5000),
                edges: vec![EntityId(e), EntityId(e + 1)],
                unresolved: false,
                invalid_targets: vec![],
            },
        ));
        for arm in 0..2 {
            ir.blocks.push(block(
                n + arm + 1,
                Terminator::Fallthrough {
                    edge: EntityId(e + arm + 2),
                },
            ));
            for (id, from, to, kind) in [
                (e + arm, n, n + arm + 1, crate::EdgeKind::Jump),
                (
                    e + arm + 2,
                    n + arm + 1,
                    n + 3,
                    crate::EdgeKind::Fallthrough,
                ),
            ] {
                ir.edges.push(crate::Edge {
                    id: EntityId(id),
                    from: EntityId(from),
                    to: EntityId(to),
                    kind,
                    arguments: vec![],
                    memory: EntityId(1000 + from),
                    effect: EntityId(2000 + from),
                });
            }
        }
    }
    ir.blocks[0].parameters.push(EntityId(5000));
    ir.blocks.push(block(
        34,
        Terminator::Exit {
            kind: crate::ExitKind::Stop,
            values: vec![],
        },
    ));
    ir
}

#[test]
fn expansion_evidence_survives_rollback_without_becoming_a_planning_or_cycle_reason() {
    let ir = expanding_suffix();
    let input = Input::new(vec![], Fork::Osaka);
    let mut owner = Builder::new(&input, &Config::default(), &Cancellation::new(), |_| {});
    let dom = crate::transform::dominators(&ir, &mut owner).unwrap();
    let facts = Facts::new(&ir, &dom, &mut owner).unwrap();
    assert!(facts.finite.contains(&EntityId(1)));
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS).unwrap();
    let checkpoint = planner.checkpoint();
    let root = planner
        .build(EntityId(1), None, None, 0, &BTreeSet::new())
        .unwrap();
    assert_eq!(
        planner.dispatch_reasons[root.0],
        Some(ReadabilityReason::Expansion)
    );
    assert!(planner.expanded_sources.contains(&EntityId(1)));
    assert!(!planner.compact_pressure);
    let attempts = planner.natural_attempts;
    planner.rollback(checkpoint);
    assert!(planner.dispatch_reasons.is_empty());
    let root = planner
        .build(EntityId(1), None, None, 0, &BTreeSet::new())
        .unwrap();
    assert_eq!(planner.natural_attempts, attempts);
    assert_eq!(
        planner.dispatch_reasons[root.0],
        Some(ReadabilityReason::Expansion)
    );
    let cached = planner
        .build(EntityId(1), None, None, usize::MAX, &BTreeSet::new())
        .unwrap();
    assert_eq!(cached, root);
    let plan = Plan {
        nodes: planner.nodes,
        root,
        regions: planner.regions,
    };
    crate::structure::verify::verify(&ir, &plan, &mut |n| planner.owner.pay(n)).unwrap();
    let decisions = plan.decisions(&mut |n| planner.owner.pay(n)).unwrap();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].reason, ReadabilityReason::Expansion);
    assert!(decisions[0].natural.is_none());
}
