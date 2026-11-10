//! Region proofs must preserve concrete effects and simultaneous edge transfers.
//! The bytecode, graph and syntax executors have independent control machinery.
use evm_decompile::{
    Artifact, Completion, Config, Execution, ExecutionLimits, ExecutionOutcome, ExitKind, Input,
    MemoryHost, ProcedurePreference, StageKind, StructuredKind, StructuredNode, U256,
    ValidationStatus, decompile, execute_bytecode, execute_ir, execute_structured,
};

#[path = "structure_regions/fixtures.rs"]
mod fixtures;
use fixtures::{blocked_loop, loop_diamond, nested_loops};

fn checked(input: &Input, completion: Completion) -> Artifact {
    let mut config = Config::default();
    config.readability.procedures = ProcedurePreference::Inline;
    let artifact = decompile(input, &config);
    assert_eq!(
        artifact.completion, completion,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| { stage.validation.status == ValidationStatus::Passed })
    );
    assert!(
        artifact.diagnostics.iter().all(|diagnostic| {
            !diagnostic.message.contains("whole-source dispatcher")
                && !diagnostic.message.contains("source-control verification")
        }),
        "unexpected structuring fallback: {:?}",
        artifact.diagnostics
    );
    artifact
}

fn host(input: &Input, words: &[u64]) -> MemoryHost {
    MemoryHost::new(
        input,
        words
            .iter()
            .flat_map(|word| U256::from(*word).to_be_bytes::<32>())
            .collect(),
    )
}

fn equivalent(
    input: &Input,
    artifact: &Artifact,
    words: &[u64],
    limits: &ExecutionLimits,
) -> (Execution, MemoryHost) {
    let initial = host(input, words);
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|s| s.kind != StageKind::Decoded)
    {
        let mut graph_host = initial.clone();
        let graph = execute_ir(stage, &mut graph_host, limits).unwrap();
        assert_eq!(
            graph.outcome, raw.outcome,
            "CFG {:?}, {words:?}",
            stage.kind
        );
        assert_eq!(
            graph.observations(),
            raw.observations(),
            "CFG {:?}, {words:?}",
            stage.kind
        );
        assert_eq!(graph_host, raw_host, "CFG {:?}, {words:?}", stage.kind);
        if stage.ir.structured.is_some() {
            let mut tree_host = initial.clone();
            let tree = execute_structured(stage, &mut tree_host, limits).unwrap();
            assert_eq!(tree.outcome, raw.outcome, "AST {:?}, {words:?}", stage.kind);
            assert_eq!(
                tree.observations(),
                raw.observations(),
                "AST {:?}, {words:?}",
                stage.kind
            );
            assert_eq!(tree_host, raw_host, "AST {:?}, {words:?}", stage.kind);
        }
    }
    (raw, raw_host)
}

fn bodies(artifact: &Artifact, source_pc: usize) -> (usize, usize) {
    let ir = &artifact.current().unwrap().ir;
    let mut occurrences = 0;
    let mut dispatchers = 0;
    let mut todo = vec![&ir.structured.as_ref().unwrap().root];
    while let Some(StructuredNode { kind, .. }) = todo.pop() {
        match kind {
            StructuredKind::Block { block } => {
                occurrences +=
                    usize::from(ir.block(*block).unwrap().source.unwrap().pc == source_pc);
            }
            StructuredKind::Sequence(children) => todo.extend(children),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                todo.extend([&**then_branch, &**else_branch]);
            }
            StructuredKind::While { header, body, .. } => todo.extend([&**header, &**body]),
            StructuredKind::Indirect { arms, .. } => todo.extend(arms.iter().map(|arm| &arm.body)),
            StructuredKind::Dispatcher { cases, .. } => {
                dispatchers += 1;
                occurrences += cases
                    .iter()
                    .filter(|case| ir.block(case.block).unwrap().source.unwrap().pc == source_pc)
                    .count();
            }
            _ => {}
        }
    }
    (occurrences, dispatchers)
}

#[test]
fn loop_failure_diamond_shares_effectful_suffix_and_preserves_parallel_values() {
    for terminal in [0xfd, 0xfe] {
        let fixture = loop_diamond(terminal);
        let artifact = checked(&fixture.input, Completion::Complete);
        assert_eq!(bodies(&artifact, fixture.join), (1, 0));
        for iterations in [0, 1, 3] {
            for left in [0, 1] {
                let (run, state) = equivalent(
                    &fixture.input,
                    &artifact,
                    &[iterations, left, 0],
                    &ExecutionLimits::default(),
                );
                assert!(matches!(
                    run.outcome,
                    ExecutionOutcome::Exit {
                        kind: ExitKind::Stop,
                        ..
                    }
                ));
                assert_eq!(state.logs.len(), iterations as usize);
                assert_eq!(state.storage[&U256::from(2)], U256::from(1));
                if iterations == 0 {
                    assert_eq!(state.storage.len(), 1);
                    continue;
                }
                let swapped = left != 0 && iterations % 2 != 0;
                let (first, second) = if swapped {
                    (0x5678, 0x1234)
                } else {
                    (0x1234, 0x5678)
                };
                assert_eq!(state.storage[&U256::ZERO], U256::from(first));
                assert_eq!(state.storage[&U256::from(1)], U256::from(second));
                for (index, (_, _, bytes)) in state.logs.iter().enumerate() {
                    let swapped = left != 0 && index % 2 == 0;
                    let values = if swapped {
                        [0x5678, 0x1234]
                    } else {
                        [0x1234, 0x5678]
                    };
                    let expected: Vec<_> = values
                        .into_iter()
                        .flat_map(|v| U256::from(v).to_be_bytes::<32>())
                        .collect();
                    assert_eq!(*bytes, expected);
                }
            }
        }
        for remaining in [1, 2, 3] {
            let (run, state) = equivalent(
                &fixture.input,
                &artifact,
                &[3, 1, remaining],
                &ExecutionLimits::default(),
            );
            assert_eq!(state.logs.len(), (3 - remaining) as usize);
            assert!(!state.storage.contains_key(&U256::from(2)));
            assert_eq!(U256::from_be_slice(&state.memory[..32]), U256::from(0x0bad));
            if terminal == 0xfd {
                assert!(matches!(
                    run.outcome,
                    ExecutionOutcome::Exit {
                        kind: ExitKind::Revert,
                        ..
                    }
                ));
            } else {
                assert!(matches!(run.outcome, ExecutionOutcome::Exceptional { .. }));
            }
        }
    }
}

