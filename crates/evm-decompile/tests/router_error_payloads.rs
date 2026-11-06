//! Whole-fixture acceptance is distinct from executing isolated constructors.
use evm_decompile::{
    Artifact, Completion, Config, EntityId, ExecutionLimits, ExecutionOutcome, ExitKind,
    ExternalResponse, Fork, Input, LiftedGuardPayload, Limits, MemoryHost, StageKind, U256,
    ValueFact, decompile, execute_bytecode, execute_ir, execute_structured,
};
use std::sync::OnceLock;

#[path = "lifted/oracle/expressions.rs"]
mod oracle;
#[path = "lifted/error_payloads/oracle.rs"]
mod payload_oracle;

fn router() -> &'static (Input, Artifact) {
    static ROUTER: OnceLock<(Input, Artifact)> = OnceLock::new();
    let result = ROUTER.get_or_init(|| {
        // A byte-identical, hash-pinned test input keeps this package's Nix source
        // closure independent of Web implementation files and dependency selection.
        let hex = include_str!("fixtures/error-payloads/ethereum-uniswap-v2-router02.hex").trim();
        let bytes = evm_bytecode::parse_hex(hex).unwrap();
        assert_eq!(bytes.len(), 21_943);
        assert_eq!(
            format!("{:#x}", alloy_primitives::keccak256(&bytes)),
            "0xa324bc7db3d091b6f1a2d526e48a9c7039e03b3cc35f7d44b15ac7a1544c11d2"
        );
        // This is a bounded real-case diagnostic, not the CLI's maximum policy.
        let config = Config {
            limits: Limits {
                max_work: 200_000_000_000,
                max_history_entities: 40_000_000,
                ..Limits::default()
            },
            ..Config::default()
        };
        let input = Input::new(bytes, Fork::Osaka);
        let artifact = decompile(&input, &config);
        (input, artifact)
    });
    assert_eq!(
        result.1.completion,
        Completion::Complete,
        "{:?}",
        result.1.diagnostics
    );
    result
}

#[test]
fn exact_supplied_v115898_has_terminal_and_original_construction_certificates() {
    let (_, artifact) = router();
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    let program = stage.ir.lifted.as_ref().unwrap();
    // This is the supplied raw SSA occurrence, retained in the original witness
    // at base 475cd9ad. Its memory contents require proof beyond ValueFact.
    let supplied_pointer = EntityId(115898);
    assert_eq!(
        stage.ir.value(supplied_pointer).unwrap().fact,
        ValueFact::Unknown
    );
    let block = stage
        .ir
        .blocks
        .iter()
        .find(|block| {
            block
                .operations
                .iter()
                .any(|op| op.opcode == 0x51 && op.outputs.contains(&supplied_pointer))
        })
        .unwrap();
    assert_eq!(block.source.unwrap().pc, 21_011);
    let index = program.render_index(&stage.ir);
    index.validate_guard_sources().unwrap();
    let supplied = index
        .revert(block.id)
        .expect("the exact supplied dynamic-loop pointer v115898 requires its terminal proof");
    assert_eq!(
        index.payload(supplied.payload).unwrap().message,
        "ds-math-mul-overflow"
    );
    assert_eq!(
        index
            .construction(block.id)
            .expect("v115898 requires its original lazy construction capsule")
            .revert,
        supplied.id
    );
}

#[test]
fn actual_router_preserves_baseline_coverage_and_raw_witness() {
    let (_, artifact) = router();
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = program.render_index(&stage.ir);
    index.validate_guard_sources().unwrap();
    assert!(
        program.reverts.len() >= 65,
        "retain baseline terminal coverage"
    );
    assert!(
        program.constructions.len() >= 65,
        "retain baseline capsule coverage"
    );
    assert!(
        program.payloads.len() >= 6,
        "retain baseline byte identities"
    );
    let ds_math = program
        .payloads
        .iter()
        .find(|p| p.message == "ds-math-mul-overflow")
        .expect("whole Router02 must have positive constant-MSTORE Error coverage");
    assert_eq!(ds_math.bytes.len(), 100);
    assert_eq!(&ds_math.bytes[88..], &[0_u8; 12]);
    let ds_proofs: Vec<_> = program
        .reverts
        .iter()
        .filter(|r| r.payload == ds_math.id)
        .collect();
    assert!(
        ds_proofs.len() >= 11,
        "retain independently measured real-case positive coverage"
    );
    for proof in &ds_proofs {
        let source = stage.ir.block(proof.block).unwrap();
        assert_eq!(source.source.unwrap().pc, 21_011);
        assert_eq!(source.operations.last().unwrap().source.unwrap().pc, 21_112);
    }
    assert!(
        program.controls.iter().any(|control| control
            .guard
            .as_ref()
            .is_some_and(|guard| guard.payload == LiftedGuardPayload::ErrorString(ds_math.id))),
        "actual Router02 must have certified nonempty require integration"
    );
    let mut witness = stage.ir.clone();
    witness.lifted = None;
    assert_eq!(
        witness,
        artifact
            .stages
            .iter()
            .find(|s| s.kind == StageKind::Structured)
            .unwrap()
            .ir
    );
    for pc in [21_011, 15_506] {
        let targets: Vec<_> = stage
            .ir
            .blocks
            .iter()
            .filter(|b| b.source.is_some_and(|s| s.pc == pc))
            .collect();
        let certified = targets
            .iter()
            .filter(|b| index.revert(b.id).is_some())
            .count();
        let unknown = targets
            .iter()
            .filter(|b| {
                b.operations
                    .iter()
                    .find(|o| o.opcode == 0x51)
                    .is_some_and(|o| {
                        stage.ir.value(o.outputs[0]).unwrap().fact == ValueFact::Unknown
                    })
            })
            .count();
        eprintln!(
            "Router02 source {pc}: {} contexts, {certified} certified, {} uncertified, {unknown} raw-ValueFact Unknown-pointer contexts",
            targets.len(),
            targets.len() - certified
        );
    }
    eprintln!(
        "Router02 coverage: {} payloads, {} terminal proofs, {} construction capsules",
        program.payloads.len(),
        program.reverts.len(),
        program.constructions.len()
    );
}

