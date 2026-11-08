//! Guard regressions use real bytecode and an independent proof/trace oracle.
use super::support::{Asm, calldata, checked, validated};
use super::{assert_unchanged_witness, lifted, structured};
use evm_decompile::{
    Artifact, Completion, Config, EntityId, ExecutionOutcome, ExitKind, ExternalResponse, Input,
    MemoryHost, Stage, StageKind, Strategy, StructuredKind, StructuredNode, Terminator, U256,
    ValidationStatus, decompile, render_text, validate_stage,
};

#[path = "guards/certificates.rs"]
mod certificates;
#[path = "guards/checker.rs"]
mod checker;
#[path = "guards/fixtures.rs"]
mod fixtures;
#[path = "guards/resources.rs"]
mod resources;

use fixtures::{Failure, decoder, decoder_data, decoder_host, direct};

fn guards(artifact: &Artifact) -> usize {
    lifted(artifact)
        .1
        .controls
        .iter()
        .filter(|control| control.guard.is_some())
        .count()
}

fn certify(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
) -> (evm_decompile::Execution, MemoryHost) {
    checker::certify(input, artifact, host)
}

fn reverted(raw: &evm_decompile::Execution, host: &MemoryHost) -> Option<Vec<u8>> {
    let ExecutionOutcome::Exit {
        kind: ExitKind::Revert,
        values,
    } = &raw.outcome
    else {
        return None;
    };
    let size = usize::try_from(values[1]).unwrap();
    if size == 0 {
        return Some(Vec::new());
    }
    let offset = usize::try_from(values[0]).unwrap();
    Some(host.memory[offset..offset + size].to_vec())
}

#[test]
fn direct_guards_preserve_both_polarities_empty_bytes_and_pass_store() {
    for failure_nonzero in [false, true] {
        for offset in [U256::ZERO, U256::from(17), U256::MAX] {
            let input = direct(failure_nonzero, Failure::Empty(offset));
            let artifact = checked(&input);
            assert_eq!(guards(&artifact), 1, "{}", render_text(&artifact));
            checker::certificate(lifted(&artifact).0);
            let text = render_text(&artifact);
            assert_eq!(text.matches("require(").count(), 1, "{text}");
            assert!(!text.contains("if ("), "{text}");
            for value in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
                let (raw, host) = certify(
                    &input,
                    &artifact,
                    MemoryHost::new(&input, calldata(&[value])),
                );
                if (value != U256::ZERO) == failure_nonzero {
                    assert_eq!(reverted(&raw, &host), Some(Vec::new()));
                    assert!(host.storage.is_empty());
                } else {
                    assert!(matches!(
                        raw.outcome,
                        ExecutionOutcome::Exit {
                            kind: ExitKind::Stop,
                            ..
                        }
                    ));
                    assert_eq!(host.storage[&U256::ZERO], U256::from(0x55));
                }
                assert_eq!(raw.trace.iter().filter(|op| op.opcode == 0x35).count(), 1);
            }
            assert_unchanged_witness(&artifact);
        }
    }
}

#[test]
fn one_artifact_mixes_requires_with_observing_raw_and_invalid_failed_ifs() {
    for failure_nonzero in [false, true] {
        for failure in ["observation", "raw", "invalid"] {
            let mut asm = Asm::default();
            asm.op(0x5f)
                .op(0x35)
                .target(if failure_nonzero {
                    "first_failed"
                } else {
                    "second"
                })
                .op(0x57);
            if !failure_nonzero {
                asm.op(0x5f).op(0x5f).op(0xfd).dest("second");
            }
            asm.push(32)
                .op(0x35)
                .target(if failure_nonzero {
                    "second_failed"
                } else {
                    "done"
                })
                .op(0x57);
            if failure_nonzero {
                asm.op(0x00).dest("second_failed");
            }
            match failure {
                "observation" => {
                    asm.op(0x5f).op(0x54).op(0x50).op(0x5f).op(0x5f).op(0xfd);
                }
                "raw" => {
                    asm.push(0x42).op(0x5f).op(0x53).push(1).op(0x5f).op(0xfd);
                }
                "invalid" => {
                    asm.op(0xfe);
                }
                _ => unreachable!(),
            }
            if failure_nonzero {
                asm.dest("first_failed").op(0x5f).op(0x5f).op(0xfd);
            } else {
                asm.dest("done").op(0x00);
            }
            let input = asm.finish();
            let artifact = checked(&input);
            let (stage, high) = lifted(&artifact);
            let index = high.render_index(&stage.ir);
            let boundary = stage.ir.structured.as_ref().unwrap().root.id;
            assert_eq!(
                high.controls
                    .iter()
                    .filter(|c| matches!(
                        index.if_plan(c.node, boundary),
                        evm_decompile::LiftedIfPlan::Require(_)
                    ))
                    .count(),
                1
            );
            assert_eq!(
                high.controls
                    .iter()
                    .filter(|c| matches!(
                        index.if_plan(c.node, boundary),
                        evm_decompile::LiftedIfPlan::FailureFirst(_)
                    ))
                    .count(),
                1
            );
            let text = render_text(&artifact);
            assert!(text.contains("require(") && text.contains("if ("), "{text}");
            assert_eq!(
                text.contains("evm.invalid();"),
                failure == "invalid",
                "{text}"
            );
            for first in [U256::ZERO, U256::from(1)] {
                for second in [U256::ZERO, U256::from(1)] {
                    let mut host = MemoryHost::new(&input, calldata(&[first, second]));
                    host.storage.insert(U256::ZERO, U256::from(0xab));
                    certify(&input, &artifact, host);
                }
            }
            assert_unchanged_witness(&artifact);
        }
    }
}