#[test]
fn successful_stop_and_return_are_preserved_inside_the_loop() {
    for (terminal, kind) in [(0x00, ExitKind::Stop), (0xf3, ExitKind::Return)] {
        let fixture = loop_diamond(terminal);
        let artifact = checked(&fixture.input, Completion::Complete);
        for remaining in [1, 3] {
            let (run, state) = equivalent(
                &fixture.input,
                &artifact,
                &[3, 1, remaining],
                &ExecutionLimits::default(),
            );
            assert!(
                matches!(run.outcome, ExecutionOutcome::Exit { kind: actual, .. } if actual == kind)
            );
            assert_eq!(state.logs.len(), (3 - remaining) as usize);
            assert!(!state.storage.contains_key(&U256::from(2)));
        }
    }
}

#[test]
fn unresolved_inner_exit_preserves_the_executed_prefix_and_partial_boundary() {
    let mut fixture = loop_diamond(0xfd);
    let len = fixture.input.runtime_bytecode.len();
    fixture.input.runtime_bytecode[len - 4..].copy_from_slice(&[0x60, 0x60, 0x35, 0x56]);
    let artifact = checked(&fixture.input, Completion::Partial);
    let stage = artifact.current().unwrap();
    assert!(
        stage
            .ir
            .blocks
            .iter()
            .any(|block| block.terminator.is_partial())
    );
    let initial = host(&fixture.input, &[3, 1, 2, 0xffff]);
    let limits = ExecutionLimits::default();
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(&fixture.input, &mut raw_host, &limits).unwrap();
    assert!(matches!(raw.outcome, ExecutionOutcome::Exceptional { .. }));
    let mut graph_host = initial.clone();
    let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut tree_host = initial;
    let tree = execute_structured(stage, &mut tree_host, &limits).unwrap();
    assert!(matches!(
        graph.outcome,
        ExecutionOutcome::PartialJump { .. }
    ));
    assert_eq!(tree.outcome, graph.outcome);
    assert_eq!(raw.observations(), graph.observations());
    assert_eq!(tree.observations(), graph.observations());
    assert_eq!(tree_host, graph_host);
    assert_eq!(raw_host, graph_host);
    assert_eq!(graph_host.logs.len(), 1);
    assert!(!graph_host.storage.contains_key(&U256::from(2)));
}

#[test]
fn irreducible_inner_cycle_is_not_discarded_as_a_failed_exit() {
    let fixture = blocked_loop();
    let artifact = checked(&fixture.input, Completion::Complete);
    assert!(bodies(&artifact, fixture.join).1 > 0);
    for entry in [0, 1] {
        for iterations in [0, 1, 3] {
            let (run, state) = equivalent(
                &fixture.input,
                &artifact,
                &[iterations, entry, 0, 0],
                &ExecutionLimits::default(),
            );
            assert!(matches!(
                run.outcome,
                ExecutionOutcome::Exit {
                    kind: ExitKind::Stop,
                    ..
                }
            ));
            assert_eq!(state.logs.len(), iterations as usize);
        }
        let (run, state) = equivalent(
            &fixture.input,
            &artifact,
            &[1, entry, 1, 1],
            &ExecutionLimits {
                max_steps: 300,
                ..ExecutionLimits::default()
            },
        );
        assert_eq!(run.outcome, ExecutionOutcome::StepLimit);
        assert!(state.logs.is_empty());
        assert!(!state.storage.contains_key(&U256::ZERO));
    }
}

#[test]
fn nested_iteration_boundaries_keep_inner_and_outer_effects_separate() {
    let fixture = nested_loops();
    let artifact = checked(&fixture.input, Completion::Complete);
    assert_eq!(bodies(&artifact, fixture.join), (1, 0));
    for (outer, inner) in [(0, 3), (2, 0), (1, 1), (2, 3)] {
        for left in [0, 1] {
            let (_, state) = equivalent(
                &fixture.input,
                &artifact,
                &[outer, inner, left, 0],
                &ExecutionLimits::default(),
            );
            assert_eq!(state.logs.len(), (outer * inner) as usize);
            if outer != 0 {
                assert_eq!(state.storage[&U256::from(4)], U256::from(1));
            }
        }
    }
    let (run, state) = equivalent(
        &fixture.input,
        &artifact,
        &[2, 3, 1, 2],
        &ExecutionLimits::default(),
    );
    assert!(matches!(
        run.outcome,
        ExecutionOutcome::Exit {
            kind: ExitKind::Revert,
            ..
        }
    ));
    assert_eq!(state.logs.len(), 1);
    assert!(!state.storage.contains_key(&U256::from(4)));
}
