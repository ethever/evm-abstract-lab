//! Whole-fixture acceptance is distinct from executing isolated constructors.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionLimits, ExecutionOutcome, ExitKind, ExternalResponse,
    Fork, Input, LiftedGuardPayload, Limits, MemoryHost, StageKind, U256, ValueFact, decompile,
    execute_bytecode, execute_ir, execute_structured,
};
use std::sync::OnceLock;

#[path = "lifted/oracle/expressions.rs"]
mod expressions;
use expressions as oracle;
#[path = "lifted/error_payloads/oracle.rs"]
mod payload_oracle;
#[path = "shared_procedures/oracle.rs"]
mod shared_oracle;
#[path = "shared_procedures/router.rs"]
mod shared_router;
#[path = "shared_procedures/support.rs"]
mod support;

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
        if let Some(directory) = std::env::var_os("DECOMPILE_EVIDENCE_DIR") {
            let path = std::path::Path::new(&directory).join("router02.json");
            let file = std::fs::File::create(path).unwrap();
            let mut writer = std::io::BufWriter::new(file);
            serde_json::to_writer(&mut writer, &artifact).unwrap();
            std::io::Write::flush(&mut writer).unwrap();
        }
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
fn supplied_dynamic_pointer_source_has_terminal_and_original_construction_certificates() {
    let (_, artifact) = router();
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    let program = stage.ir.lifted.as_ref().unwrap();
    // The original v115898 at base 475cd9ad belongs to this source site.
    // Recovery improvements can allocate different contextual entity IDs.
    // Keep the actual contract: certify dynamic pointers at the same bytecode
    // constructor, while the raw scalar summary is still Unknown.
    let index = program.render_index(&stage.ir);
    index.validate_guard_sources().unwrap();
    let blocks: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .filter(|block| {
            block.source.is_some_and(|source| source.pc == 21_011)
                && block
                    .operations
                    .iter()
                    .find(|op| op.opcode == 0x51)
                    .is_some_and(|op| {
                        stage.ir.value(op.outputs[0]).unwrap().fact == ValueFact::Unknown
                    })
                && index.revert(block.id).is_some()
        })
        .collect();
    assert!(
        !blocks.is_empty(),
        "retain an Unknown-pointer source witness"
    );
    for block in blocks {
        let supplied = index.revert(block.id).unwrap();
        assert_eq!(
            index.payload(supplied.payload).unwrap().message,
            "ds-math-mul-overflow"
        );
        assert_eq!(index.construction(block.id).unwrap().revert, supplied.id);
    }
}

fn assert_error_source_coverage(artifact: &Artifact) {
    use std::collections::BTreeSet;
    let ir = &artifact.current().unwrap().ir;
    let program = ir.lifted.as_ref().unwrap();
    let index = program.render_index(ir);
    // All 26 previously certified bytecode sites must remain covered. A raw
    // total of 129 contexts counted four redundant EXPIRED continuations at
    // PC 11634; path refinement now proves those additional copies unreachable.
    let expected = BTreeSet::from([
        3569, 3928, 4188, 5348, 5749, 6009, 6664, 7585, 7845, 8465, 8692, 9534, 9902, 10380, 10607,
        11634, 12839, 13238, 14047, 14274, 16237, 17941, 19253, 20101, 21011, 21125,
    ]);
    let actual: BTreeSet<_> = program
        .reverts
        .iter()
        .map(|proof| ir.block(proof.block).unwrap().source.unwrap().pc)
        .collect();
    assert!(
        expected.is_subset(&actual),
        "lost error source coverage: {actual:?}"
    );
    for proof in &program.reverts {
        assert_eq!(index.construction(proof.block).unwrap().revert, proof.id);
    }
    for block in ir.blocks.iter().filter(|block| {
        block.source.is_some_and(|source| {
            expected.contains(&source.pc) && ![19253, 20101, 21011, 21125].contains(&source.pc)
        })
    }) {
        assert!(
            index.revert(block.id).is_some(),
            "lost previously total source coverage at {:?}",
            block.source
        );
    }
}

