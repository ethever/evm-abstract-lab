//! Existing captured runtime code is the only consumed field. No world or RPC.
use evm_bytecode::Program;
use evm_decompile::{
    Artifact, Completion, Config, DiagnosticCode, ExecutionLimits, ExternalResponse, Fork, Input,
    MemoryHost, StageKind, Strategy, U256, ValidationStatus, decompile, execute_bytecode,
    execute_ir, execute_structured, render_text,
};
use std::collections::BTreeMap;
use std::time::Instant;

fn code(capture: &str, index: usize) -> Input {
    let data: serde_json::Value = serde_json::from_str(capture).unwrap();
    let source = data["accounts"][index]["code"].as_str().unwrap();
    let decoded = Program::from_hex_with_fork(source, Fork::Osaka).unwrap();
    Input::new(decoded.bytes().to_vec(), Fork::Osaka)
}
fn report(label: &str, a: &Artifact, started: Instant) {
    eprintln!(
        "{label}: completion={:?} stages={:?} statistics={:?} elapsed={:?} diagnostics={:?}",
        a.completion,
        a.stages.iter().map(|s| (s.id, s.kind)).collect::<Vec<_>>(),
        a.statistics,
        started.elapsed(),
        a.diagnostics
    );
    if let Ok(dir) = std::env::var("DECOMPILE_EVIDENCE_DIR") {
        std::fs::write(
            format!("{dir}/{label}.json"),
            serde_json::to_vec_pretty(a).unwrap(),
        )
        .unwrap();
        std::fs::write(format!("{dir}/{label}.txt"), render_text(a)).unwrap();
    }
}
#[test]
fn captured_weth_dispatch_and_shared_returns_execute_offline() {
    let input = code(
        include_str!(
            "../../../examples/known-recursion/captured/weth-26180958-observed-world.json"
        ),
        0,
    );
    assert_eq!(input.runtime_bytecode.len(), 3124);
    for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
        let mut config = Config {
            strategy,
            ..Config::default()
        };
        config.limits.max_work = 500_000_000;
        config.limits.max_structuring_depth = 512;
        config.limits.max_constant_set = 32;
        let started = Instant::now();
        let a = decompile(&input, &config);
        report(&format!("weth-{strategy:?}"), &a, started);
        assert!(
            !a.diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::Validation),
            "{:#?}",
            a.diagnostics
        );
        assert!(
            a.stages
                .iter()
                .all(|s| s.validation.status == ValidationStatus::Passed)
        );
        assert_eq!(a.completion, Completion::Complete, "{:#?}", a.diagnostics);
        let baseline = a
            .stages
            .iter()
            .find(|s| s.kind == StageKind::Baseline)
            .unwrap();
        assert!(baseline.ir.blocks.len() > 50);
        let current = a.current().unwrap();
        assert_eq!(current.kind, StageKind::Structured);
        let origins: BTreeMap<_, _> = a.mappings.origins.iter().map(|o| (o.entity, o)).collect();
        for stage in &a.stages {
            for entity in stage
                .ir
                .values
                .iter()
                .map(|v| v.id)
                .chain(stage.ir.edges.iter().map(|e| e.id))
            {
                let key = evm_decompile::EntityRef {
                    stage: stage.id,
                    entity,
                };
                let origin = origins
                    .get(&key)
                    .expect("every source-derived value/edge has an origin");
                assert!(
                    !origin.source.is_empty(),
                    "unresolved context ancestry {key:?}: {origin:?}"
                );
                assert!(origin.parents.iter().all(|p| origins.contains_key(p)));
                assert!(
                    origin
                        .source
                        .iter()
                        .all(|s| s.pc < s.end_pc && s.end_pc <= input.runtime_bytecode.len())
                );
            }
        }
        let query_data = [
            vec![0x31, 0x3c, 0xe5, 0x67],
            vec![0x18, 0x16, 0x0d, 0xdd],
            vec![
                0x70, 0xa0, 0x82, 0x31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
            ],
        ];
        let mut probe_host = MemoryHost::new(&input, query_data[2].clone());
        let probe = execute_bytecode(&input, &mut probe_host, &ExecutionLimits::default()).unwrap();
        let balance_key = probe
            .trace
            .iter()
            .find(|step| step.opcode == 0x54)
            .unwrap()
            .inputs[0];
        let mut hosts: Vec<_> = ["decimals", "total_supply", "balance_of"]
            .into_iter()
            .zip(query_data)
            .map(|(label, data)| (label, MemoryHost::new(&input, data)))
            .collect();
        let mut deposit = MemoryHost::new(&input, vec![0xd0, 0xe3, 0x0d, 0xb0]);
        deposit.context.insert(0x33, U256::from(1));
        deposit.context.insert(0x34, U256::from(7));
        deposit.storage.insert(balance_key, U256::from(100));
        hosts.push(("deposit", deposit));
        for success in [false, true] {
            let mut data = vec![0x2e, 0x1a, 0x7d, 0x4d];
            data.extend(U256::from(1).to_be_bytes::<32>());
            let mut withdraw = MemoryHost::new(&input, data);
            withdraw.context.insert(0x33, U256::from(1));
            withdraw.storage.insert(balance_key, U256::from(100));
            withdraw.responses.push_back(ExternalResponse {
                result: U256::from(u64::from(success)),
                ..ExternalResponse::default()
            });
            hosts.push((
                if success {
                    "withdraw_success"
                } else {
                    "withdraw_failure"
                },
                withdraw,
            ));
        }
        for (label, host) in hosts {
            let mut raw_host = host.clone();
            let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
            match label {
                "deposit" => {
                    assert_eq!(raw_host.storage.get(&balance_key), Some(&U256::from(107)));
                    assert_eq!(raw_host.logs.len(), 1);
                }
                "withdraw_success" => {
                    assert_eq!(raw_host.storage.get(&balance_key), Some(&U256::from(99)));
                    assert_eq!(raw_host.logs.len(), 1);
                    assert!(raw.trace.iter().any(|step| step.opcode == 0xf1));
                }
                "withdraw_failure" => {
                    assert_eq!(raw_host.storage.get(&balance_key), Some(&U256::from(99)));
                    // This host records primitive effects before the enclosing
                    // frame applies rollback, including the preceding SSTORE.
                    assert!(raw_host.logs.is_empty());
                    assert!(matches!(
                        raw.outcome,
                        evm_decompile::ExecutionOutcome::Exit {
                            kind: evm_decompile::ExitKind::Revert,
                            ..
                        }
                    ));
                    assert!(raw.trace.iter().any(|step| step.opcode == 0xf1));
                }
                _ => {}
            }
            let mut graph_host = host.clone();
            let graph = execute_ir(baseline, &mut graph_host, &ExecutionLimits::default()).unwrap();
            assert_eq!(raw.outcome, graph.outcome);
            assert_eq!(raw.observations(), graph.observations());
            assert_eq!(raw_host, graph_host);
            let mut syntax_host = host;
            let syntax =
                execute_structured(current, &mut syntax_host, &ExecutionLimits::default()).unwrap();
            assert_eq!(raw.outcome, syntax.outcome);
            assert_eq!(raw.observations(), syntax.observations());
            assert_eq!(raw_host, syntax_host);
        }
        let wire = serde_json::to_vec(&a).unwrap();
        let _: Artifact = serde_json::from_slice(&wire).unwrap();
    }
}
#[test]
fn larger_captured_usdc_code_obeys_declared_bounds_without_world_setup() {
    let capture = include_str!(
        "../../../examples/known-recursion/captured/usdc-26180958-observed-world.json"
    );
    for (index, label) in [(0, "usdc-implementation"), (1, "usdc-proxy")] {
        let input = code(capture, index);
        for strategy in [Strategy::Dispatcher, Strategy::CodeDuplication] {
            let mut config = Config {
                strategy,
                ..Config::default()
            };
            config.limits.max_work = 2_000_000;
            config.limits.max_iterations = 256;
            let started = Instant::now();
            let a = decompile(&input, &config);
            report(&format!("{label}-{strategy:?}"), &a, started);
            assert_eq!(a.completion, Completion::Limited);
            assert!(a.statistics.work <= config.limits.max_work);
            assert!(
                a.stages
                    .iter()
                    .all(|s| s.validation.status == ValidationStatus::Passed)
            );
            assert!(
                !a.diagnostics
                    .iter()
                    .any(|d| d.code == DiagnosticCode::Validation)
            );
            assert!(a.current().is_some());
            assert!(
                a.diagnostics
                    .iter()
                    .any(|d| d.code == DiagnosticCode::Limit)
            );
        }
    }
}
