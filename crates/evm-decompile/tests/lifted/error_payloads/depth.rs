use super::*;
use evm_decompile::{Completion, ExecutionLimits, Limits, execute_bytecode, execute_ir};

fn chain(count: usize) -> evm_decompile::Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    for _ in 0..count {
        asm.number(64).op(0x51).number(1).op(0x90).op(0x52);
    }
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

#[test]
fn small_reaching_pointer_chain_keeps_positive_payload_and_memory_semantics() {
    let input = chain(8);
    let artifact = checked(&input);
    assert_message(&artifact, DS_MATH);
    let (outcome, host, _) = oracle::certify(
        &input,
        &artifact,
        MemoryHost::new(&input, Vec::new()),
        1_048_576,
    );
    assert_eq!(
        oracle::reverted(&outcome.unwrap(), &host),
        Some(oracle::abi(DS_MATH))
    );
}

#[test]
fn deep_acyclic_pointer_memory_dependencies_refuse_or_finish_without_recursive_stack_failure() {
    let input = chain(1_000);
    let config = Config {
        limits: Limits {
            max_work: 300_000_000,
            max_history_entities: 5_000_000,
            ..Limits::default()
        },
        ..Config::default()
    };
    let artifact = decompile(&input, &config);
    assert!(
        matches!(
            artifact.completion,
            Completion::Complete | Completion::Limited
        ),
        "{:?}",
        artifact.diagnostics
    );
    assert!(artifact.statistics.work <= config.limits.max_work);
    assert!(artifact.statistics.history_entities <= config.limits.max_history_entities);
    super::super::support::validated(&artifact);
    let current = artifact
        .current()
        .expect("a refusal retains validated source IR");
    assert_ne!(
        current.kind,
        StageKind::Decoded,
        "the diagnostic budget reaches pointer analysis"
    );
    let mut raw_host = MemoryHost::new(&input, Vec::new());
    let raw = execute_bytecode(&input, &mut raw_host, &ExecutionLimits::default()).unwrap();
    let mut graph_host = MemoryHost::new(&input, Vec::new());
    let graph = execute_ir(current, &mut graph_host, &ExecutionLimits::default()).unwrap();
    assert_eq!(raw.outcome, graph.outcome);
    assert_eq!(raw.observations(), graph.observations());
    assert_eq!(raw_host, graph_host);
    assert_eq!(
        oracle::reverted(&raw.outcome, &raw_host),
        Some(oracle::abi(DS_MATH))
    );
}
