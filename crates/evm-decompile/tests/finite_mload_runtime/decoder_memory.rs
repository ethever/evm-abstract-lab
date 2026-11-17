//! Concrete ABI inputs exercise the supplied decoder, not a recreated loop.
//! The first hash precompile deliberately returns failure, so the comparison
//! ends at the source REVERT without requiring an external protocol execution.
use evm_decompile::{
    Artifact, Execution, ExecutionError, ExecutionLimits, ExecutionOutcome, ExitKind,
    ExternalResponse, Input, MemoryHost, Primitive, SemanticHost, StageKind, U256,
    execute_bytecode, execute_ir, execute_structured,
};

const D: usize = 0x40;
const CURSOR: usize = D + 0x11cd;
const OUTPUT: usize = 0x1360;

fn step(tag: u8) -> usize {
    match tag {
        0 | 7 => 1,
        1 | 2 => 185,
        3..=6 => 129,
        _ => unreachable!(),
    }
}

fn calldata(tags: &[u8]) -> (Vec<u8>, Vec<[u8; 256]>) {
    let mut stream = Vec::new();
    let mut records = Vec::new();
    for (index, &tag) in tags.iter().enumerate() {
        let mut encoded = vec![tag | 0xa0];
        encoded.extend((1..step(tag)).map(|byte| (byte + index * 17) as u8));
        let mut record = [0; 256];
        record[..32].copy_from_slice(&U256::from(tag).to_be_bytes::<32>());
        match tag {
            1 | 2 => {
                record[32..192].copy_from_slice(&encoded[1..161]);
                record[204..224].copy_from_slice(&encoded[161..181]);
                record[252..256].copy_from_slice(&encoded[181..185]);
            }
            3..=6 => record[32..160].copy_from_slice(&encoded[1..129]),
            _ => {}
        }
        records.push(record);
        stream.extend(encoded);
    }
    let mut proof = vec![0; 0x11c8];
    // Header fields are separately encoded from the source instructions.
    // Numerator and denominator one make each output record one hash group.
    proof[0x20..0x40].copy_from_slice(&U256::ONE.to_be_bytes::<32>());
    proof[0x11a0..0x11c0].copy_from_slice(&U256::ONE.to_be_bytes::<32>());
    proof[0x11c0..0x11c4].copy_from_slice(&(tags.len().max(1) as u32).to_be_bytes());
    proof[0x11c4..0x11c8].copy_from_slice(&(stream.len() as u32).to_be_bytes());
    proof.extend(stream);
    let padded = proof.len().div_ceil(32) * 32;
    let second = D + 32 + padded;
    let mut data = vec![0; 4 + second + 32];
    data[..4].copy_from_slice(&[0xf8, 0x1c, 0xcc, 0xbe]);
    data[4..36].copy_from_slice(&U256::from(D).to_be_bytes::<32>());
    data[36..68].copy_from_slice(&U256::from(second).to_be_bytes::<32>());
    data[4 + D..36 + D].copy_from_slice(&U256::from(proof.len()).to_be_bytes::<32>());
    data[36 + D..36 + D + proof.len()].copy_from_slice(&proof);
    (data, records)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    opcode: u8,
    inputs: Vec<U256>,
    outputs: usize,
    memory_limit: usize,
    result: Result<Vec<U256>, ExecutionError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordingHost {
    state: MemoryHost,
    requests: Vec<Request>,
}

impl SemanticHost for RecordingHost {
    fn execute(&mut self, primitive: Primitive<'_>) -> Result<Vec<U256>, ExecutionError> {
        let mut request = Request {
            opcode: primitive.opcode,
            inputs: primitive.inputs.to_vec(),
            outputs: primitive.output_count,
            memory_limit: primitive.memory_limit,
            result: Ok(Vec::new()),
        };
        let result = self.state.execute(primitive);
        request.result = result.clone();
        self.requests.push(request);
        result
    }
}

fn host(input: &Input, data: Vec<u8>) -> RecordingHost {
    let mut state = MemoryHost::new(input, data);
    state.context.insert(0x5a, U256::from(1_000_000));
    state.responses.push_back(ExternalResponse::default());
    RecordingHost {
        state,
        requests: Vec::new(),
    }
}

fn compare(
    input: &Input,
    artifact: &Artifact,
    initial: RecordingHost,
    limits: &ExecutionLimits,
) -> (Result<Execution, ExecutionError>, RecordingHost) {
    let mut raw_host = initial.clone();
    let raw = execute_bytecode(input, &mut raw_host, limits);
    for stage in artifact
        .stages
        .iter()
        .filter(|stage| stage.kind != StageKind::Decoded)
    {
        let mut graph_host = initial.clone();
        let graph = execute_ir(stage, &mut graph_host, limits);
        assert_eq!(
            graph.as_ref().map(|e| &e.outcome),
            raw.as_ref().map(|e| &e.outcome),
            "graph stage {:?}",
            stage.kind
        );
        assert_eq!(
            graph_host, raw_host,
            "graph changed ordered requests, first fault or memory/storage state"
        );
        if let (Ok(graph), Ok(raw)) = (&graph, &raw) {
            assert_eq!(
                graph
                    .observations()
                    .into_iter()
                    .filter(|o| !o.effects.is_pure())
                    .collect::<Vec<_>>(),
                raw.observations()
                    .into_iter()
                    .filter(|o| !o.effects.is_pure())
                    .collect::<Vec<_>>()
            );
        }
        if stage.ir.structured.is_some() {
            let mut syntax_host = initial.clone();
            let syntax = execute_structured(stage, &mut syntax_host, limits);
            assert_eq!(
                syntax.as_ref().map(|e| &e.outcome),
                raw.as_ref().map(|e| &e.outcome),
                "syntax stage {:?}",
                stage.kind
            );
            assert_eq!(
                syntax_host, raw_host,
                "syntax changed ordered requests, first fault or memory/storage state"
            );
            if let (Ok(syntax), Ok(raw)) = (&syntax, &raw) {
                assert_eq!(
                    syntax
                        .observations()
                        .into_iter()
                        .filter(|o| !o.effects.is_pure())
                        .collect::<Vec<_>>(),
                    raw.observations()
                        .into_iter()
                        .filter(|o| !o.effects.is_pure())
                        .collect::<Vec<_>>()
                );
            }
        }
    }
    (raw, raw_host)
}

/// Reuse the caller's single full-runtime artifact. No second decompilation or
/// rewritten entry trampoline weakens the source ABI guards in these cases.
pub fn certify(input: &Input, artifact: &Artifact) {
    let cases: &[&[u8]] = &[
        &[],
        &[0],
        &[1],
        &[2],
        &[3],
        &[6],
        &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[0, 1, 2, 3, 6, 0, 2, 6, 1, 3, 0, 6],
        &[7],
        &[0, 1, 7, 6],
    ];
    for &tags in cases {
        let (data, records) = calldata(tags);
        let (raw, actual) = compare(
            input,
            artifact,
            host(input, data),
            &ExecutionLimits::default(),
        );
        let raw = raw.unwrap();
        let count = tags
            .iter()
            .position(|tag| *tag == 7)
            .map_or(tags.len(), |i| i + 1);
        let mut cursor = CURSOR;
        let mut expected_cursors = Vec::new();
        let mut expected_returns = Vec::new();
        for &tag in &tags[..count] {
            expected_cursors.push(U256::from(cursor));
            if tag != 7 {
                cursor += step(tag);
                expected_returns.push(U256::from(cursor));
            }
        }
        assert_eq!(
            raw.trace
                .iter()
                .filter(|o| o.pc == Some(0x3a7a))
                .map(|o| o.inputs[0])
                .collect::<Vec<_>>(),
            expected_cursors,
            "decoder must consume the actual 1/185/129-byte records"
        );
        assert_eq!(
            raw.trace
                .iter()
                .filter(|o| matches!(o.pc, Some(0x3ec3 | 0x3eee | 0x3f03)))
                .map(|o| o.outputs[0])
                .collect::<Vec<_>>(),
            expected_returns
        );
        for (index, expected) in records.iter().take(count).enumerate() {
            let start = OUTPUT + index * 256;
            assert_eq!(
                &actual.state.memory[start..start + 256],
                expected,
                "record {index}, tags {tags:?}"
            );
        }
        let ExecutionOutcome::Exit {
            kind: ExitKind::Revert,
            values,
        } = raw.outcome
        else {
            panic!("expected source failure boundary, tags {tags:?}")
        };
        if tags[..count].contains(&7) {
            assert!(!actual.requests.iter().any(|r| r.opcode == 0xfa));
            let start = usize::try_from(values[0]).unwrap();
            assert_eq!(values[1], U256::from(4));
            assert_eq!(
                &actual.state.memory[start..start + 4],
                &[0xd0, 0x42, 0x6f, 0x7b]
            );
        } else {
            assert_eq!(
                actual.requests.iter().filter(|r| r.opcode == 0xfa).count(),
                1
            );
            assert_eq!(values[1], U256::ZERO);
        }
    }
    let tags = &[0, 1, 2, 3, 6, 0, 2, 6, 1, 3, 0, 6];
    let (data, _) = calldata(tags);
    for memory in [0x100, 0x1360, 0x1480] {
        let limits = ExecutionLimits {
            max_memory_bytes: memory,
            ..ExecutionLimits::default()
        };
        let (raw, actual) = compare(input, artifact, host(input, data.clone()), &limits);
        assert_eq!(raw.unwrap_err(), ExecutionError::MemoryLimit);
        if memory == 0x1480 {
            assert_eq!(actual.requests.last().unwrap().opcode, 0x37);
        }
    }
    for steps in [1, 32] {
        let limits = ExecutionLimits {
            max_steps: steps,
            ..ExecutionLimits::default()
        };
        let (raw, _) = compare(input, artifact, host(input, data.clone()), &limits);
        assert_eq!(raw.unwrap().outcome, ExecutionOutcome::StepLimit);
    }
    let mut invalid = data;
    invalid[4..36].copy_from_slice(&U256::MAX.to_be_bytes::<32>());
    let (raw, actual) = compare(
        input,
        artifact,
        host(input, invalid),
        &ExecutionLimits::default(),
    );
    assert!(matches!(
        raw.unwrap().outcome,
        ExecutionOutcome::Exit {
            kind: ExitKind::Revert,
            ..
        }
    ));
    assert!(
        !actual.requests.iter().any(|r| r.opcode == 0x54),
        "the source ABI u64 guard must reject before entering the decoder"
    );
    println!(
        "concrete supplied decoder: {} ABI cases, 3 memory faults, 2 step budgets, source u64 guard",
        cases.len()
    );
}
