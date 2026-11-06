//! Local proof corruption tests call the verifier without normal-form rebuild.
use crate::{Config, Fork, Input, StageKind, decompile};
fn fixture() -> Vec<u8> {
    let mut code = Vec::new();
    // Establish slot 64 from source, not a compiler convention.
    code.extend([0x60, 0x80, 0x60, 0x40, 0x52]);
    let stores = [
        (0_u8, crate::U256::from(0x08c379a0_u32) << 224),
        (4, crate::U256::from(32)),
        (36, crate::U256::from(20)),
        (
            68,
            crate::U256::from_be_slice(b"ds-math-mul-overflow\0\0\0\0\0\0\0\0\0\0\0\0"),
        ),
    ];
    for (offset, value) in stores {
        code.push(0x7f);
        code.extend(value.to_be_bytes::<32>());
        code.extend([0x60, 0x40, 0x51, 0x60, offset, 0x01, 0x52]);
    }
    code.extend([0x60, 100, 0x60, 0x40, 0x51, 0xfd]);
    code
}
#[test]
fn byte_and_source_corruptions_fail_local_replay() {
    let artifact = decompile(&Input::new(fixture(), Fork::Osaka), &Config::default());
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted, "{:?}", artifact.diagnostics);
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(program.payloads.len(), 1, "{:?}", program.reverts);
    assert_eq!(program.payloads[0].message, "ds-math-mul-overflow");
    assert_eq!(program.constructions.len(), 1);
    assert_eq!(super::verify(&stage.ir, program, &mut |_| true), Some(true));
    let mut corrupt = program.clone();
    corrupt.payloads[0].message = "ds-math-mul-underflo".to_owned();
    assert_eq!(
        super::verify(&stage.ir, &corrupt, &mut |_| true),
        Some(false)
    );
    let mut corrupt = program.clone();
    corrupt.payloads[0].bytes[99] = 1;
    assert_eq!(
        super::verify(&stage.ir, &corrupt, &mut |_| true),
        Some(false)
    );
    let mut corrupt = program.clone();
    corrupt.reverts[0].writes.reverse();
    assert_eq!(
        super::verify(&stage.ir, &corrupt, &mut |_| true),
        Some(false)
    );
    let mut corrupt = program.clone();
    corrupt.reverts[0].memory = corrupt.reverts[0].offset;
    assert_eq!(
        super::verify(&stage.ir, &corrupt, &mut |_| true),
        Some(false)
    );
    let mut corrupt = program.clone();
    corrupt.constructions[0].start += 1;
    assert_eq!(
        super::verify(&stage.ir, &corrupt, &mut |_| true),
        Some(false)
    );
    assert_eq!(
        program
            .render_index(&stage.ir)
            .render_revert(program.reverts[0].block, |_| true)
            .unwrap(),
        Some("revert(\"ds-math-mul-overflow\");".to_owned())
    );
}

#[test]
fn source_edge_bounds_remain_conditional_and_close_a_symbolic_pointer() {
    // x=calldataload(0); if x<32 { mstore(64,128+x); original constructor }
    // The bound belongs to the selected edge, not to the calldata observation.
    let mut code = vec![
        0x5f, 0x35, 0x80, 0x60, 32, 0x90, 0x10, 0x60, 11, 0x57, 0x00, 0x5b, 0x60, 128, 0x01, 0x60,
        64, 0x52,
    ];
    code.extend_from_slice(&fixture()[5..]);
    let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted, "{:?}", artifact.diagnostics);
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(program.payloads.len(), 1);
    assert_eq!(super::verify(&stage.ir, program, &mut |_| true), Some(true));
    let mut wrong = stage.ir.clone();
    let branch = wrong
        .blocks
        .iter_mut()
        .find(|b| matches!(b.terminator, crate::Terminator::Branch { .. }))
        .unwrap();
    if let crate::Terminator::Branch {
        then_edges,
        else_edge,
        ..
    } = &mut branch.terminator
    {
        std::mem::swap(&mut then_edges[0], else_edge.as_mut().unwrap());
    }
    assert_eq!(super::verify(&wrong, program, &mut |_| true), Some(false));
}

#[test]
fn deep_acyclic_memory_address_dependencies_refuse_without_host_stack_growth() {
    let mut code = fixture()[..5].to_vec();
    for _ in 0..1_000 {
        code.extend([0x60, 1, 0x60, 64, 0x51, 0x52]);
    }
    code.extend_from_slice(&fixture()[5..]);
    let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted, "{:?}", artifact.diagnostics);
    assert!(stage.ir.lifted.as_ref().unwrap().reverts.is_empty());
}

#[test]
fn local_source_admission_reuses_only_matching_terminal_materializations() {
    let artifact = decompile(&Input::new(fixture(), Fork::Osaka), &Config::default());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let block = program.constructions[0].block;
    let mut ir = stage.ir.clone();
    let node = &mut ir.structured.as_mut().unwrap().root;
    node.kind = crate::StructuredKind::Dispatcher {
        entry: block,
        cases: vec![crate::DispatchCase {
            block,
            transitions: Vec::new(),
        }],
    };
    assert_eq!(super::verify(&ir, program, &mut |_| true), Some(true));
    if let crate::StructuredKind::Dispatcher { cases, .. } =
        &mut ir.structured.as_mut().unwrap().root.kind
    {
        cases[0].transitions.push(crate::DispatchTransition {
            id: crate::EntityId(u64::MAX),
            edge: crate::EntityId(u64::MAX - 1),
            target: block,
        });
    }
    assert_eq!(super::verify(&ir, program, &mut |_| true), Some(false));
}

