use super::*;
use evm_decompile::{EntityId, ValidationStatus, validate_stage};

#[derive(Clone, Copy, Debug)]
enum Corruption {
    Message,
    Length,
    Padding,
    Offset,
    Pointer,
    Memory,
    WriteOrder,
    MissingWrite,
    ForeignWrite,
    SelfPayload,
    SelfRevert,
    RegionStart,
    RegionEnd,
    RegionOwner,
    GuardPayload,
    FailurePayload,
}

fn corrupt(stage: &mut evm_decompile::Stage, how: Corruption) {
    let program = stage.ir.lifted.as_mut().unwrap();
    match how {
        Corruption::Message => {
            program.payloads[0].message = "a different certified error".into();
            program.payloads[0].bytes = oracle::abi(program.payloads[0].message.as_bytes());
        }
        Corruption::Length => program.payloads[0].bytes[67] ^= 1,
        Corruption::Padding => *program.payloads[0].bytes.last_mut().unwrap() = 1,
        Corruption::Offset => program.payloads[0].bytes[35] = 64,
        Corruption::Pointer => program.reverts[0].offset = program.reverts[0].size,
        Corruption::Memory => program.reverts[0].memory = program.reverts[0].offset,
        Corruption::WriteOrder => program.reverts[0].writes.reverse(),
        Corruption::MissingWrite => {
            program.reverts[0].writes.pop();
        }
        Corruption::ForeignWrite => program.reverts[0].writes[0] = program.reverts[0].operation,
        Corruption::SelfPayload => program.reverts[0].payload = program.reverts[0].id,
        Corruption::SelfRevert => program.constructions[0].revert = program.constructions[0].id,
        Corruption::RegionStart => program.constructions[0].start = program.constructions[0].end,
        Corruption::RegionEnd => program.constructions[0].end += 1,
        Corruption::RegionOwner => program.constructions[0].block = stage.ir.entry.unwrap(),
        Corruption::GuardPayload => {
            let guard = program
                .controls
                .iter_mut()
                .find_map(|c| c.guard.as_mut())
                .unwrap();
            guard.payload = LiftedGuardPayload::EmptyRevert;
        }
        Corruption::FailurePayload => program.failures[0].payload = LiftedGuardPayload::EmptyRevert,
    }
}

#[test]
fn semantic_validator_rejects_forged_content_versions_order_and_payload_identity() {
    let input = fixtures::guarded(DS_MATH, false, None);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    for how in [
        Corruption::Message,
        Corruption::Length,
        Corruption::Padding,
        Corruption::Offset,
        Corruption::Pointer,
        Corruption::Memory,
        Corruption::WriteOrder,
        Corruption::MissingWrite,
        Corruption::ForeignWrite,
        Corruption::SelfPayload,
        Corruption::SelfRevert,
        Corruption::RegionStart,
        Corruption::RegionEnd,
        Corruption::RegionOwner,
        Corruption::GuardPayload,
        Corruption::FailurePayload,
    ] {
        let mut wrong = artifact.current().unwrap().clone();
        corrupt(&mut wrong, how);
        assert_eq!(
            wrong.validation.status,
            ValidationStatus::Passed,
            "forged stored Passed is deliberately untrusted"
        );
        assert_ne!(
            validate_stage(&wrong).status,
            ValidationStatus::Passed,
            "accepted {how:?}"
        );
    }
}

#[test]
fn lightweight_render_admission_quarantines_recursive_foreign_and_wrong_region_plans() {
    let input = fixtures::guarded(DS_MATH, false, None);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    for how in [
        Corruption::Pointer,
        Corruption::Memory,
        Corruption::ForeignWrite,
        Corruption::SelfPayload,
        Corruption::SelfRevert,
        Corruption::RegionStart,
        Corruption::RegionEnd,
        Corruption::RegionOwner,
    ] {
        let mut stage = artifact.current().unwrap().clone();
        corrupt(&mut stage, how);
        let program = stage.ir.lifted.as_ref().unwrap();
        let index = program.render_index(&stage.ir);
        assert!(
            index.validate_guard_sources().is_err(),
            "lightweight source admission accepted {how:?}"
        );
        for control in &program.controls {
            assert!(
                index.guard(control.node).is_none(),
                "forged source leaked guard overlay"
            );
        }
        for construction in &program.constructions {
            assert!(
                index.construction(construction.block).is_none(),
                "forged source leaked capsule"
            );
            assert!(index.revert(construction.block).is_none());
        }
    }
    let mut stage = artifact.current().unwrap().clone();
    stage.ir.lifted.as_mut().unwrap().constructions[0].revert = EntityId(u64::MAX);
    assert!(
        stage
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .validate_guard_sources(&stage.ir)
            .is_err()
    );
}

