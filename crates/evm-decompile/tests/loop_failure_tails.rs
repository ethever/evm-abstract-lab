//! Failure tails remain explicit executable regions inside natural loops.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, ExecutionOutcome, ExitKind, ExternalResponse,
    Fork, Input, MemoryHost, StageKind, Strategy, StructuredKind, StructuredNode, U256,
    ValidationStatus, decompile, execute_bytecode, execute_ir, execute_structured,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn label(&mut self, name: &'static str) {
        self.labels.insert(name, self.bytes.len());
        self.emit(&[0x5b]);
    }

    fn jump(&mut self, name: &'static str, conditional: bool) {
        self.emit(&[0x61, 0, 0, if conditional { 0x57 } else { 0x56 }]);
        self.fixups.push((self.bytes.len() - 3, name));
    }

    fn load(&mut self, offset: u8) {
        self.emit(&[0x60, offset, 0x35]);
    }

    fn finish(mut self) -> (Input, BTreeMap<&'static str, usize>) {
        for (offset, label) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[label]).unwrap().to_be_bytes());
        }
        (Input::new(self.bytes, Fork::Osaka), self.labels)
    }
}

#[derive(Clone, Copy, Debug)]
enum Tail {
    Revert,
    Invalid,
    Cycle,
    Unknown,
}

fn fixture(tail: Tail) -> (Input, BTreeMap<&'static str, usize>) {
    let mut a = Asm::default();
    a.emit(&[0x5f]);
    a.label("header");
    a.emit(&[0x80, 0x60, 3, 0x90, 0x10, 0x15]);
    a.jump("normal", true);
    a.emit(&[0x60, 1, 0x01, 0x80, 0x5f, 0x55]);
    a.load(0);
    a.emit(&[0x81, 0x14]);
    a.jump("failure_a", true);
    a.load(32);
    a.jump("failure_b", true);
    a.jump("header", false);

    a.label("failure_a");
    a.emit(&[0x60, 0xa1, 0x60, 1, 0x55]);
    a.load(64);
    a.jump("failure_left", true);
    a.emit(&[0x60, 0x11]);
    a.jump("shared_failure", false);
    a.label("failure_left");
    a.emit(&[0x60, 0x12]);
    a.jump("shared_failure", false);
    a.label("failure_b");
    a.emit(&[0x60, 0xb1, 0x60, 1, 0x55, 0x60, 0x21]);
    a.jump("shared_failure", false);

    a.label("shared_failure");
    // The incoming tag differs on all three edges. Preserve it across stores,
    // memory, a log and an external call before reaching the real terminal.
    a.emit(&[
        0x80, 0x60, 2, 0x55, 0x80, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xa0,
    ]);
    a.emit(&[
        0x5f, 0x5f, 0x5f, 0x5f, 0x5f, 0x60, 0x44, 0x61, 0xff, 0xff, 0xf1, 0x50,
    ]);
    a.jump("failure_end", false);
    a.label("failure_end");
    match tail {
        Tail::Revert => a.emit(&[0x60, 0x20, 0x5f, 0xfd]),
        Tail::Invalid => a.emit(&[0xfe]),
        Tail::Cycle => a.jump("failure_end", false),
        Tail::Unknown => {
            a.load(96);
            a.emit(&[0x56]);
        }
    }

    a.label("normal");
    a.emit(&[0x80, 0x60, 3, 0x55]);
    a.jump("normal_tail", false);
    a.label("normal_tail");
    a.emit(&[0x80, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
    a.finish()
}

fn checked(input: &Input, strategy: Strategy) -> Artifact {
    let artifact = decompile(
        input,
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
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed)
    );
    assert!(artifact.diagnostics.iter().all(|diagnostic| {
        !diagnostic.message.contains("source-control verification")
            && !diagnostic.message.contains("whole-source dispatcher")
    }));
    let simplified = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Simplified)
        .unwrap();
    let mut structured = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .unwrap()
        .ir
        .clone();
    structured.structured = None;
    assert_eq!(structured, simplified.ir);
    artifact
}

fn host(input: &Input, words: &[u64]) -> MemoryHost {
    let mut host = MemoryHost::new(
        input,
        words
            .iter()
            .flat_map(|word| U256::from(*word).to_be_bytes::<32>())
            .collect(),
    );
    host.responses.push_back(ExternalResponse {
        result: U256::from(1),
        return_data: vec![0xab, 0xcd],
        storage_writes: BTreeMap::from([(U256::from(5), U256::from(0x51))]),
    });
    host
}

