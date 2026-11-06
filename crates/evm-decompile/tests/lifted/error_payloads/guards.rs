use super::*;

#[test]
fn both_guard_polarities_execute_failure_construction_only_on_rejection() {
    for failure_nonzero in [false, true] {
        for message in [DS_MATH, TRANSFER] {
            let input = fixtures::guarded(message, failure_nonzero, None);
            let artifact = checked(&input);
            assert_message(&artifact, message);
            assert_eq!(error_guards(&artifact), 1, "{}", render_text(&artifact));
            let text = render_text(&artifact);
            assert_eq!(text.matches("require(").count(), 1, "{text}");
            assert!(text.contains(std::str::from_utf8(message).unwrap()));
            for word in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
                let (outcome, host, capsules) = oracle::certify(
                    &input,
                    &artifact,
                    MemoryHost::new(&input, calldata(&[word])),
                    1_048_576,
                );
                let outcome = outcome.unwrap();
                if (word != U256::ZERO) == failure_nonzero {
                    assert_eq!(
                        oracle::reverted(&outcome, &host),
                        Some(oracle::abi(message))
                    );
                    assert!(host.storage.is_empty());
                    assert_eq!(capsules, 1);
                } else {
                    assert!(matches!(
                        outcome,
                        ExecutionOutcome::Exit {
                            kind: ExitKind::Stop,
                            ..
                        }
                    ));
                    assert_eq!(host.storage[&U256::ZERO], U256::from(0x55));
                    assert_eq!(
                        host.memory.len(),
                        96,
                        "message was eagerly constructed on success"
                    );
                    assert_eq!(capsules, 0);
                }
            }
        }
    }
}

#[test]
fn construction_memory_faults_and_success_memory_ceiling_are_preserved() {
    let input = fixtures::guarded(DS_MATH, false, None);
    let artifact = checked(&input);
    assert_eq!(error_guards(&artifact), 1);
    for memory_limit in [0, 64, 96, 128, 160, 192, 224, 256, 1_048_576] {
        for value in [0_u64, 1] {
            let (outcome, host, capsules) = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(value)])),
                memory_limit,
            );
            if memory_limit < 96 || (value == 0 && memory_limit < 256) {
                assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
            } else if value == 0 {
                assert_eq!(
                    oracle::reverted(&outcome.unwrap(), &host),
                    Some(oracle::abi(DS_MATH))
                );
                assert_eq!(capsules, 1);
            } else {
                assert!(outcome.is_ok());
                assert_eq!(capsules, 0);
                assert_eq!(host.memory.len(), 96);
            }
        }
    }
}

#[test]
fn differing_errors_and_failure_observations_are_not_merged_into_one_fixed_message() {
    let input = fixtures::two_errors();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    assert_message(&artifact, TRANSFER);
    let entry = lifted(&artifact).0.ir.entry.unwrap();
    assert!(
        lifted(&artifact).1.controls.iter().all(|control| control
            .guard
            .as_ref()
            .is_none_or(|guard| guard.block != entry)),
        "outer branch has differing failed payloads"
    );
    for (pass, second) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
        let (outcome, host, _) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(pass), U256::from(second)])),
            1_048_576,
        );
        if pass == 0 {
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &host),
                Some(oracle::abi(if second == 0 { DS_MATH } else { TRANSFER }))
            );
        }
    }
    for opcode in [0x55, 0xf1, 0x54, 0x33] {
        let input = fixtures::guarded(DS_MATH, false, Some(opcode));
        let artifact = checked(&input);
        assert_eq!(error_guards(&artifact), 0, "absorbed opcode {opcode:#x}");
        for pass in [0_u64, 1] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(pass)]));
            host.responses.push_back(ExternalResponse::default());
            let _ = oracle::certify(&input, &artifact, host, 1_048_576);
        }
    }
}

#[test]
fn shared_and_nested_pass_reachable_tails_preserve_storage_before_error() {
    for nested in [false, true] {
        let input = fixtures::shared_join(nested);
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        let text = render_text(&artifact);
        let store = text.find("sstore(").unwrap();
        let tail = text.rfind("revert(\"ds-math-mul-overflow\");").unwrap();
        assert!(store < tail, "pass-reachable tail was removed: {text}");
        for (outer, inner) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
            let (outcome, host, _) = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(outer), U256::from(inner)])),
                1_048_576,
            );
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &host),
                Some(oracle::abi(DS_MATH))
            );
            assert_eq!(
                host.storage.contains_key(&U256::ZERO),
                outer != 0 && (!nested || inner != 0)
            );
        }
    }
}