#[test]
fn guard_failure_identity_cannot_point_at_another_valid_error_constructor() {
    let input = fixtures::two_errors();
    let artifact = checked(&input);
    assert_eq!(lifted(&artifact).1.payloads.len(), 2);
    let mut wrong = artifact.current().unwrap().clone();
    let program = wrong.ir.lifted.as_mut().unwrap();
    let proof = program.reverts.first_mut().unwrap();
    let other = program
        .payloads
        .iter()
        .find(|p| p.id != proof.payload)
        .unwrap();
    proof.payload = other.id;
    assert_ne!(validate_stage(&wrong).status, ValidationStatus::Passed);
}

#[test]
fn nonempty_guard_sources_reject_self_ancestor_other_arm_and_out_of_tree_targets() {
    let input = fixtures::guarded(DS_MATH, false, None);
    let artifact = checked(&input);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let control = program.controls.iter().find(|c| c.guard.is_some()).unwrap();
    let guard = control.guard.as_ref().unwrap();
    let ancestor = stage.ir.structured.as_ref().unwrap().root.id;
    for target in [control.node, ancestor, guard.pass_node, EntityId(u64::MAX)] {
        let mut wrong = stage.clone();
        let program = wrong.ir.lifted.as_mut().unwrap();
        program
            .controls
            .iter_mut()
            .find(|c| c.node == control.node)
            .unwrap()
            .guard
            .as_mut()
            .unwrap()
            .failure_node = target;
        let program = wrong.ir.lifted.as_ref().unwrap();
        let index = program.render_index(&wrong.ir);
        assert!(
            index.validate_guard_sources().is_err(),
            "admitted forged nonempty source {target:?}"
        );
        assert!(index.guard(control.node).is_none());
        assert_ne!(validate_stage(&wrong).status, ValidationStatus::Passed);
    }
}

#[test]
fn a_constructor_plan_cannot_expand_to_absorb_an_unrelated_observing_prefix() {
    let input = fixtures::guarded(DS_MATH, false, Some(0x55));
    let artifact = checked(&input);
    let mut wrong = artifact.current().unwrap().clone();
    let program = wrong.ir.lifted.as_mut().unwrap();
    let construction = program
        .constructions
        .first_mut()
        .expect("later payload suffix remains printable");
    assert!(construction.start > 0);
    construction.start = 0;
    let program = wrong.ir.lifted.as_ref().unwrap();
    assert!(
        program
            .render_index(&wrong.ir)
            .validate_guard_sources()
            .is_err()
    );
    assert_ne!(validate_stage(&wrong).status, ValidationStatus::Passed);
}

#[test]
fn one_exact_constructor_range_can_be_materialized_at_multiple_raw_source_occurrences() {
    use evm_decompile::{StructuredKind, StructuredNode};
    let input = fixtures::standalone(DS_MATH, Mutation::None);
    let artifact = checked(&input);
    let mut stage = artifact.current().unwrap().clone();
    let block = stage.ir.lifted.as_ref().unwrap().constructions[0].block;
    let root = &mut stage.ir.structured.as_mut().unwrap().root;
    let StructuredKind::Sequence(children) = &mut root.kind else {
        panic!("standalone fixture needs its explicit block / exit source pair")
    };
    // A second source materialization has distinct syntax identities. It uses
    // the exact same source body and terminal, without recursive AST redirects.
    children.push(StructuredNode {
        id: EntityId(u64::MAX - 2),
        kind: StructuredKind::Block { block },
    });
    children.push(StructuredNode {
        id: EntityId(u64::MAX - 1),
        kind: StructuredKind::Exit { block },
    });
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = program.render_index(&stage.ir);
    index.validate_guard_sources().unwrap();
    assert!(index.construction(block).is_some());
    assert_eq!(
        index.render_revert(block, |_| true).unwrap(),
        Some("evm.revert(\"ds-math-mul-overflow\");".into())
    );
}