#[test]
fn actual_router_preserves_baseline_coverage_and_raw_witness() {
    let (_, artifact) = router();
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = program.render_index(&stage.ir);
    index.validate_guard_sources().unwrap();
    assert_error_source_coverage(artifact);
    assert!(
        program.payloads.len() >= 7,
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
        ds_proofs.len() >= 35,
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

#[test]
fn deadline_refinement_preserves_real_expired_calls_and_error_bytes() {
    let (input, artifact) = router();
    let cases = [
        (
            "addLiquidity(address,address,uint256,uint256,uint256,uint256,address,uint256)",
            vec![1_u64, 2, 10, 10, 0, 0, 1, 99],
        ),
        (
            "removeLiquidityETH(address,uint256,uint256,uint256,address,uint256)",
            vec![1_u64, 10, 0, 0, 1, 99],
        ),
    ];
    for (signature, words) in cases {
        let mut calldata =
            alloy_primitives::keccak256(signature.as_bytes()).as_slice()[..4].to_vec();
        for word in words {
            calldata.extend(U256::from(word).to_be_bytes::<32>());
        }
        let mut host = MemoryHost::new(input, calldata);
        host.context.insert(0x42, U256::from(100));
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
            assert_eq!(raw.outcome, graph.outcome, "{signature}");
            assert_eq!(raw.observations(), graph.observations(), "{signature}");
            assert_eq!(raw_host, graph_host, "{signature}");
        }
        let ExecutionOutcome::Exit {
            kind: ExitKind::Revert,
            values,
        } = &raw.outcome
        else {
            panic!("expired {signature} did not revert: {:?}", raw.outcome);
        };
        let message = b"UniswapV2Router: EXPIRED";
        let offset = usize::try_from(values[0]).unwrap();
        let size = usize::try_from(values[1]).unwrap();
        assert_eq!(size, 100);
        let mut expected = vec![0_u8; 100];
        expected[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
        expected[35] = 32;
        expected[67] = u8::try_from(message.len()).unwrap();
        expected[68..68 + message.len()].copy_from_slice(message);
        assert_eq!(&raw_host.memory[offset..offset + size], expected.as_slice());
        let (normalized, normalized_host, capsules) =
            payload_oracle::certify(input, artifact, host, limits.max_memory_bytes);
        assert_eq!(normalized.unwrap(), raw.outcome);
        assert_eq!(normalized_host, raw_host);
        assert_eq!(capsules, 1, "{signature}");
    }
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
fn actual_liquidity_errors_keep_codecopy_and_prove_padding_after_loop_allocations() {
    let (input, artifact) = router();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = program.render_index(&stage.ir);
    let message = b"UniswapV2Library: INSUFFICIENT_LIQUIDITY";
    let expected = payload_oracle::abi(message);
    assert_eq!(expected.len(), 132);
    assert!(expected[108..].iter().all(|byte| *byte == 0));
    for signature in [
        "getAmountsIn(uint256,address[])",
        "getAmountsOut(uint256,address[])",
    ] {
        for length in [2, 3, 5] {
            for failed_call in [0, length - 2] {
                for response_size in [96, 320] {
                    let mut host = array_host(input, signature, U256::from(100), length, 1_000_000);
                    for response in &mut host.responses {
                        response.return_data.resize(response_size, 0xa5);
                    }
                    host.responses[failed_call].return_data[..32].fill(0);
                    let limits = ExecutionLimits {
                        max_steps: 25_000,
                        max_memory_bytes: 1_048_576,
                    };
                    let mut graph_host = host.clone();
                    let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
                    let terminal = graph
                        .trace
                        .iter()
                        .rev()
                        .find(|step| step.opcode == 0xfd)
                        .unwrap();
                    let proof = program
                        .reverts
                        .iter()
                        .find(|r| Some(r.operation) == terminal.operation)
                        .expect(
                            "the executed CODECOPY liquidity error needs a complete byte proof",
                        );
                    assert_eq!(
                        proof.code_hash,
                        Some(alloy_primitives::keccak256(&input.runtime_bytecode))
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
                    let plan = index.construction(proof.block).unwrap();
                    let body = program.block(proof.block).unwrap();
                    assert!(
                        body.statements[plan.start..plan.end]
                            .iter()
                            .any(|statement| {
                                stage
                                    .ir
                                    .block(proof.block)
                                    .unwrap()
                                    .operations
                                    .iter()
                                    .any(|op| op.id == statement.operation && op.opcode == 0x39)
                            })
                    );
                    let (outcome, state, capsules) =
                        payload_oracle::certify(input, artifact, host, limits.max_memory_bytes);
                    assert_eq!(
                        payload_oracle::reverted(&outcome.unwrap(), &state),
                        Some(expected.clone())
                    );
                    assert_eq!(capsules, 1);
                    assert_eq!(state.responses.len(), length - 2 - failed_call);
                }
            }
        }
    }
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
    let shared = shared_oracle::certify(
        input,
        artifact,
        array_host(input, signature, amount, 3, 1000),
        &ExecutionLimits::default(),
    );
    assert!(
        shared.calls.len() >= 2,
        "actual Router array path must execute shared definitions repeatedly"
    );
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

#[test]
fn router_hybrid_guards_cover_certified_sites_and_preserve_explicit_source_nesting() {
    use evm_decompile::{StructuredKind, Terminator};
    use std::collections::{BTreeMap, BTreeSet};
    let (_, artifact) = router();
    let ir = &artifact.current().unwrap().ir;
    let high = ir.lifted.as_ref().unwrap();
    // Definitions expand canonical bodies; remove only call substitution for
    // this source-occurrence coverage check, keeping every semantic certificate.
    let mut expanded = high.clone();
    expanded.invocations.clear();
    expanded.procedures.clear();
    let index = expanded.render_index(ir);
    index.validate_failure_first_sources().unwrap();
    let boundary = ir.structured.as_ref().unwrap().root.id;
    let mut source_controls = BTreeMap::new();
    let mut source_ifs = BTreeSet::new();
    let mut pending = vec![&ir.structured.as_ref().unwrap().root];
    while let Some(node) = pending.pop() {
        match &node.kind {
            StructuredKind::Sequence(v) => pending.extend(v),
            StructuredKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                source_controls.insert(node.id, *condition);
                source_ifs.insert(node.id);
                pending.extend([&**then_branch, &**else_branch]);
            }
            StructuredKind::While {
                header,
                condition,
                body,
                ..
            } => {
                if let Some(condition) = condition {
                    source_controls.insert(node.id, *condition);
                }
                pending.extend([&**header, &**body]);
            }
            StructuredKind::Indirect { arms, .. } => pending.extend(arms.iter().map(|a| &a.body)),
            StructuredKind::Dispatcher { .. } => {
                panic!("this reducible Router fixture must retain natural regions")
            }
            _ => {}
        }
    }
    assert_eq!(
        source_controls.keys().copied().collect::<BTreeSet<_>>(),
        expanded.controls.iter().map(|c| c.node).collect(),
        "every actual source If/conditional While has exactly its semantic binding"
    );
    let blocks: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
    let edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
    let expressions: BTreeMap<_, _> = expanded.expressions.iter().map(|e| (e.id, e)).collect();
    let failures: BTreeSet<_> = expanded.failures.iter().map(|f| f.block).collect();
    let mut expected_sources = BTreeSet::new();
    let mut actual_sources = BTreeSet::new();
    let mut sites = 0;
    let mut requires = 0;
    let mut explicit = 0;
    for control in &expanded.controls {
        let block = blocks[&expressions[&control.condition].block];
        if source_ifs.contains(&control.node)
            && let Terminator::Branch {
                condition,
                then_edges,
                else_edge: Some(other),
                unresolved: false,
                invalid_targets,
                ..
            } = &block.terminator
        {
            assert_eq!(source_controls[&control.node], *condition);
            if then_edges.len() == 1
                && invalid_targets.is_empty()
                && (failures.contains(&edges[&then_edges[0]].to)
                    ^ failures.contains(&edges[other].to))
            {
                expected_sources.insert((block.id, then_edges[0], *other));
                assert!(
                    control.guard.is_some(),
                    "qualified original branch {:?} at PC {:?} lost its source guard",
                    block.id,
                    block.source.map(|s| s.pc)
                );
            }
        }
        match index.if_plan(control.node, boundary) {
            evm_decompile::LiftedIfPlan::Require(_) => requires += 1,
            evm_decompile::LiftedIfPlan::FailureFirst(_) => explicit += 1,
            evm_decompile::LiftedIfPlan::Original => {}
        }
        if let Some(guard) = &control.guard {
            sites += 1;
            actual_sources.insert((
                guard.block,
                if guard.pass_nonzero {
                    guard.pass_edge
                } else {
                    guard.failure_edge
                },
                if guard.pass_nonzero {
                    guard.failure_edge
                } else {
                    guard.pass_edge
                },
            ));
            assert!(
                !matches!(
                    index.if_plan(control.node, boundary),
                    evm_decompile::LiftedIfPlan::Original
                ),
                "certified source guard {:?} lost its selected projection",
                control.node
            );
        }
    }
    // Count original branch witnesses, not repeated occurrences of a tail.
    // Eliminating redundant AST copies must not look like lost source coverage.
    assert!(!expected_sources.is_empty());
    assert_eq!(actual_sources, expected_sources);
    let expected_pcs: BTreeSet<_> = expected_sources
        .iter()
        .map(|(block, _, _)| blocks[block].source.unwrap().pc)
        .collect();
    let actual_pcs: BTreeSet<_> = expanded
        .controls
        .iter()
        .filter_map(|c| c.guard.as_ref())
        .map(|g| blocks[&g.block].source.unwrap().pc)
        .collect();
    assert_eq!(actual_pcs, expected_pcs);
    eprintln!(
        "Router guard coverage: {sites} occurrences, {} original branch witnesses, {} original PCs",
        expected_sources.len(),
        expected_pcs.len()
    );
    let text = evm_decompile::render_text(artifact);
    assert!(
        requires > 0 && explicit > 0,
        "selected sites: require={requires}, explicit={explicit}"
    );
    assert!(text.contains("require(") && text.contains("evm.invalid();"));
    let depth = |text: &str| {
        text.lines()
            .map(|line| (line.len() - line.trim_start().len()) / 4)
            .max()
            .unwrap()
    };
    // The old absolute ceiling depended on hiding CALL and REVERT inside a
    // large shared helper. The optional-return bool decoder must now remain
    // visible in its caller; its two nested value-producing branches are not
    // failed-else continuations. Keep the source guard/plan checks above and
    // independently require outlining not to add nesting to that same fully
    // expanded, certified projection. Do not count hiding terminals as flattening.
    let mut source_artifact = artifact.clone();
    let source_high = source_artifact
        .stages
        .last_mut()
        .unwrap()
        .ir
        .lifted
        .as_mut()
        .unwrap();
    source_high.procedures.clear();
    source_high.invocations.clear();
    let source_text = evm_decompile::render_text(&source_artifact);
    let shared_depth = depth(&text);
    let source_depth = depth(&source_text);
    assert!(
        shared_depth <= source_depth,
        "outlining added control nesting to the explicit source: {source_depth} -> {shared_depth}"
    );
    assert!(
        text.contains("evm.mload(")
            && text.contains("evm.mstore(")
            && text.contains("evm.revert(offset = ")
    );
}