#[test]
fn transparent_chains_branching_dags_and_shared_revert_tails_are_certified() {
    for failure in [Failure::Chain, Failure::Diamond] {
        let input = direct(false, failure);
        let artifact = checked(&input);
        assert!(guards(&artifact) >= 1, "{}", render_text(&artifact));
        checker::certificate(lifted(&artifact).0);
        for value in [0_u64, 1, 3] {
            for inner in [0_u64, 1, 3] {
                let (raw, host) = certify(
                    &input,
                    &artifact,
                    MemoryHost::new(&input, calldata(&[U256::from(value), U256::from(inner)])),
                );
                assert_eq!(reverted(&raw, &host).is_some(), value == 0);
            }
        }
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn actual_nested_decoder_sequence_flattens_and_preserves_signed_wrapping_checks_live_phi_and_calls()
{
    let input = decoder(false);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = decompile(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        assert_eq!(
            artifact.completion,
            Completion::Complete,
            "{:?}",
            artifact.diagnostics
        );
        validated(&artifact);
        assert!(guards(&artifact) >= 4, "{}", render_text(&artifact));
        checker::certificate(lifted(&artifact).0);
        let text = render_text(&artifact);
        assert!(
            text.contains("require(") && !text.contains("} else {"),
            "nested certified checks must use flat requires: {text}"
        );
        assert!(
            text.contains("evm.slt("),
            "signed decoder comparison lost: {text}"
        );
        assert!(
            text.contains(" - 0x4)"),
            "calldatasize underflow was rewritten as integer arithmetic: {text}"
        );
        assert!(
            fixtures::has_following_if(&structured(&artifact).ir.structured.as_ref().unwrap().root),
            "fixture must exercise the real If-following-failure Sequence shape"
        );
        for size in [0_usize, 1, 3, 4, 35, 36, 67, 68, 99, 100, 132] {
            for boolean in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
                let data = decoder_data(size, boolean, U256::from(64), U256::ZERO);
                let host = decoder_host(&input, data);
                certify(&input, &artifact, host);
            }
        }
        for offset in [
            U256::ZERO,
            U256::from(1),
            U256::from(3),
            U256::from(4),
            U256::from(35),
            U256::from(36),
            U256::from(64),
            U256::MAX - U256::from(3),
            U256::MAX,
        ] {
            for len in [0_u64, 1, 3, 4, 35, 36] {
                let data = decoder_data(132, U256::from(1), offset, U256::from(len));
                certify(&input, &artifact, decoder_host(&input, data));
            }
        }
        let mut host = MemoryHost::new(
            &input,
            decoder_data(132, U256::from(1), U256::from(64), U256::from(3)),
        );
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: vec![0xaa],
            ..ExternalResponse::default()
        });
        let (raw, host) = certify(&input, &artifact, host);
        assert!(matches!(
            raw.outcome,
            ExecutionOutcome::Exit {
                kind: ExitKind::Stop,
                ..
            }
        ));
        assert_eq!(host.storage[&U256::ZERO], U256::from(3));
        assert_eq!(host.storage[&U256::from(1)], U256::from(68));
        assert_eq!(raw.trace.iter().filter(|op| op.opcode == 0xf1).count(), 1);
        assert_eq!(raw.trace.iter().filter(|op| op.opcode == 0x35).count(), 3);
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn pass_side_effect_before_shared_join_revert_remains_visible_and_executes() {
    let input = fixtures::join_revert();
    let artifact = checked(&input);
    assert!(guards(&artifact) >= 1, "{}", render_text(&artifact));
    checker::certificate(lifted(&artifact).0);
    let text = render_text(&artifact);
    let failure_if = text.find("require(").unwrap();
    let store = text.find("evm.sstore(").unwrap();
    let revert = text
        .rfind("evm.revert(")
        .expect("join REVERT still executes on the pass path");
    assert!(failure_if < store && store < revert, "{text}");
    for value in [0_u64, 1] {
        let (raw, host) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(value)])),
        );
        assert_eq!(reverted(&raw, &host), Some(Vec::new()));
        assert_eq!(host.storage.is_empty(), value == 0);
    }
    let all_fail = decoder(true);
    let artifact = checked(&all_fail);
    checker::certificate(lifted(&artifact).0);
    for size in [0, 4, 35, 36, 68, 99, 100, 132] {
        let (raw, host) = certify(
            &all_fail,
            &artifact,
            decoder_host(
                &all_fail,
                decoder_data(size, U256::from(1), U256::from(64), U256::from(3)),
            ),
        );
        assert_eq!(reverted(&raw, &host), Some(Vec::new()));
    }
}