#[test]
fn equal_error_bytes_share_identity_but_replay_their_own_constructor_faults() {
    let input = fixtures::same_error_different_constructors();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    assert_eq!(lifted(&artifact).1.payloads.len(), 1);
    assert_eq!(lifted(&artifact).1.reverts.len(), 2);
    let (stage, program) = lifted(&artifact);
    assert!(
        program.controls.iter().any(|c| c
            .guard
            .as_ref()
            .is_some_and(|g| g.block == stage.ir.entry.unwrap()
                && matches!(g.payload, LiftedGuardPayload::ErrorString(_)))),
        "same payload identity must permit the outer guard while preserving its actual lazy route"
    );
    for (pass, large) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
        for memory_limit in [96, 256, 4192, 4224] {
            let (outcome, host, _) = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(pass), U256::from(large)])),
                memory_limit,
            );
            if pass != 0 {
                assert_eq!(host.memory.len(), 96);
                assert!(outcome.is_ok());
            } else if large == 0 && memory_limit >= 256 || large != 0 && memory_limit >= 4224 {
                assert_eq!(
                    oracle::reverted(&outcome.unwrap(), &host),
                    Some(oracle::abi(DS_MATH))
                );
            } else {
                assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
            }
        }
    }
}

#[test]
fn a_locally_closed_error_proof_survives_an_unresolved_success_route() {
    let input = fixtures::partial_pass();
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, evm_decompile::Completion::Partial);
    assert_message(&artifact, DS_MATH);
    assert_eq!(error_guards(&artifact), 1);
    let (outcome, host, _) = oracle::certify(
        &input,
        &artifact,
        MemoryHost::new(&input, calldata(&[U256::ZERO])),
        1_048_576,
    );
    assert_eq!(
        oracle::reverted(&outcome.unwrap(), &host),
        Some(oracle::abi(DS_MATH))
    );
}

#[test]
fn unrelated_memory_reads_and_writes_are_not_absorbed_into_failure_construction() {
    for input in [
        fixtures::guarded(DS_MATH, false, Some(0x51)),
        fixtures::guarded(DS_MATH, false, Some(0x52)),
        fixtures::interleaved_unrelated_memory(true),
        fixtures::interleaved_unrelated_memory(false),
    ] {
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        assert_eq!(
            error_guards(&artifact),
            0,
            "unrelated memory observation became a require: {}",
            render_text(&artifact)
        );
        let (stage, program) = lifted(&artifact);
        let index = program.render_index(&stage.ir);
        for block in &stage.ir.blocks {
            for operation in &block.operations {
                let unrelated = matches!(operation.opcode, 0x51 | 0x52)
                    && operation.inputs.first().is_some_and(|id| {
                        stage
                            .ir
                            .value(*id)
                            .unwrap()
                            .fact
                            .singleton()
                            .is_some_and(|word| {
                                word == U256::from(512)
                                    || word == U256::from(0x300)
                                    || word == U256::MAX
                            })
                    });
                if !unrelated {
                    continue;
                }
                if let Some(capsule) = index.construction(block.id) {
                    let body = program.block(block.id).unwrap();
                    let position = body
                        .statements
                        .iter()
                        .position(|s| s.operation == operation.id)
                        .unwrap();
                    assert!(
                        position < capsule.start || position >= capsule.end,
                        "capsule hid unrelated memory operation {:?}",
                        operation.id
                    );
                }
            }
        }
        for pass in [0_u64, 1] {
            let _ = oracle::certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[U256::from(pass)])),
                1_048_576,
            );
        }
    }
}

#[test]
fn nested_guard_can_share_one_constructor_reached_by_two_original_jumps() {
    let input = fixtures::shared_nested_constructor();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    let (stage, program) = lifted(&artifact);
    assert_eq!(program.reverts.len(), 1);
    assert_eq!(
        program.constructions.len(),
        1,
        "a shared constructor remains one reusable source range"
    );
    assert!(error_guards(&artifact) >= 1, "{}", render_text(&artifact));
    program
        .render_index(&stage.ir)
        .validate_guard_sources()
        .unwrap();
    for (outer, inner) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
        let (outcome, host, capsules) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(outer), U256::from(inner)])),
            1_048_576,
        );
        if outer == 0 {
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &host),
                Some(oracle::abi(DS_MATH))
            );
            assert_eq!(capsules, 1);
        } else {
            assert_eq!(capsules, 0);
            assert_eq!(host.storage[&U256::ZERO], U256::from(0x55));
        }
    }
}