#[test]
fn actual_get_amount_out_max_has_terminal_and_original_construction_certificates() {
    let (input, artifact) = router();
    concrete_overflow(input, artifact, Route::Scalar);
}

#[test]
fn actual_get_amounts_out_max_path2_has_terminal_and_original_construction_certificates() {
    let (input, artifact) = router();
    concrete_overflow(input, artifact, Route::ArrayOut(2));
}

#[test]
fn actual_get_amounts_out_max_path3_has_terminal_and_original_construction_certificates() {
    let (input, artifact) = router();
    concrete_overflow(input, artifact, Route::ArrayOut(3));
}

#[test]
fn actual_get_amounts_in_max_path3_has_terminal_and_original_construction_certificates() {
    let (input, artifact) = router();
    concrete_overflow(input, artifact, Route::ArrayIn(3));
}

enum Route {
    Scalar,
    ArrayOut(usize),
    ArrayIn(usize),
}

fn concrete_overflow(input: &Input, artifact: &Artifact, route: Route) {
    let stage = artifact.current().unwrap();
    let (signature, n) = match route {
        Route::Scalar => ("getAmountOut(uint256,uint256,uint256)", 2),
        Route::ArrayOut(n) => ("getAmountsOut(uint256,address[])", n),
        Route::ArrayIn(n) => ("getAmountsIn(uint256,address[])", n),
    };
    let scalar = matches!(route, Route::Scalar);
    let mut host = if scalar {
        let mut calldata = alloy_primitives::keccak256("getAmountOut(uint256,uint256,uint256)")
            .as_slice()[..4]
            .to_vec();
        for word in [U256::MAX, U256::from(1), U256::from(1)] {
            calldata.extend(word.to_be_bytes::<32>());
        }
        MemoryHost::new(input, calldata)
    } else {
        array_host(input, signature, U256::MAX, n, 1)
    };
    host.context.insert(0x3b, U256::from(1));
    host.context.insert(0x5a, U256::from(10_000_000));
    let limits = ExecutionLimits {
        max_steps: 25_000,
        max_memory_bytes: 1_048_576,
    };
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    let mut graph_host = host.clone();
    let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut syntax_host = host.clone();
    let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
    assert_eq!(raw.outcome, graph.outcome);
    assert_eq!(raw.outcome, syntax.outcome);
    assert_eq!(raw.observations(), graph.observations());
    assert_eq!(raw.observations(), syntax.observations());
    assert_eq!(raw_host, graph_host);
    assert_eq!(raw_host, syntax_host);
    let (normalized, normalized_host, capsules) =
        payload_oracle::certify(input, artifact, host, limits.max_memory_bytes);
    assert_eq!(normalized.unwrap(), raw.outcome);
    assert_eq!(normalized_host, raw_host);
    let ExecutionOutcome::Exit {
        kind: ExitKind::Revert,
        values,
    } = &raw.outcome
    else {
        panic!(
            "actual overflow call did not reach REVERT: {:?}",
            raw.outcome
        )
    };
    let offset = usize::try_from(values[0]).unwrap();
    let size = usize::try_from(values[1]).unwrap();
    let mut expected = vec![0_u8; 100];
    expected[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
    expected[35] = 32;
    expected[67] = 20;
    expected[68..88].copy_from_slice(b"ds-math-mul-overflow");
    assert_eq!(size, expected.len());
    assert_eq!(&raw_host.memory[offset..offset + size], expected.as_slice());
    let terminal = graph
        .trace
        .iter()
        .rev()
        .find(|op| op.opcode == 0xfd)
        .unwrap();
    assert_eq!(terminal.pc, Some(21_112));
    let program = stage.ir.lifted.as_ref().unwrap();
    let proof = program
        .reverts
        .iter()
        .find(|r| Some(r.operation) == terminal.operation);
    eprintln!(
        "Concrete {signature} path{n}: {} raw steps, pointer {offset:#x}, terminal {:?}, certified={}",
        raw.steps,
        terminal.operation,
        proof.is_some()
    );
    let proof =
        proof.expect("the actually executed overflow terminal requires a payload certificate");
    assert_eq!(
        capsules, 1,
        "the normalized path must execute the original capsule once"
    );
    assert_eq!(
        program
            .payloads
            .iter()
            .find(|p| p.id == proof.payload)
            .unwrap()
            .bytes,
        expected
    );
    let index = program.render_index(&stage.ir);
    let capsule = index.construction(proof.block).expect(
        "the actually executed overflow requires its original lazy construction certificate",
    );
    assert_eq!(capsule.revert, proof.id);
    if scalar {
        assert_eq!(offset, 128);
    } else {
        assert_eq!(offset, 0xc0 + 64 * n + 0xbd);
    }
    eprintln!(
        "Concrete {signature} path{n} overflow: {} raw steps, offset {offset:#x}, size {size}, certified=true",
        raw.steps,
    );
}

fn array_host(input: &Input, signature: &str, amount: U256, n: usize, reserve: u64) -> MemoryHost {
    let mut calldata = alloy_primitives::keccak256(signature.as_bytes()).as_slice()[..4].to_vec();
    for word in [amount, U256::from(64), U256::from(n)]
        .into_iter()
        .chain((1..=n).map(|i| U256::from(i * 0x1111)))
    {
        calldata.extend(word.to_be_bytes::<32>());
    }
    let mut host = MemoryHost::new(input, calldata);
    host.context.insert(0x3b, U256::from(1));
    host.context.insert(0x5a, U256::from(10_000_000));
    for _ in 1..n {
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: [U256::from(reserve), U256::from(reserve), U256::ZERO]
                .into_iter()
                .flat_map(|word| word.to_be_bytes::<32>())
                .collect(),
            ..ExternalResponse::default()
        });
    }
    host
}