#[test]
fn canonical_empty_error_string_is_distinct_from_empty_revert() {
    let input = direct(false, Failure::Error);
    let artifact = checked(&input);
    assert_eq!(guards(&artifact), 1);
    let (_, program) = lifted(&artifact);
    assert_eq!(program.payloads.len(), 1);
    assert!(program.payloads[0].message.is_empty());
    assert_eq!(program.payloads[0].bytes.len(), 68);
    let text = render_text(&artifact);
    assert!(
        text.contains(", \"\");") && text.contains("require("),
        "{text}"
    );
    for value in [0_u64, 1] {
        super::oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(value)])),
        );
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn nonempty_error_panic_forwarded_data_and_failure_observations_stay_conditional() {
    for failure in [
        Failure::Payload,
        Failure::Panic,
        Failure::Forwarded,
        Failure::Call,
        Failure::Sload,
        Failure::Mstore,
        Failure::WorldRead,
    ] {
        let input = direct(false, failure);
        let artifact = checked(&input);
        assert_eq!(
            guards(&artifact),
            0,
            "failure {failure:?}: {}",
            render_text(&artifact)
        );
        assert!(!render_text(&artifact).contains("require("));
        for value in [0_u64, 1] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(value)]));
            host.return_data = vec![0xde, 0xad, 0xbe, 0xef];
            host.storage.insert(U256::from(9), U256::from(0xab));
            host.context.insert(0x33, U256::from(0xcafe));
            host.responses.push_back(ExternalResponse {
                result: U256::ZERO,
                return_data: vec![0x44],
                ..ExternalResponse::default()
            });
            let (raw, host) = certify(&input, &artifact, host);
            if value == 0 {
                let bytes = reverted(&raw, &host).unwrap();
                match failure {
                    Failure::Payload => assert_eq!(bytes, vec![0x42]),
                    Failure::Panic => assert_eq!(&bytes[..4], &[0x4e, 0x48, 0x7b, 0x71]),
                    Failure::Error => assert_eq!(&bytes[..4], &[0x08, 0xc3, 0x79, 0xa0]),
                    Failure::Forwarded => assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]),
                    _ => assert!(bytes.is_empty()),
                }
            } else {
                assert_eq!(host.storage[&U256::ZERO], U256::from(0x55));
                assert_eq!(
                    raw.trace
                        .iter()
                        .filter(|op| matches!(op.opcode, 0xf1 | 0x54 | 0x52 | 0x33 | 0x3e))
                        .count(),
                    0,
                    "failure-local observation eagerly executed"
                );
            }
        }
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn stop_return_invalid_unknown_targets_and_faults_are_not_empty_revert_guards() {
    for failure in [
        Failure::Stop,
        Failure::Return,
        Failure::Invalid,
        Failure::Unknown,
        Failure::BadJump,
        Failure::Underflow,
        Failure::FaultingMemory,
    ] {
        let input = direct(false, failure);
        let artifact = decompile(&input, &Config::default());
        validated(&artifact);
        assert_eq!(
            artifact.current().unwrap().kind,
            StageKind::Lifted,
            "{:?}",
            artifact.diagnostics
        );
        assert_eq!(
            guards(&artifact),
            0,
            "{failure:?}: {}",
            render_text(&artifact)
        );
        checker::certificate(lifted(&artifact).0);
        assert!(!render_text(&artifact).contains("require("));
        let mut host = MemoryHost::new(&input, calldata(&[U256::ZERO]));
        let raw = evm_decompile::execute_bytecode(
            &input,
            &mut host,
            &evm_decompile::ExecutionLimits::default(),
        );
        match failure {
            Failure::Stop => assert!(matches!(
                raw.unwrap().outcome,
                ExecutionOutcome::Exit {
                    kind: ExitKind::Stop,
                    ..
                }
            )),
            Failure::Return => assert!(matches!(
                raw.unwrap().outcome,
                ExecutionOutcome::Exit {
                    kind: ExitKind::Return,
                    ..
                }
            )),
            Failure::FaultingMemory => assert!(matches!(
                raw,
                Err(evm_decompile::ExecutionError::ReturnDataBounds)
            )),
            _ => assert!(matches!(
                raw.unwrap().outcome,
                ExecutionOutcome::Exceptional { .. }
            )),
        }
    }
}

