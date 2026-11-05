use evm_decompile::{
    Artifact, Completion, Config, Execution, ExecutionLimits, Fork, Input, MemoryHost, StageKind,
    U256, ValidationStatus, decompile, execute_bytecode, execute_ir, execute_structured,
};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Asm {
    bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    pub(super) fn pc(&self) -> usize {
        self.bytes.len()
    }

    pub(super) fn op(&mut self, opcode: u8) -> &mut Self {
        self.bytes.push(opcode);
        self
    }

    pub(super) fn push(&mut self, value: u8) -> &mut Self {
        self.bytes.extend([0x60, value]);
        self
    }

    pub(super) fn dest(&mut self, label: &'static str) -> &mut Self {
        self.labels.insert(label, self.bytes.len());
        self.op(0x5b)
    }

    pub(super) fn target(&mut self, label: &'static str) -> &mut Self {
        self.op(0x61);
        self.fixups.push((self.bytes.len(), label));
        self.bytes.extend([0, 0]);
        self
    }

    pub(super) fn finish(mut self) -> Input {
        for (offset, label) in self.fixups {
            self.bytes[offset..offset + 2].copy_from_slice(
                &u16::try_from(self.labels[label])
                    .expect("small complete bytecode fixture")
                    .to_be_bytes(),
            );
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}

pub(super) fn calldata(words: &[U256]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|word| word.to_be_bytes::<32>())
        .collect()
}

pub(super) fn checked(input: &Input) -> Artifact {
    let artifact = decompile(input, &Config::default());
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    validated(&artifact);
    artifact
}

pub(super) fn validated(artifact: &Artifact) {
    for stage in &artifact.stages {
        assert_eq!(
            stage.validation.status,
            ValidationStatus::Passed,
            "{:?}: {:?}",
            stage.kind,
            stage.validation
        );
    }
}

/// The raw PC/stack machine supplies both observable behavior and every original
/// primitive result. Lifted expression checks consume this trace independently.
pub(super) fn differential(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
) -> (Execution, MemoryHost) {
    let limits = ExecutionLimits::default();
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
        assert_eq!(raw.outcome, graph.outcome, "IR {:?}", stage.kind);
        assert_eq!(
            raw.observations(),
            graph.observations(),
            "IR {:?}",
            stage.kind
        );
        assert_eq!(raw_host, graph_host, "IR host {:?}", stage.kind);
        if stage.ir.structured.is_some() {
            let mut syntax_host = host.clone();
            let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
            assert_eq!(raw.outcome, syntax.outcome, "structured {:?}", stage.kind);
            assert_eq!(
                raw.observations(),
                syntax.observations(),
                "structured {:?}",
                stage.kind
            );
            assert_eq!(raw_host, syntax_host, "structured host {:?}", stage.kind);
        }
    }
    (raw, raw_host)
}

pub(super) fn comparison_branch(opcode: u8, inversions: usize) -> Input {
    let mut asm = Asm::default();
    asm.push(32).op(0x35).op(0x5f).op(0x35).op(opcode);
    for _ in 0..inversions {
        asm.op(0x15);
    }
    asm.target("true")
        .op(0x57)
        .op(0x5f)
        .op(0x5f)
        .op(0x55)
        .op(0x00)
        .dest("true")
        .push(1)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    asm.finish()
}
