//! Direct admission cannot assume a forged Passed/Failed snapshot has a valid
//! base source envelope, guard tree or construction capsule.
use super::{checked, fixtures};
use evm_decompile::{EntityId, Stage, ValidationStatus};

#[derive(Clone, Copy, Debug)]
enum Mutation {
    MissingHighTarget,
    MissingRawTarget,
    MissingRawEdge,
    DanglingRawTarget,
    DanglingRawSource,
    DanglingHighTransfer,
    DanglingHighBlock,
    GuardSelfPass,
    ConstructionSpan,
    ConstructionIdentity,
    PayloadIdentity,
}

fn mutate(stage: &mut Stage, mutation: Mutation) {
    match mutation {
        Mutation::MissingHighTarget => {
            let target = stage.ir.edges[0].to;
            let high = stage.ir.lifted.as_mut().unwrap();
            assert!(high.blocks.iter().any(|block| block.block == target));
            high.blocks.retain(|block| block.block != target);
        }
        Mutation::MissingRawTarget => {
            let target = stage.ir.edges[0].to;
            stage.ir.blocks.retain(|block| block.id != target);
        }
        Mutation::MissingRawEdge => {
            stage.ir.edges.remove(0);
        }
        Mutation::DanglingRawTarget => stage.ir.edges[0].to = EntityId(u64::MAX),
        Mutation::DanglingRawSource => stage.ir.edges[0].from = EntityId(u64::MAX),
        Mutation::DanglingHighTransfer => {
            stage.ir.lifted.as_mut().unwrap().transfers[0].edge = EntityId(u64::MAX)
        }
        Mutation::DanglingHighBlock => {
            stage.ir.lifted.as_mut().unwrap().blocks[0].block = EntityId(u64::MAX)
        }
        Mutation::GuardSelfPass => {
            let control = stage
                .ir
                .lifted
                .as_mut()
                .unwrap()
                .controls
                .iter_mut()
                .find(|control| control.guard.is_some())
                .unwrap();
            control.guard.as_mut().unwrap().pass_node = control.node;
        }
        Mutation::ConstructionSpan => {
            stage.ir.lifted.as_mut().unwrap().constructions[0].end = usize::MAX
        }
        Mutation::ConstructionIdentity => {
            let high = stage.ir.lifted.as_mut().unwrap();
            assert!(high.reverts.len() >= 2);
            high.constructions[0].revert = high
                .reverts
                .iter()
                .find(|proof| proof.block != high.constructions[0].block)
                .unwrap()
                .id;
        }
        Mutation::PayloadIdentity => {
            let high = stage.ir.lifted.as_mut().unwrap();
            let mut payload = high.payloads[0].clone();
            payload.id = EntityId(u64::MAX);
            payload.message.replace_range(..1, "S");
            payload.bytes[68] = b'S';
            assert!(payload.is_canonical());
            high.reverts[0].payload = payload.id;
            high.payloads.push(payload);
        }
    }
}

#[test]
fn malformed_base_envelopes_and_guard_capsules_refuse_without_panics_or_omissions() {
    let fixture = fixtures::sequential();
    let artifact = checked(&fixture.input);
    assert!(
        !artifact
            .current()
            .unwrap()
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .procedures
            .is_empty()
    );
    let artifact = &artifact;
    // Automatic scope joining attempts every case and fails the test on any panic.
    std::thread::scope(|scope| {
        for mutation in [
            Mutation::MissingHighTarget,
            Mutation::MissingRawTarget,
            Mutation::MissingRawEdge,
            Mutation::DanglingRawTarget,
            Mutation::DanglingRawSource,
            Mutation::DanglingHighTransfer,
            Mutation::DanglingHighBlock,
            Mutation::GuardSelfPass,
            Mutation::ConstructionSpan,
            Mutation::ConstructionIdentity,
            Mutation::PayloadIdentity,
        ] {
            for status in [ValidationStatus::Passed, ValidationStatus::Failed] {
                scope.spawn(move || {
                    let mut stage = artifact.current().unwrap().clone();
                    stage.validation.status = status;
                    mutate(&mut stage, mutation);
                    let high = stage.ir.lifted.as_ref().unwrap();
                    assert!(
                        high.validate_shared_sources(&stage.ir, |_| true).is_err(),
                        "admitted base {mutation:?}"
                    );
                    let index = high.render_index(&stage.ir);
                    assert_eq!(index.procedures().count(), 0, "unsafe base {mutation:?}");
                    for call in &high.invocations {
                        assert!(index.shared_invocation(call.source).is_none());
                    }
                    eprintln!(
                        "malformed base source rejected without panic: {mutation:?} / {status:?}"
                    );
                });
            }
        }
    });
}

#[test]
fn supplied_internal_loop_ports_cannot_be_removed_duplicated_or_redirected() {
    let fixture = fixtures::closed_loop_calls();
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let original = stage.ir.lifted.as_ref().unwrap();
    let position = original
        .procedures
        .iter()
        .position(|procedure| !procedure.interface.continuations.is_empty())
        .expect("actual internal loop exit port");
    for mutation in ["missing", "extra", "target"] {
        let mut supplied = original.clone();
        let ports = &mut supplied.procedures[position].interface.continuations;
        match mutation {
            "missing" => {
                ports.pop().unwrap();
            }
            "extra" => ports.push(ports[0].clone()),
            "target" => ports[0].target = stage.ir.entry.unwrap(),
            _ => unreachable!(),
        }
        assert!(
            supplied
                .validate_shared_sources(&stage.ir, |_| true)
                .is_err(),
            "admitted {mutation} loop port"
        );
        assert_eq!(supplied.render_index(&stage.ir).procedures().count(), 0);
    }
}
