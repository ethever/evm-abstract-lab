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

fn extent_loop(output: Option<u64>, ancestor_dirty: bool, later_dirty: bool) -> crate::Ir {
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
    if ancestor_dirty {
        code.number(0xa5)
            .number(64)
            .op(0x51)
            .number(108)
            .op(0x01)
            .op(0x52);
    }
    code.number(0)
        .dest("head")
        .op(0x80)
        .number(128)
        .op(0x51)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50);
    if later_dirty {
        code.number(0xa5)
            .number(64)
            .op(0x51)
            .number(108)
            .op(0x01)
            .op(0x52);
    }
    code.number(24)
        .number(64)
        .op(0x51)
        .number(108)
        .op(0x01)
        .op(0xfd)
        .dest("body")
        .number(64)
        .op(0x51)
        .number(189)
        .op(0x01)
        .number(64)
        .op(0x52);
    if let Some(output) = output {
        code.number(output);
    } else {
        code.number(32).op(0x35);
    }
    code.number(64)
        .op(0x51)
        .number(0)
        .number(0)
        .number(1)
        .number(255)
        .op(0xfa)
        .op(0x50)
        // A zero-sized copy at an unknown destination preserves the bound.
        .number(0)
        .number(0)
        .number(32)
        .op(0x35)
        .op(0x39)
        .number(1)
        .op(0x01)
        .target("head")
        .op(0x56)
        .dest("reject")
        .op(0x00);
    decompile(&code.finish(), &Config::default())
        .stages
        .into_iter()
        .find(|s| s.kind == StageKind::Structured)
        .unwrap()
        .ir
}

fn extent_candidate(ir: &crate::Ir) -> (Vec<EntityId>, Candidate) {
    let mut source = Source::new(ir, |_| true).unwrap();
    let mut flows = BTreeMap::new();
    for group in super::super::super::cycles::components(ir, &mut |_| true).unwrap() {
        if group.len() > 1 {
            let shape = source.shape(&group).unwrap();
            let initial = source.entry_state(&shape, &flows).unwrap();
            return (group, source.propose(&shape, &initial).unwrap());
        }
        let block = source.blocks[&group[0]];
        let flow = if Some(block.id) == ir.entry {
            Flow::Known(State {
                extent: source.empty_extent(block),
                memory: Some(block.memory_parameter),
                effect: Some(block.effect_parameter),
                ..State::default()
            })
        } else {
            source.join(block, &flows, None)
        };
        source.process(block, flow, None, false, &mut flows, false);
    }
    panic!("one actual source loop");
}

fn tail_zero(ir: &crate::Ir) -> Option<usize> {
    let terminal = ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|o| o.opcode == 0xfd)
        .unwrap();
    let mut memory = super::super::super::Memory::new(ir, |_| true).unwrap();
    let base = memory.word(terminal.inputs[0])?;
    memory.zero_suffix(terminal.memory_input.unwrap(), base, 24)
}

#[test]
fn zero_suffix_induction_covers_calls_and_rejects_actual_history_clobbers() {
    let clean = extent_loop(Some(96), false, false);
    assert_eq!(tail_zero(&clean), Some(0));
    for dirty in [
        extent_loop(Some(160), false, false),
        extent_loop(None, false, false),
        extent_loop(Some(96), true, false),
        extent_loop(Some(96), false, true),
    ] {
        assert_eq!(tail_zero(&dirty), None);
    }
    let mut unknown_copy = clean;
    let copy = unknown_copy
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| o.opcode == 0x39)
        .unwrap();
    copy.inputs[2] = copy.inputs[0];
    assert_eq!(tail_zero(&unknown_copy), None);
}

#[test]
fn fresh_extent_verifier_rejects_forged_steps_and_changed_call_output_widths() {
    let ir = extent_loop(Some(96), false, false);
    let (component, candidate) = extent_candidate(&ir);
    assert_eq!(candidate.extent.as_ref().unwrap().step, U256::from(189));
    assert!(accepted(&ir, &component, &candidate));
    let mut forged = candidate.clone();
    forged.extent.as_mut().unwrap().step = U256::ZERO;
    let mut verifier = Source::new(&ir, |_| true).unwrap();
    assert!(
        verifier
            .verify_candidate(&component, &forged, &BTreeMap::new())
            .is_none()
    );
    assert!(
        verifier.admitted_extents.is_empty(),
        "failed hypotheses must not publish token bounds"
    );
    assert!(verifier.admitted.is_empty());

    let mut changed = ir.clone();
    let width = changed
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|o| o.opcode == 0xfa)
        .unwrap()
        .inputs[5];
    changed
        .values
        .iter_mut()
        .find(|v| v.id == width)
        .unwrap()
        .kind = crate::ValueKind::Constant(U256::from(160));
    assert!(!accepted(&changed, &component, &candidate));
}
