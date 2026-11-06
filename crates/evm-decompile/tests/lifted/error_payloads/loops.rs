use super::*;
use evm_decompile::{
    Cancellation, Completion, ExecutionLimits, Input, Phase, decompile_with_progress,
    execute_bytecode,
};

#[path = "loops/fixtures.rs"]
mod fixtures;
use fixtures::{Bound, Change, SPECS, Spec, candidate_pair, counted};

fn host(input: &Input, words: &[U256], calls: u64) -> MemoryHost {
    let mut host = MemoryHost::new(input, calldata(words));
    for _ in 0..calls {
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: vec![0xee; 96],
            ..ExternalResponse::default()
        });
    }
    host
}

fn assert_counted_certificate(spec: Spec) {
    let input = counted(spec, Change::None);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    assert_eq!(lifted(&artifact).1.constructions.len(), 1, "{spec:?}");
    for n in [0, 1, 2, spec.limit] {
        let (outcome, observed, capsules) = oracle::certify(
            &input,
            &artifact,
            host(
                &input,
                &[U256::from(n), U256::ZERO, U256::ZERO],
                spec.iterations(n),
            ),
            1_048_576,
        );
        let outcome = outcome.unwrap();
        assert_eq!(
            oracle::reverted(&outcome, &observed),
            Some(oracle::abi(DS_MATH))
        );
        let ExecutionOutcome::Exit { values, .. } = outcome else {
            unreachable!()
        };
        assert_eq!(values, vec![U256::from(spec.pointer(n)), U256::from(100)]);
        assert_eq!(
            capsules, 1,
            "current-iteration p and q must own one exact constructor"
        );
        assert!(observed.responses.is_empty());
    }
}

#[test]
fn dynamic_counted_loop_certifies_coefficient13_growth37_and_cell64() {
    assert_counted_certificate(SPECS[0]);
}

#[test]
fn dynamic_counted_loop_certifies_false_or_bound_counter_step2_and_cell96() {
    assert_counted_certificate(SPECS[1]);
}

#[test]
fn dynamic_counted_loop_certifies_true_and_bound_counter_step3_and_cell256() {
    assert_counted_certificate(SPECS[2]);
}

#[test]
fn dynamic_counted_loop_certifies_modular_minus1_counter_and_cell0() {
    assert_counted_certificate(SPECS[3]);
}

#[test]
fn dynamic_loop_bytes_and_fault_prefixes_match_without_reusing_producer_proofs() {
    for spec in SPECS {
        let input = counted(spec, Change::None);
        let artifact = checked(&input);
        for n in [0, 1, 2, spec.limit] {
            for memory_limit in [31, usize::try_from(spec.base + 16).unwrap(), 1_048_576] {
                let (outcome, observed, _) = oracle::certify(
                    &input,
                    &artifact,
                    host(&input, &[U256::from(n)], spec.iterations(n)),
                    memory_limit,
                );
                if memory_limit == 1_048_576 {
                    assert_eq!(
                        oracle::reverted(&outcome.unwrap(), &observed),
                        Some(oracle::abi(DS_MATH))
                    );
                } else {
                    assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
                }
            }
        }
    }
}

#[test]
fn dynamic_loop_certificates_roundtrip_with_current_schema_and_source_navigation() {
    for spec in SPECS {
        let input = counted(spec, Change::None);
        let artifact = checked(&input);
        assert_message(&artifact, DS_MATH);
        let (stage, program) = lifted(&artifact);
        let bytes = serde_json::to_vec(program).unwrap();
        let decoded: evm_decompile::LiftedProgram = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&decoded, program);
        let index = decoded.render_index(&stage.ir);
        index.validate_guard_sources().unwrap();
        for proof in &decoded.reverts {
            let capsule = index
                .construction(proof.block)
                .expect("new loop proof must retain a navigable original constructor");
            assert_eq!(capsule.revert, proof.id);
            assert_eq!(
                index.render_revert(proof.block, |_| true).unwrap(),
                Some("revert(\"ds-math-mul-overflow\");".into())
            );
            for id in [proof.id, capsule.id, proof.payload] {
                let origin = artifact
                    .origin(evm_decompile::EntityRef {
                        stage: stage.id,
                        entity: id,
                    })
                    .unwrap();
                assert!(!origin.parents.is_empty());
                assert!(!origin.source.is_empty());
            }
        }
    }
}

#[test]
fn missing_or_disjunctive_length_bounds_do_not_create_counted_loop_certificates() {
    for bound in [Bound::Missing, Bound::TrueOr] {
        let spec = Spec { bound, ..SPECS[0] };
        let input = counted(spec, Change::None);
        let artifact = checked(&input);
        assert!(
            lifted(&artifact).1.reverts.is_empty(),
            "unproved ordinary pointer bounds: {bound:?}"
        );
        for n in [0, 1, 2] {
            let _ = oracle::certify(
                &input,
                &artifact,
                host(&input, &[U256::from(n)], spec.iterations(n)),
                1_048_576,
            );
        }
    }
}

