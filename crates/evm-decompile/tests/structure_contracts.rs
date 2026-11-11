//! Fixed real runtimes exercise planning size and independent execution paths.
use evm_decompile::{
    Artifact, Cancellation, Completion, Config, ExecutionLimits, ExecutionOutcome, ExitKind, Fork,
    Input, Limits, MemoryHost, StageKind, StructuredKind, StructuredNode, TextExportStatus, U256,
    ValidationStatus, decompile, execute_bytecode, execute_ir, execute_structured, export_text,
};
use std::sync::OnceLock;

struct Fixture {
    input: Input,
    artifact: Artifact,
}

fn fixture(hex: &str, metadata: &str) -> Fixture {
    let metadata: serde_json::Value = serde_json::from_str(metadata).unwrap();
    let bytes = evm_bytecode::parse_hex(hex).unwrap();
    assert_eq!(
        bytes.len() as u64,
        metadata["byte_length"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:#x}", alloy_primitives::keccak256(&bytes)),
        metadata["code_hash"].as_str().unwrap()
    );
    let input = Input::new(bytes, Fork::Osaka);
    // Match the finite whole-Router acceptance envelope. In addition to final
    // size, this catches repeated planning after local rollback; a regression
    // must stop with evidence rather than consume the default u64::MAX budget.
    // Release CLI/browser acceptance also checks the ordinary default policy.
    let config = Config {
        limits: Limits {
            max_work: 200_000_000_000,
            max_history_entities: 40_000_000,
            ..Limits::default()
        },
        ..Config::default()
    };
    let artifact = decompile(&input, &config);
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
            .all(|s| s.validation.status == ValidationStatus::Passed)
    );
    assert!(
        artifact.diagnostics.iter().all(|diagnostic| {
            !diagnostic.message.contains("source-control verification")
                && !diagnostic.message.contains("whole-source dispatcher")
        }),
        "real-contract structure must not rely on verification fallback: {:?}",
        artifact.diagnostics
    );
    Fixture { input, artifact }
}

fn pair() -> &'static Fixture {
    static PAIR: OnceLock<Fixture> = OnceLock::new();
    PAIR.get_or_init(|| {
        fixture(
            include_str!("fixtures/structure/ethereum-uniswap-v2-usdc-weth-pair.hex"),
            include_str!("fixtures/structure/ethereum-uniswap-v2-usdc-weth-pair.json"),
        )
    })
}

fn pool_manager() -> &'static Fixture {
    static MANAGER: OnceLock<Fixture> = OnceLock::new();
    MANAGER.get_or_init(|| {
        fixture(
            include_str!("fixtures/structure/ethereum-uniswap-v4-pool-manager.hex"),
            include_str!("fixtures/structure/ethereum-uniswap-v4-pool-manager.json"),
        )
    })
}

#[test]
fn pool_manager_high_projection_omits_empty_then_else_pairs() {
    let fixture = pool_manager();
    let export = export_text(&fixture.artifact, &Cancellation::default());
    assert_eq!(export.status, TextExportStatus::Complete);
    let rows: Vec<_> = export.text.lines().collect();
    let mut checked = 0;
    for (index, row) in rows.iter().enumerate() {
        if !row.trim_start().starts_with("if (") || !row.ends_with('{') {
            continue;
        }
        checked += 1;
        let next = rows[index + 1..]
            .iter()
            .find(|row| !row.trim().is_empty() && !row.trim_start().starts_with("//"));
        assert!(
            next.is_none_or(|row| row.trim() != "} else {"),
            "empty then retained at line {}: {}",
            index + 1,
            row,
        );
    }
    assert!(
        checked > 100,
        "exercise the real contract, not an empty/refused export"
    );
    assert!(export.text.contains("evm.tstore("));
    assert!(export.text.contains("evm.revert("));
}

