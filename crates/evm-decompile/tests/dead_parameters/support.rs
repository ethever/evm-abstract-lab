use evm_decompile::{
    Artifact, Execution, ExecutionLimits, ExecutionOutcome, ExitKind, Fork, Input, MemoryHost,
    StageKind, U256, execute_bytecode, execute_ir, execute_structured,
};
use std::collections::BTreeMap;

pub(super) const ADMIN_SLOT: &str =
    "10d6a54a4754c8869d6886b5f5d7fbfa5b4522237ea5c60d11bc4e7a1ff9390b";
pub(super) const IMPLEMENTATION_SLOT: &str =
    "7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3";

pub(super) fn word(hex: &str) -> U256 {
    U256::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap()
}

pub(super) fn proxy_host(input: &Input, data: Vec<u8>, admin: bool) -> MemoryHost {
    let capture: serde_json::Value = serde_json::from_str(super::USDC).unwrap();
    let account = &capture["accounts"][1];
    let mut host = MemoryHost::new(input, data);
    host.storage = account["storage"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (word(key), word(value.as_str().unwrap())))
        .collect();
    let caller = if admin {
        host.storage[&word(ADMIN_SLOT)]
    } else {
        U256::from(0xcafe)
    };
    host.context
        .insert(0x30, word(account["address"].as_str().unwrap()));
    host.context.insert(0x33, caller);
    host.context.insert(0x3b, U256::from(1));
    host.context.insert(0x5a, U256::from(1_000_000));
    host
}

pub(super) fn upgrade_and_call(implementation: U256, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x4f, 0x1e, 0xf2, 0x86];
    for argument in [implementation, U256::from(64), U256::from(payload.len())] {
        bytes.extend(argument.to_be_bytes::<32>());
    }
    bytes.extend(payload);
    bytes.resize(4 + 96 + payload.len().div_ceil(32) * 32, 0);
    bytes
}

pub(super) fn calldata(value: u64) -> Vec<u8> {
    U256::from(value).to_be_bytes::<32>().to_vec()
}

/// Compare the direct EVM stack machine to every published executable stage.
pub(super) fn differential(
    input: &Input,
    artifact: &Artifact,
    host: MemoryHost,
    limits: &ExecutionLimits,
) -> (Execution, MemoryHost) {
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, limits).unwrap();
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = host.clone();
        let graph = execute_ir(stage, &mut graph_host, limits)
            .unwrap_or_else(|error| panic!("IR {:?}: {error}", stage.kind));
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
            let syntax = execute_structured(stage, &mut syntax_host, limits)
                .unwrap_or_else(|error| panic!("structured {:?}: {error}", stage.kind));
            assert_eq!(raw.outcome, syntax.outcome, "structured {:?}", stage.kind);
            assert_eq!(raw.observations(), syntax.observations());
            assert_eq!(raw_host, syntax_host, "structured host {:?}", stage.kind);
        }
    }
    (raw, raw_host)
}

pub(super) fn exit(execution: &Execution, expected: ExitKind) {
    assert!(
        matches!(execution.outcome, ExecutionOutcome::Exit { kind, .. } if kind == expected),
        "{:?}",
        execution.outcome
    );
}

#[derive(Default)]
pub(super) struct Asm {
    bytes: Vec<u8>,
    pub(super) labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    pub(super) fn op(&mut self, opcode: u8) -> &mut Self {
        self.bytes.push(opcode);
        self
    }

    pub(super) fn push(&mut self, value: u8) -> &mut Self {
        self.bytes.extend([0x60, value]);
        self
    }

    pub(super) fn dest(&mut self, name: &'static str) -> &mut Self {
        self.labels.insert(name, self.bytes.len());
        self.op(0x5b)
    }

    pub(super) fn target(&mut self, name: &'static str) -> &mut Self {
        self.bytes.push(0x61);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.extend([0, 0]);
        self
    }

    pub(super) fn finish(mut self) -> Input {
        for (offset, name) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[name]).unwrap().to_be_bytes());
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}