#[test]
fn wrap_alias_entry_call_and_counter_negatives_cannot_reuse_a_safe_loop_summary() {
    for change in [
        Change::InitialWrap,
        Change::GrowthWrap,
        Change::SlotAlias,
        Change::HeaderOutput,
        Change::ArbitraryOutput,
        Change::BranchGrowth,
        Change::EntryBypass,
        Change::SideEntry,
        Change::ExtraBackedge,
        Change::NoProgress,
        Change::UpdatedRead,
        Change::StalePointer,
        Change::PreviousIterationPointer,
        Change::NestedClobber,
        Change::FeasibleClobber,
    ] {
        let spec = SPECS[0];
        let input = counted(spec, change);
        let artifact = checked(&input);
        assert!(
            lifted(&artifact).1.reverts.is_empty(),
            "admitted unsafe source variant {change:?}"
        );
        assert!(lifted(&artifact).1.constructions.is_empty(), "{change:?}");
        let mut execution_host = host(
            &input,
            &[U256::from(1), U256::from(spec.slot), U256::ZERO],
            1,
        );
        if change == Change::NoProgress {
            execution_host.responses.front_mut().unwrap().result = U256::ZERO;
        }
        if change == Change::HeaderOutput {
            execution_host
                .responses
                .front_mut()
                .unwrap()
                .return_data
                .fill(0);
        }
        if matches!(change, Change::EntryBypass | Change::SideEntry) {
            execution_host.calldata = calldata(&[U256::from(1), U256::ZERO, U256::from(1)]);
        }
        let _ = oracle::certify(&input, &artifact, execution_host, 1_048_576);
    }
}

#[test]
fn negative_fixture_pointer_changes_have_independent_concrete_witnesses() {
    // These two variants keep valid memory addresses, making the p/q mismatch
    // observable as bytes rather than depending solely on a memory-limit fault.
    for change in [
        Change::UpdatedRead,
        Change::StalePointer,
        Change::PreviousIterationPointer,
    ] {
        let spec = SPECS[0];
        let input = counted(spec, change);
        let mut concrete = host(&input, &[U256::from(1)], 1);
        let raw = execute_bytecode(&input, &mut concrete, &ExecutionLimits::default()).unwrap();
        assert_ne!(
            oracle::reverted(&raw.outcome, &concrete),
            Some(oracle::abi(DS_MATH)),
            "{change:?}"
        );
    }
}

#[test]
fn same_source_load_id_across_epochs_keeps_the_previous_iteration_capture_distinct() {
    let spec = SPECS[0];
    let input = counted(spec, Change::PreviousIterationPointer);
    let artifact = checked(&input);
    assert!(lifted(&artifact).1.reverts.is_empty());
    let mut concrete = host(&input, &[U256::from(2)], 2);
    let raw = execute_bytecode(&input, &mut concrete, &ExecutionLimits::default()).unwrap();
    let pointer_reads: Vec<_> = raw
        .trace
        .iter()
        .filter(|op| op.opcode == 0x51 && op.inputs == [U256::from(spec.slot)])
        .collect();
    assert!(
        pointer_reads.iter().any(|a| pointer_reads
            .iter()
            .any(|b| a.pc == b.pc && a.outputs != b.outputs)),
        "the negative fixture must reuse one source load with different dynamic results"
    );
    assert_ne!(
        oracle::reverted(&raw.outcome, &concrete),
        Some(oracle::abi(DS_MATH))
    );
    let _ = oracle::certify(
        &input,
        &artifact,
        host(&input, &[U256::from(2)], 2),
        1_048_576,
    );
}

#[test]
fn constructor_certificates_preserve_pointer_phase_before_and_after_iteration_updates() {
    let spec = SPECS[0];
    let input = counted(spec, Change::EarlyError);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    assert_eq!(lifted(&artifact).1.reverts.len(), 2);
    assert_eq!(lifted(&artifact).1.constructions.len(), 2);
    for early in [false, true] {
        let (outcome, observed, capsules) = oracle::certify(
            &input,
            &artifact,
            host(
                &input,
                &[U256::from(2), U256::from(u64::from(early))],
                if early { 0 } else { 2 },
            ),
            1_048_576,
        );
        let outcome = outcome.unwrap();
        assert_eq!(
            oracle::reverted(&outcome, &observed),
            Some(oracle::abi(DS_MATH))
        );
        let ExecutionOutcome::Exit { values, .. } = outcome else {
            unreachable!()
        };
        let expected = if early {
            spec.base + 2 * spec.coefficient
        } else {
            spec.pointer(2)
        };
        assert_eq!(values[0], U256::from(expected));
        assert_eq!(capsules, 1);
    }
}