#[test]
fn reverting_loop_exits_and_unanchored_cycles_never_gain_finite_failure_ranks() {
    for exit in [false, true] {
        let input = fixtures::loop_failure(exit);
        let artifact = checked(&input);
        checker::certificate(lifted(&artifact).0);
        let stage = lifted(&artifact).0;
        let program = lifted(&artifact).1;
        let entry = stage.ir.entry.unwrap();
        let Terminator::Branch {
            else_edge: Some(edge),
            ..
        } = stage.ir.block(entry).unwrap().terminator
        else {
            panic!("outer conditional");
        };
        assert!(
            !program
                .failures
                .iter()
                .any(|fact| fact.block == stage.ir.edge(edge).unwrap().to)
        );
        assert!(
            !program.controls.iter().any(|control| control
                .guard
                .as_ref()
                .is_some_and(|guard| guard.block == entry)),
            "infinite failure-side path is not a must-terminate guard"
        );
        let loop_blocks: Vec<_> = stage
            .ir
            .blocks
            .iter()
            .filter(|block| {
                matches!(
                    block.terminator,
                    Terminator::Jump { .. } | Terminator::Branch { .. }
                )
            })
            .map(|block| block.id)
            .collect();
        assert!(loop_blocks.len() >= 2);
        let (raw, host) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(1)])),
        );
        assert!(matches!(
            raw.outcome,
            ExecutionOutcome::Exit {
                kind: ExitKind::Stop,
                ..
            }
        ));
        assert_eq!(host.storage[&U256::ZERO], U256::from(0x55));
        let initial = MemoryHost::new(&input, calldata(&[U256::ZERO]));
        let limits = evm_decompile::ExecutionLimits {
            max_steps: 64,
            ..evm_decompile::ExecutionLimits::default()
        };
        let raw = evm_decompile::execute_bytecode(&input, &mut initial.clone(), &limits).unwrap();
        assert_eq!(
            raw.outcome,
            ExecutionOutcome::StepLimit,
            "concrete infinite path does not terminate at its REVERT exit"
        );
        let graph = evm_decompile::execute_ir(stage, &mut initial.clone(), &limits).unwrap();
        let structured =
            evm_decompile::execute_structured(stage, &mut initial.clone(), &limits).unwrap();
        assert_eq!(graph.outcome, raw.outcome);
        assert_eq!(structured.outcome, raw.outcome);
        if exit {
            let (raw, host) = super::oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::ZERO, U256::from(1)])),
            );
            assert_eq!(reverted(&raw, &host), Some(Vec::new()));
            assert!(host.storage.is_empty());
        }
    }
}