#[test]
fn dispatcher_terminal_cases_reuse_certified_construction_without_erasing_state_transitions() {
    let input = fixtures::dispatcher();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    let (stage, program) = lifted(&artifact);
    let source = serde_json::to_value(stage.ir.structured.as_ref().unwrap()).unwrap();
    assert!(
        source["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"].get("dispatcher").is_some())
    );
    let terminal = program.reverts[0].block;
    let index = program.render_index(&stage.ir);
    assert!(
        index.construction(terminal).is_some(),
        "valid dispatcher terminal case needs a source construction plan"
    );
    assert!(render_text(&artifact).contains("revert(\"ds-math-mul-overflow\");"));
    for words in [[0_u64, 0_u64, 0_u64], [1, 0, 0], [1, 1, 0], [0, 0, 1]] {
        let words = words.map(U256::from);
        let (outcome, host, capsules) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&words)),
            1_048_576,
        );
        assert_eq!(
            oracle::reverted(&outcome.unwrap(), &host),
            Some(oracle::abi(DS_MATH))
        );
        assert_eq!(capsules, 1);
    }
}

#[test]
fn empty_error_string_empty_revert_and_bubbled_bytes_never_merge_with_a_fixed_error() {
    use fixtures::MixedFailure;
    for other in [
        MixedFailure::Empty,
        MixedFailure::EmptyError,
        MixedFailure::Forwarded,
    ] {
        let input = fixtures::mixed_errors(other);
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        let (stage, program) = lifted(&artifact);
        assert!(
            program.controls.iter().all(|c| c
                .guard
                .as_ref()
                .is_none_or(|g| g.block != stage.ir.entry.unwrap())),
            "different failure identities became an outer fixed guard: {other:?}"
        );
        for (pass, second) in [(0_u64, 0_u64), (0, 1), (1, 0), (1, 1)] {
            let mut host =
                MemoryHost::new(&input, calldata(&[U256::from(pass), U256::from(second)]));
            host.return_data = vec![0xde, 0xad, 0xbe, 0xef];
            let (outcome, host, _) = oracle::certify(&input, &artifact, host, 1_048_576);
            if pass == 0 {
                let expected = if second == 0 {
                    oracle::abi(DS_MATH)
                } else {
                    match other {
                        MixedFailure::Empty => Vec::new(),
                        MixedFailure::EmptyError => oracle::abi(b""),
                        MixedFailure::Forwarded => vec![0xde, 0xad, 0xbe, 0xef],
                    }
                };
                assert_eq!(oracle::reverted(&outcome.unwrap(), &host), Some(expected));
            }
        }
    }
}

#[test]
fn error_guards_inside_loops_preserve_live_parallel_pass_phis() {
    let input = fixtures::loop_guard();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    assert!(error_guards(&artifact) > 0);
    for word in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
        let (outcome, host, capsules) = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[word])),
            1_048_576,
        );
        if word == U256::ZERO {
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &host),
                Some(oracle::abi(DS_MATH))
            );
            assert_eq!(capsules, 1);
        } else {
            assert!(matches!(
                outcome.unwrap(),
                ExecutionOutcome::Exit {
                    kind: ExitKind::Stop,
                    ..
                }
            ));
            assert_eq!(host.storage.len(), 2);
            assert_eq!(capsules, 0);
            assert_eq!(host.memory.len(), 96);
        }
    }
}

#[test]
fn a_pass_reachable_error_join_remains_after_the_actual_external_call() {
    let input = fixtures::shared_call_join();
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    assert!(error_guards(&artifact) > 0);
    let text = render_text(&artifact);
    let require = text.find("require(").unwrap();
    let call = text.find("call(").unwrap();
    let tail = text.rfind("revert(\"ds-math-mul-overflow\");").unwrap();
    assert!(require < call && call < tail, "{text}");
    for pass in [0_u64, 1] {
        let mut host = MemoryHost::new(&input, calldata(&[U256::from(pass)]));
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: vec![0x42; 32],
            ..ExternalResponse::default()
        });
        let (outcome, host, _) = oracle::certify(&input, &artifact, host, 1_048_576);
        assert_eq!(
            oracle::reverted(&outcome.unwrap(), &host),
            Some(oracle::abi(DS_MATH))
        );
        assert_eq!(host.responses.len(), usize::from(pass == 0));
    }
}
