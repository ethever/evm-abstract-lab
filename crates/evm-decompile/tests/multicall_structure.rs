//! Real runtime regression for natural loops with out-of-line failure tails.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, ExecutionOutcome, ExitKind, ExternalResponse,
    Fork, Input, MemoryHost, StageKind, StructuredKind, U256, ValidationStatus, decompile,
    execute_bytecode, execute_ir, execute_structured,
};
use std::{collections::BTreeMap, sync::OnceLock};

fn fixture() -> &'static (Input, Artifact) {
    static FIXTURE: OnceLock<(Input, Artifact)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let bytes =
            evm_bytecode::parse_hex(include_str!("fixtures/structure/ethereum-multicall3.hex"))
                .unwrap();
        let metadata: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/structure/ethereum-multicall3.json"))
                .unwrap();
        assert_eq!(bytes.len() as u64, metadata["byte_length"]);
        assert_eq!(
            format!("{:#x}", alloy_primitives::keccak256(&bytes)),
            metadata["code_hash"].as_str().unwrap()
        );
        let input = Input::new(bytes, Fork::Osaka);
        let artifact = decompile(&input, &Config::default());
        assert_eq!(artifact.completion, Completion::Complete);
        assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
        assert!(
            artifact.diagnostics.is_empty(),
            "{:?}",
            artifact.diagnostics
        );
        assert!(
            artifact
                .stages
                .iter()
                .all(|s| s.validation.status == ValidationStatus::Passed)
        );
        (input, artifact)
    })
}

#[test]
fn balanced_multicall_retains_each_call_loop_without_dispatch() {
    let (_, artifact) = fixture();
    let ir = &artifact.current().unwrap().ir;
    let mut pending = vec![&ir.structured.as_ref().unwrap().root];
    let mut headers = BTreeMap::<usize, usize>::new();
    while let Some(node) = pending.pop() {
        match &node.kind {
            StructuredKind::Sequence(nodes) => pending.extend(nodes),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                pending.extend([&**then_branch, &**else_branch]);
            }
            StructuredKind::While { header, body, .. } => {
                if let StructuredKind::Block { block } = header.kind {
                    *headers
                        .entry(ir.block(block).unwrap().source.unwrap().pc)
                        .or_default() += 1;
                }
                pending.extend([&**header, &**body]);
            }
            StructuredKind::Indirect { arms, .. } => pending.extend(arms.iter().map(|a| &a.body)),
            StructuredKind::Dispatcher { .. } => {
                panic!("reducible Multicall3 loops must remain natural")
            }
            _ => {}
        }
    }
    // The final header is the shared tryAggregate code in three call contexts.
    for (pc, count) in [(0x325, 1), (0x546, 1), (0x714, 1), (0x8c3, 3)] {
        assert_eq!(headers.get(&pc), Some(&count), "loop at {pc:#x}");
    }
}

#[derive(Clone, Copy, Debug)]
enum Method {
    Aggregate,
    Aggregate3,
    Aggregate3Value,
    TryAggregate,
    TryBlockAndAggregate,
    BlockAndAggregate,
}
impl Method {
    fn selector(self) -> [u8; 4] {
        match self {
            Self::Aggregate => [0x25, 0x2d, 0xba, 0x42],
            Self::Aggregate3 => [0x82, 0xad, 0x56, 0xcb],
            Self::Aggregate3Value => [0x17, 0x4d, 0xea, 0x71],
            Self::TryAggregate => [0xbc, 0xe3, 0x8b, 0xd7],
            Self::TryBlockAndAggregate => [0x39, 0x95, 0x42, 0xe9],
            Self::BlockAndAggregate => [0xc3, 0x07, 0x7f, 0xa9],
        }
    }
    fn supports_failure(self) -> bool {
        !matches!(self, Self::Aggregate | Self::BlockAndAggregate)
    }
    fn value(self) -> bool {
        matches!(self, Self::Aggregate3Value)
    }
    fn calldata(self, allow_failure: bool) -> Vec<u8> {
        let mut data = self.selector().to_vec();
        if matches!(self, Self::TryAggregate | Self::TryBlockAndAggregate) {
            data.extend(words(&[u64::from(!allow_failure), 64]));
        } else {
            data.extend(words(&[32]));
        }
        let allow_field = matches!(self, Self::Aggregate3 | Self::Aggregate3Value);
        let mut tuples = Vec::new();
        for (address, value, byte) in [(0x11, 3, 0xaa), (0x22, 5, 0xbb)] {
            let mut head = vec![address];
            if allow_field {
                head.push(u64::from(allow_failure));
            }
            if self.value() {
                head.push(value);
            }
            head.push((head.len() as u64 + 1) * 32);
            let mut tuple = words(&head);
            tuple.extend(words(&[1]));
            tuple.push(byte);
            tuple.resize(tuple.len() + 31, 0);
            tuples.push(tuple);
        }
        data.extend(words(&[2, 64, 64 + tuples[0].len() as u64]));
        data.extend(tuples.into_iter().flatten());
        data
    }
}

