//! Whole-fixture profitability counts executable body materializations, not
//! just definitions, comments or source-navigation metadata.
use evm_decompile::{EntityId, StructuredKind, StructuredNode, render_text};
use std::collections::{BTreeMap, BTreeSet};

fn visit<'a>(node: &'a StructuredNode, nodes: &mut BTreeMap<EntityId, &'a StructuredNode>) {
    nodes.insert(node.id, node);
    match &node.kind {
        StructuredKind::Sequence(children) => {
            for child in children {
                visit(child, nodes);
            }
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            visit(then_branch, nodes);
            visit(else_branch, nodes);
        }
        StructuredKind::While { header, body, .. } => {
            visit(header, nodes);
            visit(body, nodes);
        }
        StructuredKind::Indirect { arms, .. } => {
            for arm in arms {
                visit(&arm.body, nodes);
            }
        }
        _ => {}
    }
}

fn bodies(node: &StructuredNode, omitted: &BTreeSet<EntityId>, blocks: &mut Vec<EntityId>) {
    if omitted.contains(&node.id) {
        return;
    }
    match &node.kind {
        StructuredKind::Block { block } => blocks.push(*block),
        StructuredKind::Sequence(children) => {
            for child in children {
                bodies(child, omitted, blocks);
            }
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            bodies(then_branch, omitted, blocks);
            bodies(else_branch, omitted, blocks);
        }
        StructuredKind::While { header, body, .. } => {
            bodies(header, omitted, blocks);
            bodies(body, omitted, blocks);
        }
        StructuredKind::Indirect { arms, .. } => {
            for arm in arms {
                bodies(&arm.body, omitted, blocks);
            }
        }
        StructuredKind::Dispatcher { cases, .. } => {
            blocks.extend(cases.iter().map(|case| case.block))
        }
        _ => {}
    }
}

#[test]
fn whole_router_must_share_multiblock_and_context_bodies_with_meaningful_reduction() {
    let (_, artifact) = super::router();
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "whole Router sharing is mandatory"
    );
    assert!(
        high.invocations.len() >= 2,
        "real reusable invocations are mandatory"
    );
    high.validate_shared_sources(&stage.ir, |_| true).unwrap();
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let mut nodes = BTreeMap::new();
    visit(root, &mut nodes);
    let mut before = Vec::new();
    bodies(root, &BTreeSet::new(), &mut before);
    let omitted = high.invocations.iter().map(|call| call.source).collect();
    let mut after = Vec::new();
    bodies(root, &omitted, &mut after);
    let mut substantial_multiblock = 0;
    let mut alpha_context_groups = 0;
    let mut repeated_source_groups = 0;
    let mut arithmetic_definitions = 0;
    for procedure in &high.procedures {
        let Some(body) = nodes.get(&procedure.body) else {
            assert!(high.expression(procedure.body).is_some());
            assert_eq!(
                procedure.kind,
                evm_decompile::SharedProcedureKind::PureArithmetic
            );
            arithmetic_definitions += 1;
            continue;
        };
        let mut definition_blocks = Vec::new();
        let mut nested = omitted.clone();
        nested.remove(&procedure.body);
        bodies(body, &nested, &mut definition_blocks);
        let distinct: BTreeSet<_> = definition_blocks.iter().copied().collect();
        let operations: usize = distinct
            .iter()
            .map(|id| stage.ir.block(*id).unwrap().operations.len())
            .sum();
        for block in &distinct {
            for operation in &stage.ir.block(*block).unwrap().operations {
                assert!(
                    !matches!(operation.opcode, 0x00 | 0xf0..=0xf5 | 0xfa | 0xfd..=0xff),
                    "procedure hid an EVM terminal or external call"
                );
            }
        }
        substantial_multiblock += usize::from(distinct.len() >= 2 && operations >= 8);
        after.extend(definition_blocks);
        let mut same_sources = BTreeMap::<Vec<EntityId>, usize>::new();
        let mut alpha = false;
        for call in high
            .invocations
            .iter()
            .filter(|call| call.procedure == procedure.id)
        {
            let actual: Vec<_> = distinct
                .iter()
                .map(|id| call.actual(*id).unwrap())
                .collect();
            *same_sources.entry(actual).or_default() += 1;
            alpha |= distinct.iter().any(|id| {
                let actual = call.actual(*id).unwrap();
                *id != actual
                    && stage.ir.block(*id).unwrap().source.map(|s| s.pc)
                        == stage.ir.block(actual).unwrap().source.map(|s| s.pc)
            });
        }
        alpha_context_groups += usize::from(alpha);
        repeated_source_groups += usize::from(same_sources.values().any(|count| *count >= 2));
    }
    assert!(
        substantial_multiblock > 0,
        "must share useful real multi-block executable regions"
    );
    assert!(
        alpha_context_groups > 0,
        "must parameterize genuinely distinct source contexts"
    );
    assert!(
        repeated_source_groups > 0,
        "must share repeated occurrences of the same source region"
    );
    assert!(
        after.len() * 10 <= before.len() * 9,
        "executable materializations decreased less than 10%: {} -> {}",
        before.len(),
        after.len()
    );
    let shared_text = render_text(artifact);
    let mut unshared = artifact.clone();
    let unshared_high = unshared
        .stages
        .last_mut()
        .unwrap()
        .ir
        .lifted
        .as_mut()
        .unwrap();
    unshared_high.procedures.clear();
    unshared_high.invocations.clear();
    let unshared_text = render_text(&unshared);
    assert!(
        shared_text.len() * 10 <= unshared_text.len() * 9,
        "whole Router text decreased less than 10%: {} -> {}",
        unshared_text.len(),
        shared_text.len()
    );
    super::assert_error_source_coverage(artifact);
    let bindings: usize = high
        .invocations
        .iter()
        .map(|call| call.bindings.len())
        .sum();
    let source_entities = stage.ir.blocks.len()
        + stage.ir.edges.len()
        + stage.ir.values.len()
        + nodes.len()
        + stage
            .ir
            .blocks
            .iter()
            .map(|block| block.operations.len())
            .sum::<usize>();
    assert!(
        bindings <= source_entities.saturating_mul(4),
        "source-size proportional certificate storage"
    );
    eprintln!(
        "Router sharing: {} procedures ({arithmetic_definitions} arithmetic expressions), {} invocations, {substantial_multiblock} substantial multi-block definitions, {alpha_context_groups} alpha-context groups, {repeated_source_groups} repeated-source groups; bodies {} -> {}; bytes {} -> {}; lines {} -> {}; {bindings} binding records / {source_entities} source entities; {} terminal proofs / {} capsules",
        high.procedures.len(),
        high.invocations.len(),
        before.len(),
        after.len(),
        unshared_text.len(),
        shared_text.len(),
        unshared_text.lines().count(),
        shared_text.lines().count(),
        high.reverts.len(),
        high.constructions.len()
    );
}
