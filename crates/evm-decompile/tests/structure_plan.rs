//! Source bytecode, CFG and structured execution use separate control engines.
//! These fixtures exercise region planning rather than any rendered spelling.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, ExecutionOutcome, Fork, Input, MemoryHost,
    StageKind, Strategy, StructuredKind, StructuredNode, U256, ValidationStatus, decompile,
    execute_bytecode, execute_ir, execute_structured,
};
use std::collections::BTreeMap;

#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    jumps: Vec<(usize, &'static str)>,
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
        self.emit(&[0x60, 0, if conditional { 0x57 } else { 0x56 }]);
        self.jumps.push((self.bytes.len() - 2, name));
    }
    fn finish(mut self) -> Input {
        for (position, label) in self.jumps {
            self.bytes[position] = u8::try_from(self.labels[label]).unwrap();
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}
fn calldata(words: &[u64]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|word| U256::from(*word).to_be_bytes::<32>())
        .collect()
}
fn checked(input: &Input, config: &Config) -> Artifact {
    let artifact = decompile(input, config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(
        artifact.current().unwrap().kind,
        StageKind::Lifted,
        "{:?}",
        artifact.diagnostics
    );
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed)
    );
    artifact
}
fn equivalent(input: &Input, artifact: &Artifact, words: &[u64]) -> MemoryHost {
    let limits = ExecutionLimits::default();
    let host = MemoryHost::new(input, calldata(words));
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
        assert_eq!(
            graph.outcome, raw.outcome,
            "CFG {:?}, inputs={words:?}",
            stage.kind
        );
        assert_eq!(
            graph.observations(),
            raw.observations(),
            "CFG {:?}, inputs={words:?}",
            stage.kind
        );
        assert_eq!(graph_host, raw_host);
        if stage.ir.structured.is_some() {
            let mut syntax_host = host.clone();
            let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
            assert_eq!(
                syntax.outcome, raw.outcome,
                "AST {:?}, inputs={words:?}",
                stage.kind
            );
            assert_eq!(
                syntax.observations(),
                raw.observations(),
                "AST {:?}, inputs={words:?}",
                stage.kind
            );
            assert_eq!(syntax_host, raw_host);
        }
    }
    raw_host
}
fn occurrences(node: &StructuredNode, pc: usize, ir: &evm_decompile::Ir) -> (usize, usize) {
    let mut bodies = 0;
    let mut dispatchers = 0;
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match &node.kind {
            StructuredKind::Block { block } => {
                bodies += usize::from(ir.block(*block).unwrap().source.is_some_and(|s| s.pc == pc))
            }
            StructuredKind::Sequence(children) => pending.extend(children),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => pending.extend([&**then_branch, &**else_branch]),
            StructuredKind::While { header, body, .. } => pending.extend([&**header, &**body]),
            StructuredKind::Indirect { arms, .. } => {
                pending.extend(arms.iter().map(|arm| &arm.body))
            }
            StructuredKind::Dispatcher { cases, .. } => {
                dispatchers += 1;
                bodies += cases
                    .iter()
                    .filter(|case| {
                        ir.block(case.block)
                            .unwrap()
                            .source
                            .is_some_and(|s| s.pc == pc)
                    })
                    .count();
            }
            _ => {}
        }
    }
    (bodies, dispatchers)
}
fn shared_tail(early: &[u8]) -> (Input, usize) {
    let mut code = Asm::default();
    code.emit(&[0x60, 0, 0x35]);
    code.jump("a", true);
    code.emit(&[0x60, 2]);
    code.jump("tail", false);
    code.label("a");
    code.emit(&[0x60, 0x20, 0x35]);
    code.jump("early", true);
    code.emit(&[0x60, 1]);
    code.jump("tail", false);
    code.label("early");
    code.emit(early);
    code.label("tail");
    let tail = code.labels["tail"];
    code.emit(&[0x5f, 0x55, 0x60, 0x2a, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
    (code.finish(), tail)
}

#[test]
fn thirty_nine_byte_conditional_tail_is_shared_before_materialization() {
    let (input, tail) = shared_tail(&[0x60, 0, 0x5f, 0xfd]);
    assert_eq!(input.runtime_bytecode.len(), 39);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let artifact = checked(
            &input,
            &Config {
                strategy,
                ..Config::default()
            },
        );
        let stage = artifact.current().unwrap();
        assert_eq!(
            occurrences(&stage.ir.structured.as_ref().unwrap().root, tail, &stage.ir),
            (1, 0)
        );
        for first in [0, 1] {
            for second in [0, 1] {
                let host = equivalent(&input, &artifact, &[first, second]);
                assert_eq!(
                    host.storage.get(&U256::ZERO).copied(),
                    if first != 0 && second != 0 {
                        None
                    } else {
                        Some(U256::from(if first == 0 { 2 } else { 1 }))
                    }
                );
            }
        }
        let wire = serde_json::to_vec(&artifact).unwrap();
        assert_eq!(serde_json::from_slice::<Artifact>(&wire).unwrap(), artifact);
    }
}

#[test]
fn conditional_tails_preserve_each_kind_of_early_exit_and_effect_order() {
    for early in [
        vec![0x00],
        vec![0x5f, 0x5f, 0xf3],
        vec![0x5f, 0x5f, 0xfd],
        vec![0x5f, 0xff],
        vec![0xfe],
        vec![0x60, 0xff, 0x56],
    ] {
        let (input, _) = shared_tail(&early);
        let artifact = checked(&input, &Config::default());
        for first in [0, 1] {
            for second in [0, 1] {
                equivalent(&input, &artifact, &[first, second]);
            }
        }
    }
}

#[test]
fn forced_compact_admission_preserves_simultaneous_phi_and_all_source_effects() {
    let (input, tail) = shared_tail(&[0x60, 0, 0x5f, 0xfd]);
    let artifact = checked(
        &input,
        &Config {
            limits: evm_decompile::Limits {
                max_structuring_depth: 0,
                ..evm_decompile::Limits::default()
            },
            ..Config::default()
        },
    );
    let stage = artifact.current().unwrap();
    assert!(occurrences(&stage.ir.structured.as_ref().unwrap().root, tail, &stage.ir).1 > 0);
    for first in [0, 1] {
        for second in [0, 1] {
            equivalent(&input, &artifact, &[first, second]);
        }
    }
    let explicit = decompile(
        &input,
        &Config {
            strategy: Strategy::CodeDuplication,
            limits: evm_decompile::Limits {
                max_structuring_depth: 0,
                ..evm_decompile::Limits::default()
            },
            ..Config::default()
        },
    );
    assert_eq!(explicit.completion, Completion::Limited);
    assert!(explicit.current().unwrap().ir.structured.is_none());
}

fn multiple_loop_exits() -> Input {
    let mut code = Asm::default();
    code.emit(&[0x5f]);
    code.label("head");
    code.emit(&[0x80, 0x60, 3, 0x90, 0x10, 0x15]);
    code.jump("last", true);
    code.emit(&[0x5f, 0x35]);
    code.jump("early", true);
    code.emit(&[0x60, 1, 0x01]);
    code.jump("head", false);
    code.label("early");
    code.emit(&[0x60, 11, 0x60, 1, 0x55]);
    code.jump("follow", false);
    code.label("last");
    code.emit(&[0x60, 22, 0x60, 1, 0x55]);
    code.jump("follow", false);
    code.label("follow");
    code.emit(&[0x80, 0x5f, 0x55, 0x50, 0x00]);
    code.finish()
}

#[test]
fn multi_exit_loop_keeps_the_original_bridge_effects_and_continuation() {
    let input = multiple_loop_exits();
    for depth in [0, 512] {
        let artifact = checked(
            &input,
            &Config {
                limits: evm_decompile::Limits {
                    max_structuring_depth: depth,
                    ..evm_decompile::Limits::default()
                },
                ..Config::default()
            },
        );
        for flag in [0, 1] {
            let host = equivalent(&input, &artifact, &[flag]);
            assert_eq!(
                host.storage[&U256::ZERO],
                U256::from(if flag == 0 { 3 } else { 0 })
            );
            assert_eq!(
                host.storage[&U256::from(1)],
                U256::from(if flag == 0 { 22 } else { 11 })
            );
        }
    }
}

#[test]
fn unresolved_target_retains_a_partial_boundary_after_observable_prefix() {
    let input = Input::new(vec![0x60, 9, 0x5f, 0x55, 0x5f, 0x35, 0x56], Fork::Osaka);
    let artifact = decompile(&input, &Config::default());
    assert_eq!(
        artifact.completion,
        Completion::Partial,
        "{:?}",
        artifact.diagnostics
    );
    let stage = artifact.current().unwrap();
    assert!(stage.ir.structured.is_some(), "{:?}", artifact.diagnostics);
    let mut graph_host = MemoryHost::new(&input, calldata(&[255]));
    let mut syntax_host = graph_host.clone();
    let graph = execute_ir(stage, &mut graph_host, &ExecutionLimits::default()).unwrap();
    let syntax = execute_structured(stage, &mut syntax_host, &ExecutionLimits::default()).unwrap();
    assert!(matches!(
        graph.outcome,
        ExecutionOutcome::PartialJump { .. }
    ));
    assert_eq!(syntax.outcome, graph.outcome);
    assert_eq!(syntax.observations(), graph.observations());
    assert_eq!(syntax_host, graph_host);
    assert_eq!(graph_host.storage[&U256::ZERO], U256::from(9));
}

#[test]
fn refused_work_retains_only_previously_validated_stages() {
    let (input, _) = shared_tail(&[0x60, 0, 0x5f, 0xfd]);
    for work in [0, 10, 1_000] {
        let artifact = decompile(
            &input,
            &Config {
                limits: evm_decompile::Limits {
                    max_work: work,
                    ..evm_decompile::Limits::default()
                },
                ..Config::default()
            },
        );
        assert_eq!(artifact.completion, Completion::Limited);
        assert!(
            artifact
                .stages
                .iter()
                .all(|stage| stage.validation.status == ValidationStatus::Passed)
        );
        if let Some(current) = artifact.current() {
            assert_eq!(current.validation.status, ValidationStatus::Passed);
        }
    }
}