fn equivalent(input: &Input, artifact: &Artifact, words: &[u64]) -> (MemoryHost, ExecutionOutcome) {
    let limits = ExecutionLimits {
        max_steps: 2_000,
        ..ExecutionLimits::default()
    };
    let initial = host(input, words);
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = initial.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
        assert_eq!(
            graph.outcome, raw.outcome,
            "CFG {:?}, {words:?}",
            stage.kind
        );
        assert_eq!(graph.observations(), raw.observations());
        assert_eq!(graph_host, raw_host);
        if stage.ir.structured.is_some() {
            let mut syntax_host = initial.clone();
            let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
            assert_eq!(
                syntax.outcome, raw.outcome,
                "AST {:?}, {words:?}",
                stage.kind
            );
            assert_eq!(syntax.observations(), raw.observations());
            assert_eq!(syntax_host, raw_host);
        }
    }
    (raw_host, raw.outcome)
}

fn visit(node: &StructuredNode, f: &mut impl FnMut(&StructuredNode)) {
    f(node);
    match &node.kind {
        StructuredKind::Sequence(children) => {
            for child in children {
                visit(child, f);
            }
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            visit(then_branch, f);
            visit(else_branch, f);
        }
        StructuredKind::While { header, body, .. } => {
            visit(header, f);
            visit(body, f);
        }
        StructuredKind::Indirect { arms, .. } => {
            for arm in arms {
                visit(&arm.body, f);
            }
        }
        _ => {}
    }
}

