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
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS);
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
    let mut planner = Planner::new(&ir, &facts, &mut owner, MAX_PLAN_SLOTS);
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
    assert_eq!(planner.common_join(left, right, None), Ok(Some(tail)));
    assert!(planner.owner.budget.used() - before > 8);
    planner.rollback(checkpoint);
    let before = planner.owner.budget.used();
    assert_eq!(planner.common_join(left, right, None), Ok(Some(tail)));
    assert_eq!(
        planner.owner.budget.used() - before,
        8,
        "the source fact requires no second graph walk"
    );
    let before = planner.owner.budget.used();
    assert_eq!(planner.common_join(failed, tail, None), Ok(None));
    assert_eq!(
        planner.owner.budget.used() - before,
        8,
        "an empty candidate set performs no distance scan"
    );
    planner.rollback(checkpoint);
    assert_eq!(planner.join_cache.len(), 2);
    assert_eq!(planner.common_join(failed, tail, None), Ok(None));
}