#[test]
fn a_false_eof_branch_is_a_successful_halt_not_a_missing_failure_successor() {
    for outer in [false, true] {
        let input = fixtures::eof_branch(outer);
        let artifact = checked(&input);
        let (stage, program) = lifted(&artifact);
        assert_eq!(guards(&artifact), 0, "{}", render_text(&artifact));
        checker::certificate(stage);
        let eof = stage
            .ir
            .blocks
            .iter()
            .find(|block| {
                matches!(
                    block.terminator,
                    Terminator::Branch {
                        else_edge: None,
                        ..
                    }
                )
            })
            .unwrap();
        assert!(!program.failures.iter().any(|fact| fact.block == eof.id));
        for (value, inner) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
            let (raw, host) = super::oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(value), U256::from(inner)])),
            );
            let expected = if outer && value != 0 {
                ExitKind::Stop
            } else if (if outer { inner } else { value }) == 0 {
                ExitKind::EndOfCode
            } else {
                ExitKind::Revert
            };
            assert!(
                matches!(raw.outcome, ExecutionOutcome::Exit { kind, .. } if kind == expected),
                "{:?}",
                raw.outcome
            );
            assert_eq!(host.storage.contains_key(&U256::ZERO), outer && value == 1);
        }
        let mut invented = stage.clone();
        invented
            .ir
            .lifted
            .as_mut()
            .unwrap()
            .failures
            .push(evm_decompile::LiftedFailure {
                block: eof.id,
                rank: 1,
                payload: evm_decompile::LiftedGuardPayload::EmptyRevert,
            });
        invented
            .ir
            .lifted
            .as_mut()
            .unwrap()
            .failures
            .sort_by_key(|fact| fact.block);
        assert_eq!(validate_stage(&invented).status, ValidationStatus::Failed);
    }
}

#[test]
fn observations_before_failed_if_bind_once_and_keep_their_old_storage_value() {
    let input = fixtures::old_capture();
    let artifact = checked(&input);
    assert_eq!(guards(&artifact), 1);
    let text = render_text(&artifact);
    assert!(
        text.find("evm.sload(").unwrap() < text.find("require(").unwrap(),
        "{text}"
    );
    assert!(
        text.find("require(").unwrap() < text.find("evm.sstore(").unwrap(),
        "{text}"
    );
    for value in [0_u64, 1, 3] {
        let mut host = MemoryHost::new(&input, calldata(&[U256::from(value)]));
        host.storage.insert(U256::from(9), U256::from(0x11));
        let (raw, host) = certify(&input, &artifact, host);
        assert_eq!(raw.trace.iter().filter(|op| op.opcode == 0x54).count(), 1);
        assert_eq!(raw.trace.iter().filter(|op| op.opcode == 0x35).count(), 1);
        if value == 0 {
            assert_eq!(host.storage[&U256::from(9)], U256::from(0x11));
            assert_eq!(reverted(&raw, &host), Some(Vec::new()));
        } else {
            assert_eq!(host.storage[&U256::from(9)], U256::from(0x77));
            assert_eq!(host.storage[&U256::ZERO], U256::from(0x11));
        }
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn nested_guards_inherit_a_shared_outer_join_without_omitting_its_revert() {
    let input = fixtures::nested_join_revert();
    let artifact = checked(&input);
    assert_eq!(
        guards(&artifact),
        2,
        "both outer and inner guards must recover: {}",
        render_text(&artifact)
    );
    checker::certificate(lifted(&artifact).0);
    let text = render_text(&artifact);
    assert_eq!(text.matches("require(").count(), 2, "{text}");
    assert!(!text.contains("if ("), "{text}");
    assert!(
        text.find("evm.sstore(").unwrap() < text.rfind("evm.revert(").unwrap(),
        "{text}"
    );
    for a in [0_u64, 1] {
        for b in [0_u64, 1] {
            let (raw, host) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(a), U256::from(b)])),
            );
            assert_eq!(reverted(&raw, &host), Some(Vec::new()));
            assert_eq!(host.storage.contains_key(&U256::ZERO), a != 0 && b != 0);
            assert_eq!(
                raw.trace.iter().filter(|op| op.opcode == 0x35).count(),
                if a == 0 { 1 } else { 2 }
            );
        }
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn a_closed_empty_failure_can_guard_a_success_route_with_an_explicit_partial_jump() {
    let input = fixtures::partial_success();
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Partial);
    validated(&artifact);
    assert_eq!(guards(&artifact), 1, "{}", render_text(&artifact));
    let text = render_text(&artifact);
    assert!(
        text.contains("require(") && text.contains("partial"),
        "{text}"
    );
    let (stage, program) = lifted(&artifact);
    let guard = program
        .controls
        .iter()
        .find_map(|control| control.guard.as_ref())
        .unwrap();
    let unknown = stage.ir.edge(guard.pass_edge).unwrap().to;
    assert!(!program.failures.iter().any(|fact| fact.block == unknown));
    let (raw, host) = certify(
        &input,
        &artifact,
        MemoryHost::new(&input, calldata(&[U256::ZERO, U256::MAX])),
    );
    assert_eq!(reverted(&raw, &host), Some(Vec::new()));
    checker::partial_success(
        &input,
        &artifact,
        MemoryHost::new(&input, calldata(&[U256::from(1), U256::MAX])),
    );
    assert!(
        stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .any(|op| op.opcode == 0xfd)
    );
    assert_unchanged_witness(&artifact);
}