fn words(values: &[u64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| U256::from(*v).to_be_bytes::<32>())
        .collect()
}

fn compare(method: Method, allow_failure: bool, first_succeeds: bool, correct_value: bool) {
    let (input, artifact) = fixture();
    let mut initial = MemoryHost::new(input, method.calldata(allow_failure));
    initial.context.insert(
        0x34,
        U256::from(if method.value() && correct_value {
            8
        } else {
            0
        }),
    );
    for (success, byte) in [(first_succeeds, 0xa1), (true, 0xb2)] {
        initial.responses.push_back(ExternalResponse {
            result: U256::from(u64::from(success)),
            return_data: vec![byte; 3],
            ..ExternalResponse::default()
        });
    }
    let limits = ExecutionLimits::default();
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    let rejects_call = !first_succeeds && !(allow_failure && method.supports_failure());
    let expected = if rejects_call || (method.value() && !correct_value) {
        ExitKind::Revert
    } else {
        ExitKind::Return
    };
    assert!(
        matches!(raw.outcome, ExecutionOutcome::Exit { kind, .. } if kind == expected),
        "{method:?}: {:?}",
        raw.outcome
    );
    let calls: Vec<_> = raw
        .trace
        .iter()
        .filter(|step| step.opcode == 0xf1)
        .collect();
    assert_eq!(calls.len(), if rejects_call { 1 } else { 2 });
    assert_eq!(calls[0].inputs[1], U256::from(0x11));
    if calls.len() == 2 {
        assert_eq!(calls[1].inputs[1], U256::from(0x22));
    }
    assert_eq!(raw_host.responses.len(), 2 - calls.len());
    let stage = artifact.current().unwrap();
    let mut graph_host = initial.clone();
    let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut tree_host = initial;
    let tree = execute_structured(stage, &mut tree_host, &limits).unwrap();
    assert_eq!(graph.outcome, raw.outcome, "{method:?}");
    assert_eq!(tree.outcome, raw.outcome, "{method:?}");
    assert_eq!(graph.observations(), raw.observations(), "{method:?}");
    assert_eq!(tree.observations(), raw.observations(), "{method:?}");
    assert_eq!(graph_host, raw_host, "{method:?}");
    assert_eq!(tree_host, raw_host, "{method:?}");
}

#[test]
fn multicall_call_order_failure_policy_and_value_check_match_bytecode() {
    for method in [
        Method::Aggregate,
        Method::Aggregate3,
        Method::Aggregate3Value,
        Method::TryAggregate,
        Method::TryBlockAndAggregate,
        Method::BlockAndAggregate,
    ] {
        compare(method, false, true, true);
        compare(method, false, false, true);
        if method.supports_failure() {
            compare(method, true, false, true);
        }
    }
    // The value mismatch is checked after both calls, on the normal loop exit.
    compare(Method::Aggregate3Value, false, true, false);
}