#[test]
fn pool_manager_recovers_custom_words_and_panic_without_requiring_string_abi() {
    use evm_decompile::{LiftedByteSegment, LiftedGuardPayload, LiftedRevertDataKind};
    let artifact = &pool_manager().artifact;
    let ir = &artifact.current().unwrap().ir;
    let program = ir.lifted.as_ref().unwrap();
    for pc in [7714, 7758, 7857] {
        let proofs: Vec<_> = program
            .revert_data
            .iter()
            .filter(|data| {
                ir.block(data.block)
                    .unwrap()
                    .source
                    .is_some_and(|source| source.pc == pc)
            })
            .collect();
        assert!(!proofs.is_empty(), "missing recovered terminal site {pc}");
        assert!(
            proofs
                .iter()
                .all(|data| matches!(data.content, LiftedRevertDataKind::Segments(_))),
            "constant-address selector/word constructor remained opaque at {pc}"
        );
        if pc != 7857 {
            assert!(
                proofs.iter().any(|data| data
                    .content
                    .segments()
                    .iter()
                    .any(|segment| matches!(segment, LiftedByteSegment::Word { size: 32, .. }))),
                "lost full symbolic argument at {pc}"
            );
        }
    }
    // Failure to establish a pointer's nonwrapping range must not invalidate
    // the whole projection or pretend that its original memory slice is empty.
    let pointer = program
        .revert_data
        .iter()
        .find(|data| {
            ir.block(data.block)
                .unwrap()
                .source
                .is_some_and(|source| source.pc == 7802)
        })
        .expect("unknown pointer still has an occurrence-local raw data record");
    if matches!(pointer.content, LiftedRevertDataKind::Raw) {
        assert!(
            !program
                .constructions
                .iter()
                .any(|construction| construction.revert == pointer.id)
        );
    }
    let recovered = program
        .revert_data
        .iter()
        .filter(|data| matches!(data.content, LiftedRevertDataKind::Segments(_)))
        .count();
    let guards = program
        .controls
        .iter()
        .filter(|control| {
            control
                .guard
                .as_ref()
                .is_some_and(|guard| matches!(guard.payload, LiftedGuardPayload::Data(_)))
        })
        .count();
    let index = program.render_index(ir);
    let boundary = ir.structured.as_ref().unwrap().root.id;
    let selected = program
        .controls
        .iter()
        .filter(|control| {
            control
                .guard
                .as_ref()
                .is_some_and(|guard| matches!(guard.payload, LiftedGuardPayload::Data(_)))
                && matches!(
                    index.if_plan(control.node, boundary),
                    evm_decompile::LiftedIfPlan::Require(_)
                )
        })
        .count();
    println!(
        "PoolManager revert data: total={}, recovered={recovered}, data_guards={guards}, selected_data_requires={selected}, dynamic_site={:?}",
        program.revert_data.len(),
        pointer.content
    );
    let export = export_text(artifact, &Cancellation::default());
    assert_eq!(export.status, TextExportStatus::Complete);
    assert!(export.text.contains("Panic(0x41)"));
    assert!(export.text.contains("selector = 0xd5e2f7ab"));
    assert!(export.text.contains("selector = 0x1ad777f8"));
}

// A dispatcher case/transition remains real retained structure, even though it
// is not a separate owned AST node. Count it so fallback cannot hide growth.
fn structure_weight(root: &StructuredNode) -> usize {
    let mut weight = 0;
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        weight += 1;
        match &node.kind {
            StructuredKind::Sequence(children) => pending.extend(children),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                pending.extend([&**then_branch, &**else_branch]);
            }
            StructuredKind::While { header, body, .. } => {
                pending.extend([&**header, &**body]);
            }
            StructuredKind::Indirect { arms, .. } => {
                weight += arms.len();
                pending.extend(arms.iter().map(|arm| &arm.body));
            }
            StructuredKind::Dispatcher { cases, .. } => {
                weight += cases.len()
                    + cases
                        .iter()
                        .map(|case| case.transitions.len())
                        .sum::<usize>();
            }
            _ => {}
        }
    }
    weight
}

#[test]
fn real_pair_and_pool_manager_finish_with_structure_proportional_to_their_cfg() {
    for fixture in [pair(), pool_manager()] {
        let stage = fixture.artifact.current().unwrap();
        let weight = structure_weight(&stage.ir.structured.as_ref().unwrap().root);
        // Fixture acceptance, not an assertion of a universal complexity bound.
        // The prior Pair has 527,810 AST nodes for 883 blocks and 1,003 edges;
        // PoolManager never produced a Structured stage at all.
        let ceiling = 32 * (stage.ir.blocks.len() + stage.ir.edges.len());
        assert!(
            weight <= ceiling,
            "retained structure {weight} exceeds fixture ceiling {ceiling}"
        );
        let structured = fixture
            .artifact
            .stages
            .iter()
            .find(|s| s.kind == StageKind::Structured)
            .unwrap();
        let simplified = fixture
            .artifact
            .stages
            .iter()
            .find(|s| s.kind == StageKind::Simplified)
            .unwrap();
        let mut primitive = structured.ir.clone();
        primitive.structured = None;
        assert_eq!(
            primitive, simplified.ir,
            "region planning must preserve every primitive body and edge transfer"
        );
        let mut witness = stage.ir.clone();
        witness.lifted = None;
        assert_eq!(
            witness, structured.ir,
            "lifting must preserve the admitted primitive/control witness"
        );
        let export = export_text(&fixture.artifact, &Cancellation::default());
        assert_eq!(
            export.status,
            TextExportStatus::Complete,
            "a completed artifact must also export its entire fixed contract"
        );
    }
}