#[test]
fn source_capsule_admission_rejects_an_omitted_store_even_with_forged_pure_flags() {
    let mut code = fixture()[..5].to_vec();
    code.extend([0x60, 9, 0x5f, 0x55]);
    code.extend_from_slice(&fixture()[5..]);
    let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let mut ir = artifact.current().unwrap().ir.clone();
    let raw = ir
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| o.opcode == 0x55)
        .unwrap();
    raw.effects = crate::Effects::default();
    let store = raw.id;
    let program = ir.lifted.as_mut().unwrap();
    let plan = &mut program.constructions[0];
    let body = program
        .blocks
        .iter_mut()
        .find(|b| b.block == plan.block)
        .unwrap();
    body.statements.retain(|s| s.operation != store);
    plan.start = 0;
    plan.end = body.statements.len();
    let program = ir.lifted.as_ref().unwrap();
    assert!(program.render_index(&ir).validate_guard_sources().is_err());
    assert_eq!(super::verify(&ir, program, &mut |_| true), Some(false));
}

#[test]
fn source_failure_bridges_reject_effectful_opcodes_with_forged_pure_flags() {
    // CALLVALUE selects STOP or a pure forwarding block before the constructor.
    let mut code = vec![
        0x34, 0x61, 0, 12, 0x57, 0x60, 9, 0x50, 0x61, 0, 14, 0x56, 0x5b, 0x00, 0x5b,
    ];
    code.extend(fixture());
    let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    let program = stage.ir.lifted.as_ref().unwrap();
    program.validate_guard_sources(&stage.ir).unwrap();
    let bridge = program
        .failures
        .iter()
        .find(|fact| fact.rank > 0)
        .expect("ranked bridge into an Error(string) failure")
        .block;
    assert!(
        program
            .controls
            .iter()
            .any(|control| control.guard.is_some())
    );
    for opcode in [0x51, 0x55, 0xf1, 0xfe] {
        let mut changed = stage.clone();
        let op = changed
            .ir
            .blocks
            .iter_mut()
            .find(|block| block.id == bridge)
            .unwrap()
            .operations
            .iter_mut()
            .find(|op| op.opcode == 0x50)
            .unwrap();
        op.opcode = opcode;
        op.effects = crate::Effects::default();
        let program = changed.ir.lifted.as_ref().unwrap();
        assert!(program.validate_guard_sources(&changed.ir).is_err());
        let index = program.render_index(&changed.ir);
        assert!(index.validate_guard_sources().is_err());
        for control in &program.controls {
            assert!(index.guard(control.node).is_none());
        }
        assert_eq!(
            super::verify(&changed.ir, program, &mut |_| true),
            Some(false)
        );
    }
}

#[test]
fn stage_diff_reports_payload_identity_changes_even_when_the_rank_is_unchanged() {
    let artifact = decompile(&Input::new(fixture(), Fork::Osaka), &Config::default());
    let before = &artifact.current().unwrap().ir;
    let mut after = before.clone();
    let program = after.lifted.as_mut().unwrap();
    let fact = program
        .failures
        .iter_mut()
        .find(|fact| matches!(fact.payload, crate::LiftedGuardPayload::ErrorString(_)))
        .unwrap();
    let block = fact.block;
    fact.payload = crate::LiftedGuardPayload::EmptyRevert;
    program.validate().unwrap();
    let owner = program.block(block).unwrap().id;
    let diff = crate::diff_ir(before, &after);
    assert_eq!(diff.changed, vec![owner]);
    assert!(diff.added.is_empty() && diff.removed.is_empty());
    assert!(crate::diff_ir(before, before).changed.is_empty());
}

#[test]
fn local_edge_bounds_require_the_actual_condition_producer_at_every_step() {
    for inversions in [0, 2] {
        let mut code = vec![0x5f, 0x35, 0x80, 0x60, 32, 0x90, 0x10];
        code.extend(std::iter::repeat_n(0x15, inversions));
        let target = code.len() + 1;
        code.extend([0x60, 0, 0x57, 0x00]);
        code[target] = u8::try_from(code.len()).unwrap();
        code.extend([0x5b, 0x60, 128, 0x01, 0x60, 64, 0x52]);
        code.extend_from_slice(&fixture()[5..]);
        let artifact = decompile(&Input::new(code, Fork::Osaka), &Config::default());
        let ir = &artifact.current().unwrap().ir;
        let program = ir.lifted.as_ref().unwrap();
        assert_eq!(program.payloads.len(), 1);
        assert_eq!(super::verify(ir, program, &mut |_| true), Some(true));
        let producers: Vec<_> = ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter(|op| matches!(op.opcode, 0x10 | 0x15))
            .map(|op| op.id)
            .collect();
        assert_eq!(producers.len(), inversions + 1);
        for producer in producers {
            let mut changed = ir.clone();
            changed
                .blocks
                .iter_mut()
                .flat_map(|block| &mut block.operations)
                .find(|op| op.id == producer)
                .unwrap()
                .outputs[0] = crate::EntityId(u64::MAX);
            // The local verifier must reject before any whole-stage SSA check:
            // the forged ValueKind link no longer names its actual producer.
            assert_eq!(super::verify(&changed, program, &mut |_| true), Some(false));
        }
    }
}