#[test]
fn guards_inside_loops_keep_live_pass_phi_and_simultaneous_backedge_swaps() {
    let input = fixtures::loop_phi();
    let artifact = checked(&input);
    assert_eq!(guards(&artifact), 1, "{}", render_text(&artifact));
    let (_, program) = lifted(&artifact);
    let guard = program
        .controls
        .iter()
        .find_map(|control| control.guard.as_ref())
        .unwrap();
    assert!(
        program.transfer(guard.pass_edge).unwrap().arguments.len() >= 3,
        "guard pass tuple must actually contain live values: {:?}; program: {:?}",
        program.transfer(guard.pass_edge),
        program.transfers
    );
    for value in [0_u64, 1] {
        let (raw, host) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(value)])),
        );
        if value == 0 {
            assert_eq!(reverted(&raw, &host), Some(Vec::new()));
            assert!(host.storage.is_empty());
        } else {
            assert_eq!(host.storage[&U256::ZERO], U256::from(1));
            assert_eq!(host.storage[&U256::from(1)], U256::from(2));
        }
        assert_eq!(
            raw.trace.iter().filter(|op| op.opcode == 0x35).count(),
            if value == 0 { 1 } else { 3 }
        );
    }
    let mut wrong = lifted(&artifact).0.clone();
    wrong
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .transfers
        .iter_mut()
        .find(|transfer| transfer.edge == guard.pass_edge)
        .unwrap()
        .arguments
        .swap(0, 1);
    assert_eq!(validate_stage(&wrong).status, ValidationStatus::Failed);
}

#[test]
fn a_stop_inside_an_unconditional_success_loop_keeps_suffix_projection_conservative() {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35).target("success").op(0x57);
    // An actual failed-path observation prevents the old require certificate.
    asm.push(9).op(0x54).op(0x50).op(0x5f).op(0x5f).op(0xfd);
    asm.dest("success")
        .push(32)
        .op(0x35)
        .dest("header")
        .op(0x80)
        .op(0x15)
        .target("done")
        .op(0x57)
        .push(1)
        .op(0x90)
        .op(0x03)
        .target("header")
        .op(0x56)
        .dest("done")
        .op(0x50)
        .push(0x55)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let root = &stage.ir.structured.as_ref().unwrap().root;
    let index = program.render_index(&stage.ir);
    let entry = stage.ir.entry.unwrap();
    let control = program
        .controls
        .iter()
        .find(|c| program.expression(c.condition).unwrap().block == entry)
        .unwrap();
    assert!(
        control.guard.is_none(),
        "the explicit source plan must accept nontransparent failure bodies"
    );
    assert!(
        index.failure_first(control.node, root.id).is_none(),
        "a STOP inside a loop does not close the loop's own normal fallthrough"
    );
    let source = index.node(control.node).unwrap();
    let StructuredKind::If { then_branch, .. } = &source.kind else {
        unreachable!()
    };
    let mut pending = vec![&**then_branch];
    let mut has_loop = false;
    while let Some(node) = pending.pop() {
        match &node.kind {
            StructuredKind::Sequence(v) => pending.extend(v),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => pending.extend([&**then_branch, &**else_branch]),
            StructuredKind::While { .. } => has_loop = true,
            _ => {}
        }
    }
    assert!(
        has_loop,
        "the nofallthrough premise must cover a real natural loop"
    );
    for pass in [0_u64, 1] {
        for count in [0_u64, 1, 3] {
            let mut host =
                MemoryHost::new(&input, calldata(&[U256::from(pass), U256::from(count)]));
            host.storage.insert(U256::from(9), U256::from(0x42));
            let (execution, observed) = certify(&input, &artifact, host);
            assert_eq!(
                execution
                    .trace
                    .iter()
                    .filter(|op| op.opcode == 0x54)
                    .count(),
                usize::from(pass == 0)
            );
            assert_eq!(observed.storage.contains_key(&U256::ZERO), pass != 0);
        }
    }
}