#[test]
fn actual_path3_success_executes_two_pointer_epochs_and_returns_100_90_82() {
    let (input, artifact) = router();
    array_success(
        input,
        artifact,
        "getAmountsOut(uint256,address[])",
        U256::from(100),
    );
}

#[test]
fn actual_reverse_path3_success_executes_modular_decrement_and_returns_100_90_82() {
    let (input, artifact) = router();
    array_success(
        input,
        artifact,
        "getAmountsIn(uint256,address[])",
        U256::from(82),
    );
}

fn array_success(input: &Input, artifact: &Artifact, signature: &str, amount: U256) {
    let host = array_host(input, signature, amount, 3, 1000);
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &ExecutionLimits::default()).unwrap();
    let ExecutionOutcome::Exit {
        kind: ExitKind::Return,
        values,
    } = &raw.outcome
    else {
        panic!("expected successful path3 result: {:?}", raw.outcome)
    };
    let offset = usize::try_from(values[0]).unwrap();
    let size = usize::try_from(values[1]).unwrap();
    let (words, remainder) = raw_host.memory[offset..offset + size].as_chunks::<32>();
    assert!(
        remainder.is_empty(),
        "array return must contain complete ABI words"
    );
    let actual: Vec<_> = words
        .iter()
        .map(|word| U256::from_be_bytes(*word))
        .collect();
    assert_eq!(actual, [32_u64, 3, 100, 90, 82].map(U256::from));
    let allocations: Vec<_> = raw
        .trace
        .iter()
        .filter(|op| op.opcode == 0x52 && op.inputs[0] == U256::from(64))
        .map(|op| op.inputs[1])
        .collect();
    let start = allocations
        .iter()
        .position(|value| *value == U256::from(0x180))
        .unwrap();
    assert_eq!(
        &allocations[start..start + 5],
        [0x180_u64, 0x1c8, 0x23d, 0x285, 0x2fa].map(U256::from),
        "each pair of writes must use its same saved iteration-start pointer"
    );
    let outputs: Vec<_> = raw
        .trace
        .iter()
        .filter(|op| op.opcode == 0xfa)
        .map(|op| (op.inputs[4], op.inputs[5]))
        .collect();
    assert_eq!(
        outputs,
        vec![
            (U256::from(0x23d), U256::from(96)),
            (U256::from(0x2fa), U256::from(96))
        ]
    );
    assert!(raw_host.responses.is_empty());
    let (normalized, normalized_host, capsules) =
        payload_oracle::certify(input, artifact, host, 1_048_576);
    assert_eq!(normalized.unwrap(), raw.outcome);
    assert_eq!(normalized_host, raw_host);
    assert_eq!(capsules, 0);
}

#[test]
fn actual_array_allocation_and_call_output_fault_prefixes_match_normalized_execution() {
    let (input, artifact) = router();
    for memory_limit in [63, 0x180, 0x29c] {
        let (outcome, _, _) = payload_oracle::certify(
            input,
            artifact,
            array_host(
                input,
                "getAmountsOut(uint256,address[])",
                U256::from(100),
                3,
                1000,
            ),
            memory_limit,
        );
        assert_eq!(outcome, Err(evm_decompile::ExecutionError::MemoryLimit));
    }
}