#[test]
fn failed_candidate_caches_do_not_certify_its_sibling_or_erase_a_verified_sibling() {
    for unsafe_first in [true, false] {
        let input = candidate_pair(unsafe_first);
        let artifact = checked(&input);
        let (_, program) = lifted(&artifact);
        assert_eq!(
            program.reverts.len(),
            1,
            "only the loop preserving its header may certify; unsafe_first={unsafe_first}"
        );
        assert_eq!(program.constructions.len(), 1);
        for second in [false, true] {
            let unsafe_route = unsafe_first != second;
            let mut execution_host = host(
                &input,
                &[
                    U256::from(2),
                    U256::ZERO,
                    U256::ZERO,
                    U256::from(u64::from(second)),
                ],
                if unsafe_route { 1 } else { 2 },
            );
            if unsafe_route {
                execution_host
                    .responses
                    .front_mut()
                    .unwrap()
                    .return_data
                    .fill(0);
            }
            let (outcome, observed, capsules) =
                oracle::certify(&input, &artifact, execution_host, 1_048_576);
            assert_eq!(
                oracle::reverted(&outcome.unwrap(), &observed),
                Some(oracle::abi(DS_MATH))
            );
            assert_eq!(capsules, usize::from(!unsafe_route));
        }
    }
}

#[test]
fn rejected_bound_arms_stay_outside_the_dynamic_allocation_and_constructor() {
    for spec in SPECS {
        let input = counted(spec, Change::None);
        let artifact = checked(&input);
        for words in [
            [U256::from(spec.limit + 1), U256::ZERO, U256::ZERO],
            [U256::from(2), U256::ZERO, U256::from(1)],
        ] {
            if matches!(spec.bound, Bound::Upper) && words[2] != U256::ZERO {
                continue;
            }
            let (outcome, observed, capsules) =
                oracle::certify(&input, &artifact, host(&input, &words, 0), 1_048_576);
            assert!(matches!(
                outcome,
                Ok(ExecutionOutcome::Exit {
                    kind: ExitKind::Stop,
                    ..
                })
            ));
            assert!(observed.memory.is_empty());
            assert_eq!(capsules, 0);
        }
    }
}

#[test]
fn dynamic_relation_limits_and_mid_lift_cancellation_publish_no_tentative_proof() {
    let input = counted(SPECS[0], Change::None);
    let before_cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &before_cancel, |progress| {
        if progress.phase == Phase::Lift {
            before_cancel.cancel();
        }
    });
    assert_eq!(before.current().unwrap().kind, StageKind::Structured);
    let mut lift_progress = Vec::new();
    let complete = decompile_with_progress(
        &input,
        &Config::default(),
        &Cancellation::new(),
        |progress| {
            if progress.phase == Phase::Lift {
                lift_progress.push(progress.work);
            }
        },
    );
    assert_eq!(complete.completion, Completion::Complete);
    assert_message(&complete, DS_MATH);
    let first = *lift_progress.first().unwrap();
    let last = *lift_progress.last().unwrap();
    assert!(
        last > first,
        "source relation work must expose cooperative progress"
    );
    let midpoint = first + (last - first) / 2;
    let cancelled = Cancellation::new();
    let result = decompile_with_progress(&input, &Config::default(), &cancelled, |progress| {
        if progress.phase == Phase::Lift && progress.work >= midpoint {
            cancelled.cancel();
        }
    });
    assert_eq!(result.completion, Completion::Cancelled);
    assert_eq!(result.stages, before.stages);
    assert_eq!(result.events, before.events);
    assert_eq!(result.mappings, before.mappings);
    for max_work in [before.statistics.work + 1, complete.statistics.work - 1] {
        let mut config = Config::default();
        config.limits.max_work = max_work;
        let limited = decompile(&input, &config);
        assert_eq!(limited.completion, Completion::Limited);
        assert!(limited.statistics.work <= max_work);
        assert_eq!(limited.stages, before.stages);
        assert_eq!(limited.events, before.events);
        assert_eq!(limited.mappings, before.mappings);
    }
    let mut config = Config::default();
    config.limits.max_history_entities = complete.statistics.history_entities - 1;
    let limited = decompile(&input, &config);
    assert_eq!(limited.completion, Completion::Limited);
    assert_eq!(limited.stages, before.stages);
    assert!(limited.statistics.history_entities <= config.limits.max_history_entities);
    let fresh = checked(&input);
    assert_eq!(
        fresh, complete,
        "failed transactions must not poison the next analysis"
    );
}