#[test]
fn pool_manager_keeps_four_natural_loops_and_one_loop_local_normal_join() {
    let ir = &pool_manager().artifact.current().unwrap().ir;
    let root = &ir.structured.as_ref().unwrap().root;
    let mut pending = vec![root];
    let mut loops = 0;
    let mut dispatchers = 0;
    let mut joins = 0;
    while let Some(node) = pending.pop() {
        match &node.kind {
            StructuredKind::Sequence(children) => pending.extend(children),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => pending.extend([&**then_branch, &**else_branch]),
            StructuredKind::While { header, body, .. } => {
                loops += 1;
                pending.extend([&**header, &**body]);
            }
            StructuredKind::Indirect { arms, .. } => {
                pending.extend(arms.iter().map(|arm| &arm.body));
            }
            StructuredKind::Dispatcher { .. } => dispatchers += 1,
            StructuredKind::Block { block } => {
                // The captured loop diamond's two nonfailing arms meet here.
                // A finite REVERT alternative must not duplicate this suffix.
                joins += usize::from(ir.block(*block).unwrap().source.unwrap().pc == 0x53bb);
            }
            _ => {}
        }
    }
    assert_eq!(
        dispatchers, 0,
        "the captured CFG has no irreducible residual"
    );
    assert_eq!(loops, 4);
    assert_eq!(joins, 1, "loop-local normal continuation must be shared");
    assert!(
        structure_weight(root) <= 2 * (ir.blocks.len() + ir.edges.len()),
        "natural regions must not replace the former 655-case dispatcher with an expanded tree"
    );
}

fn execute(fixture: &Fixture, host: MemoryHost) -> (ExecutionOutcome, Vec<u8>) {
    let limits = ExecutionLimits::default();
    let mut raw_host = host.clone();
    let raw = execute_bytecode(&fixture.input, &mut raw_host, &limits).unwrap();
    let stage = fixture.artifact.current().unwrap();
    let mut graph_host = host.clone();
    let graph = execute_ir(stage, &mut graph_host, &limits).unwrap();
    let mut tree_host = host;
    let tree = execute_structured(stage, &mut tree_host, &limits).unwrap();
    assert_eq!(raw.outcome, graph.outcome);
    assert_eq!(raw.outcome, tree.outcome);
    assert_eq!(raw.observations(), graph.observations());
    assert_eq!(raw.observations(), tree.observations());
    assert_eq!(raw_host, graph_host);
    assert_eq!(raw_host, tree_host);
    let returned = match &raw.outcome {
        ExecutionOutcome::Exit {
            kind: ExitKind::Return | ExitKind::Revert,
            values,
        } if values.len() == 2 && values[1] != U256::ZERO => {
            let offset = usize::try_from(values[0]).unwrap();
            let size = usize::try_from(values[1]).unwrap();
            raw_host.memory[offset..offset + size].to_vec()
        }
        _ => Vec::new(),
    };
    (raw.outcome, returned)
}

fn words(values: &[u64]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| U256::from(*v).to_be_bytes::<32>())
        .collect()
}

#[test]
fn real_pair_reserve_read_and_failed_dispatch_preserve_primitive_observations() {
    let fixture = pair();
    let mut host = MemoryHost::new(&fixture.input, vec![0x09, 0x02, 0xf1, 0xac]);
    host.storage.insert(
        U256::from(8),
        U256::from(13) | (U256::from(17) << 112) | (U256::from(19) << 224),
    );
    let (outcome, data) = execute(fixture, host);
    assert!(matches!(
        outcome,
        ExecutionOutcome::Exit {
            kind: ExitKind::Return,
            ..
        }
    ));
    assert_eq!(data, words(&[13, 17, 19]));
    for calldata in [Vec::new(), vec![0xff; 4], vec![0x09, 0x02, 0xf1]] {
        assert!(matches!(
            execute(fixture, MemoryHost::new(&fixture.input, calldata)).0,
            ExecutionOutcome::Exit {
                kind: ExitKind::Revert,
                ..
            }
        ));
    }
}

#[test]
fn real_pool_manager_storage_and_transient_reads_preserve_dynamic_slots() {
    let fixture = pool_manager();
    for (selector, transient) in [
        ([0x1e, 0x2e, 0xae, 0xaf], false),
        ([0xf1, 0x35, 0xba, 0xaa], true),
    ] {
        for (slot, value) in [(0, 41), (0x123, 73)] {
            let calldata = selector.into_iter().chain(words(&[slot])).collect();
            let mut host = MemoryHost::new(&fixture.input, calldata);
            if transient {
                host.transient.insert(U256::from(slot), U256::from(value));
            } else {
                host.storage.insert(U256::from(slot), U256::from(value));
            }
            let (outcome, data) = execute(fixture, host);
            assert!(matches!(
                outcome,
                ExecutionOutcome::Exit {
                    kind: ExitKind::Return,
                    ..
                }
            ));
            assert_eq!(data, words(&[value]));
        }
    }
    for calldata in [Vec::new(), vec![0xff; 4], vec![0x1e, 0x2e, 0xae, 0xaf]] {
        assert!(matches!(
            execute(fixture, MemoryHost::new(&fixture.input, calldata)).0,
            ExecutionOutcome::Exit {
                kind: ExitKind::Revert,
                ..
            }
        ));
    }
}
