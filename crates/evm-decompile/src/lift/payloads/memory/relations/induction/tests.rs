//! A verifier must replay predecessor loop proofs without rediscovering them.
use super::source_proof::assembly::Asm;
use super::*;
use crate::{Config, StageKind, decompile};
fn serial() -> crate::Ir {
    let mut code = Asm::default();
    code.number(0)
        .op(0x35)
        .op(0x80)
        .number(7)
        .op(0x90)
        .op(0x11)
        .target("reject")
        .op(0x57)
        .op(0x80)
        .number(128)
        .op(0x52)
        .number(13)
        .op(0x02)
        .number(512)
        .op(0x01)
        .number(64)
        .op(0x52);
    for (head, body, end, growth) in [
        ("first_head", "first_body", "first_end", 37),
        ("second_head", "second_body", "second_end", 41),
    ] {
        code.number(0)
            .dest(head)
            .op(0x80)
            .number(128)
            .op(0x51)
            .op(0x90)
            .op(0x10)
            .target(body)
            .op(0x57)
            .target(end)
            .op(0x56)
            .dest(body)
            .number(64)
            .op(0x51)
            .number(growth)
            .op(0x01)
            .number(64)
            .op(0x52)
            .number(1)
            .op(0x01)
            .target(head)
            .op(0x56)
            .dest(end)
            .op(0x50);
    }
    code.op(0x00).dest("reject").op(0x00);
    decompile(&code.finish(), &Config::default())
        .stages
        .into_iter()
        .find(|s| s.kind == StageKind::Structured)
        .unwrap()
        .ir
}
fn second(ir: &crate::Ir) -> (Vec<EntityId>, Vec<EntityId>, Candidate) {
    let mut source = Source::new(ir, |_| true).unwrap();
    let mut flows = BTreeMap::new();
    let mut first = None;
    for group in super::super::super::cycles::components(ir, &mut |_| true).unwrap() {
        if group.len() == 1 {
            let block = source.blocks[&group[0]];
            let flow = if Some(block.id) == ir.entry {
                Flow::Known(State {
                    memory: Some(block.memory_parameter),
                    effect: Some(block.effect_parameter),
                    ..State::default()
                })
            } else {
                source.join(block, &flows, None)
            };
            source.process(block, flow, None, false, &mut flows, false);
        } else if first.is_none() {
            source.loop_region(&group, &mut flows);
            first = Some(group);
        } else {
            let shape = source.shape(&group).unwrap();
            let initial = source.entry_state(&shape, &flows).unwrap();
            return (
                first.unwrap(),
                group,
                source.propose(&shape, &initial).unwrap(),
            );
        }
    }
    panic!("two actual source loops");
}
fn accepted(ir: &crate::Ir, component: &[EntityId], candidate: &Candidate) -> bool {
    Source::new(ir, |_| true)
        .unwrap()
        .verify_candidate(component, candidate, &BTreeMap::new())
        .is_some()
}
#[test]
fn predecessor_loop_certificate_is_replayed_from_fresh_source() {
    let ir = serial();
    let (first, second, candidate) = second(&ir);
    assert_eq!(candidate.dependencies.len(), 1);
    assert!(accepted(&ir, &second, &candidate));
    let mut missing = candidate.clone();
    missing.dependencies.clear();
    assert!(!accepted(&ir, &second, &missing));
    let mut forged = candidate.clone();
    forged.dependencies.values_mut().next().unwrap().bound = U256::ZERO;
    assert!(!accepted(&ir, &second, &forged));
    let step = ir.blocks.iter().filter(|b| first.contains(&b.id)).flat_map(|b| &b.operations).find(|o| o.opcode == 0x01 && o.inputs.iter().any(|id| matches!(ir.value(*id).map(|v| &v.kind), Some(crate::ValueKind::Constant(c)) if *c == U256::from(37)))).unwrap().id;
    let mut changed = ir.clone();
    changed
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| o.id == step)
        .unwrap()
        .opcode = 0x02;
    assert!(!accepted(&changed, &second, &candidate));
}