#[test]
fn branching_shared_failure_dags_admit_a_loop_with_unchanged_effects_and_edge_values() {
    for tail in [Tail::Revert, Tail::Invalid] {
        let (input, labels) = fixture(tail);
        for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
            let artifact = checked(&input, strategy);
            let ir = &artifact.current().unwrap().ir;
            let root = &ir.structured.as_ref().unwrap().root;
            let (mut loops, mut dispatchers, mut normal, mut normal_tail) = (0, 0, 0, 0);
            visit(root, &mut |node| match node.kind {
                StructuredKind::While { .. } => loops += 1,
                StructuredKind::Dispatcher { .. } => dispatchers += 1,
                StructuredKind::Block { block } => {
                    let pc = ir.block(block).unwrap().source.unwrap().pc;
                    normal += usize::from(pc == labels["normal"]);
                    normal_tail += usize::from(pc == labels["normal_tail"]);
                }
                _ => {}
            });
            assert_eq!((loops, dispatchers), (1, 0), "{tail:?}");
            assert_eq!((normal, normal_tail), (1, 1));
            let shared = ir
                .blocks
                .iter()
                .find(|block| {
                    block
                        .source
                        .is_some_and(|s| s.pc == labels["shared_failure"])
                })
                .unwrap();
            assert!(!shared.parameters.is_empty(), "exercise edge-carried tags");
            for fail_at in [0, 1, 2, 3] {
                for fail_b in [0, 1] {
                    for side in [0, 1] {
                        let (host, outcome) =
                            equivalent(&input, &artifact, &[fail_at, fail_b, side, 0]);
                        let failure = fail_at != 0 || fail_b != 0;
                        if failure {
                            let first = fail_at == 1 || fail_b == 0;
                            let tag = if first {
                                if side == 0 { 0x11 } else { 0x12 }
                            } else {
                                0x21
                            };
                            assert_eq!(host.storage[&U256::from(2)], U256::from(tag));
                            assert_eq!(host.storage[&U256::from(5)], U256::from(0x51));
                            assert_eq!(host.logs.len(), 1);
                            assert_eq!(host.logs[0].2, U256::from(tag).to_be_bytes::<32>());
                            assert!(host.responses.is_empty());
                            match tail {
                                Tail::Revert => assert!(matches!(
                                    outcome,
                                    ExecutionOutcome::Exit {
                                        kind: ExitKind::Revert,
                                        ..
                                    }
                                )),
                                Tail::Invalid => {
                                    assert!(matches!(outcome, ExecutionOutcome::Exceptional { .. }))
                                }
                                _ => unreachable!(),
                            }
                        } else {
                            assert!(matches!(
                                outcome,
                                ExecutionOutcome::Exit {
                                    kind: ExitKind::Return,
                                    ..
                                }
                            ));
                            assert_eq!(host.storage[&U256::from(3)], U256::from(3));
                            assert!(host.logs.is_empty());
                            assert_eq!(host.responses.len(), 1);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn residual_failure_cycle_keeps_its_nonterminating_execution() {
    let (input, _) = fixture(Tail::Cycle);
    let artifact = checked(&input, Strategy::Dispatcher);
    let (host, outcome) = equivalent(&input, &artifact, &[1, 0, 0, 0]);
    assert_eq!(outcome, ExecutionOutcome::StepLimit);
    assert_eq!(host.logs.len(), 1);
    assert_eq!(host.storage[&U256::from(2)], U256::from(0x11));
    let (_, outcome) = equivalent(&input, &artifact, &[0, 0, 0, 0]);
    assert!(matches!(
        outcome,
        ExecutionOutcome::Exit {
            kind: ExitKind::Return,
            ..
        }
    ));
}

#[test]
fn a_shared_eventually_reverting_follow_stays_outside_the_loop() {
    let mut a = Asm::default();
    a.emit(&[0x5f]);
    a.label("header");
    a.emit(&[0x80, 0x60, 3, 0x90, 0x10, 0x15]);
    a.jump("last", true);
    a.load(0);
    a.jump("early", true);
    a.emit(&[0x60, 1, 0x01]);
    a.jump("header", false);
    a.label("early");
    a.emit(&[0x60, 0x11, 0x60, 1, 0x55]);
    a.jump("follow", false);
    a.label("last");
    a.emit(&[0x60, 0x22, 0x60, 1, 0x55]);
    a.jump("follow", false);
    a.label("follow");
    a.emit(&[0x80, 0x5f, 0x52]);
    a.jump("follow_tail", false);
    a.label("follow_tail");
    a.emit(&[0x60, 0x20, 0x5f, 0xfd]);
    let (input, labels) = a.finish();
    let artifact = checked(&input, Strategy::Dispatcher);
    let ir = &artifact.current().unwrap().ir;
    let mut loops = 0;
    let mut follows = 0;
    visit(&ir.structured.as_ref().unwrap().root, &mut |node| {
        if let StructuredKind::While { .. } = node.kind {
            loops += 1;
            visit(node, &mut |inside| {
                if let StructuredKind::Block { block } = inside.kind {
                    let pc = ir.block(block).unwrap().source.unwrap().pc;
                    assert!(![labels["follow"], labels["follow_tail"]].contains(&pc));
                }
            });
        }
        if let StructuredKind::Block { block } = node.kind {
            follows += usize::from(ir.block(block).unwrap().source.unwrap().pc == labels["follow"]);
        }
    });
    assert_eq!((loops, follows), (1, 1));
    for flag in [0, 1] {
        let (host, outcome) = equivalent(&input, &artifact, &[flag]);
        assert!(matches!(
            outcome,
            ExecutionOutcome::Exit {
                kind: ExitKind::Revert,
                ..
            }
        ));
        assert_eq!(host.memory[31], if flag == 0 { 3 } else { 0 });
        assert_eq!(
            host.storage[&U256::from(1)],
            U256::from(if flag == 0 { 0x22 } else { 0x11 })
        );
    }
}

#[test]
fn unknown_failure_tail_remains_partial_and_preserves_its_observable_prefix() {
    let (input, _) = fixture(Tail::Unknown);
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Partial);
    let stage = artifact.current().unwrap();
    let initial = host(&input, &[1, 0, 0, 0xffff]);
    let mut graph_host = initial.clone();
    let graph = execute_ir(stage, &mut graph_host, &ExecutionLimits::default()).unwrap();
    let mut syntax_host = initial;
    let syntax = execute_structured(stage, &mut syntax_host, &ExecutionLimits::default()).unwrap();
    assert!(matches!(
        graph.outcome,
        ExecutionOutcome::PartialJump { .. }
    ));
    assert_eq!(syntax.outcome, graph.outcome);
    assert_eq!(syntax.observations(), graph.observations());
    assert_eq!(syntax_host, graph_host);
    assert_eq!(graph_host.logs.len(), 1);
    assert_eq!(graph_host.storage[&U256::from(2)], U256::from(0x11));
}
