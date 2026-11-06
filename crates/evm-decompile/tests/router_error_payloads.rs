//! Whole-fixture acceptance is distinct from executing isolated constructors.
use evm_decompile::{
    Completion, Config, EntityId, ExecutionLimits, ExecutionOutcome, ExitKind, ExternalResponse,
    Fork, Input, LiftedGuardPayload, Limits, MemoryHost, StageKind, U256, ValueFact, decompile,
    execute_bytecode, execute_ir, execute_structured,
};

#[test]
fn actual_router_certifies_real_payloads_and_preserves_unproved_unknown_pointer_paths() {
    // A byte-identical, hash-pinned test input keeps this package's Nix source
    // closure independent of Web implementation files and dependency selection.
    let hex = include_str!("fixtures/error-payloads/ethereum-uniswap-v2-router02.hex").trim();
    let bytes = evm_bytecode::parse_hex(hex).unwrap();
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
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
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
    let supplied = program.reverts.iter().find(|r| r.block == block.id);
    if let Some(proof) = supplied {
        assert_eq!(
            index.payload(proof.payload).unwrap().message,
            "ds-math-mul-overflow"
        );
    } else {
        // Relational loop/allocation facts are separate from a local byte
        // proof. An unproved real source route keeps its original operations.
        assert!(index.construction(block.id).is_none());
        assert!(!program.failures.iter().any(|f| f.block == block.id));
        let body = program.block(block.id).unwrap();
        for operation in block
            .operations
            .iter()
            .filter(|o| matches!(o.opcode, 0x51 | 0x52 | 0xfd))
        {
            assert!(body.statements.iter().any(|s| s.operation == operation.id));
        }
    }
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
            "Router02 source {pc}: {} contexts, {certified} certified, {unknown} Unknown-pointer contexts",
            targets.len()
        );
    }
    eprintln!(
        "Router02 coverage: {} payloads, {} terminal proofs, {} construction capsules; supplied block {:?} certified={}",
        program.payloads.len(),
        program.reverts.len(),
        program.constructions.len(),
        block.id,
        supplied.is_some()
    );
    concrete_overflow(&input, stage, true);
    concrete_overflow(&input, stage, false);
}

fn concrete_overflow(input: &Input, stage: &evm_decompile::Stage, scalar: bool) {
    let (signature, arguments) = if scalar {
        (
            "getAmountOut(uint256,uint256,uint256)",
            vec![U256::MAX, U256::from(1), U256::from(1)],
        )
    } else {
        (
            "getAmountsOut(uint256,address[])",
            vec![
                U256::MAX,
                U256::from(64),
                U256::from(2),
                U256::from(0x1111),
                U256::from(0x2222),
            ],
        )
    };
    let mut calldata = alloy_primitives::keccak256(signature.as_bytes()).as_slice()[..4].to_vec();
    for word in arguments {
        calldata.extend(word.to_be_bytes::<32>());
    }
    let mut host = MemoryHost::new(input, calldata);
    host.context.insert(0x3b, U256::from(1));
    let return_data = [U256::from(1), U256::from(1), U256::ZERO]
        .into_iter()
        .flat_map(|word| word.to_be_bytes::<32>())
        .collect();
    if !scalar {
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data,
            ..ExternalResponse::default()
        });
    }
    let limits = ExecutionLimits {
        max_steps: 25_000,
        max_memory_bytes: 1_048_576,
    };
    let mut raw_host = host.clone();
    let raw = execute_bytecode(input, &mut raw_host, &limits).unwrap();
    let mut graph_host = host.clone();
    let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut syntax_host = host;
    let syntax = execute_structured(stage, &mut syntax_host, &limits).unwrap();
    assert_eq!(raw.outcome, graph.outcome);
    assert_eq!(raw.outcome, syntax.outcome);
    assert_eq!(raw.observations(), graph.observations());
    assert_eq!(raw.observations(), syntax.observations());
    assert_eq!(raw_host, graph_host);
    assert_eq!(raw_host, syntax_host);
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
    if let Some(proof) = proof {
        assert_eq!(
            program
                .payloads
                .iter()
                .find(|p| p.id == proof.payload)
                .unwrap()
                .bytes,
            expected
        );
    } else {
        assert!(
            program
                .blocks
                .iter()
                .flat_map(|b| &b.statements)
                .any(|s| Some(s.operation) == terminal.operation)
        );
    }
    if scalar {
        let proof = proof
            .expect("the actual public scalar overflow must have a terminal payload certificate");
        let index = program.render_index(&stage.ir);
        let capsule = index.construction(proof.block)
            .expect("the executed public scalar overflow must have its original lazy construction certificate");
        assert_eq!(capsule.revert, proof.id);
        assert_eq!(offset, 128);
    }
    eprintln!(
        "Concrete {signature} overflow: {} raw steps, offset {offset}, size {size}, certified={}",
        raw.steps,
        proof.is_some()
    );
}
