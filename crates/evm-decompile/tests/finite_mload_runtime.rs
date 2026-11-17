//! The supplied runtime's loop must retain its dispatch table through recovery.
#[path = "finite_mload_runtime/decoder_memory.rs"]
mod decoder_memory;

use evm_decompile::{
    Cancellation, Completion, Config, FactScope, Fork, Input, Limits, StageKind, Terminator,
    TextExportStatus, U256, ValidationStatus, ValueFact, decompile, export_text,
};
use std::collections::BTreeSet;

#[test]
fn supplied_runtime_recovers_every_table_context_and_finishes_complete() {
    let bytes = evm_bytecode::parse_hex(include_str!(
        "fixtures/recovery/provided-rollup-dispatch.hex"
    ))
    .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/recovery/provided-rollup-dispatch.json"
    ))
    .unwrap();
    assert_eq!(
        bytes.len() as u64,
        metadata["byte_length"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:#x}", alloy_primitives::keccak256(&bytes)),
        metadata["code_hash"].as_str().unwrap()
    );
    let input = Input::new(bytes, Fork::Osaka);
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
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| { stage.validation.status == ValidationStatus::Passed })
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    let baseline = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Baseline)
        .unwrap();
    let ir = &baseline.ir;
    assert_eq!(ir.fact_scope, FactScope::CompleteLocalCfg);
    assert!(ir.blocks.iter().all(|block| !block.terminator.is_partial()));
    let expected: BTreeSet<_> = [0x3ebf_usize, 0x3ec6, 0x3ef1, 0x3f06].into_iter().collect();
    let addresses: BTreeSet<_> = (0..8).map(|i| U256::from(0x80 + i * 32)).collect();
    let mut closed_tables = 0;
    let mut open_tables = 0;
    for block in &ir.blocks {
        let Some(jump) = block
            .operations
            .last()
            .filter(|op| op.opcode == 0x56 && op.source.is_some_and(|span| span.pc == 0x3a95))
        else {
            continue;
        };
        let Terminator::Jump {
            edges,
            unresolved,
            invalid_targets,
            ..
        } = &block.terminator
        else {
            panic!("table dispatch lost its jump: {jump:?}");
        };
        // Check every context, including the loop body after numeric widening.
        if *unresolved {
            open_tables += 1;
            continue;
        }
        let load = block
            .operations
            .iter()
            .find(|op| op.opcode == 0x51 && op.source.is_some_and(|span| span.pc == 0x3a85))
            .unwrap();
        let ValueFact::Constants(offsets) = &ir.value(load.inputs[0]).unwrap().fact else {
            panic!("the table read must have a finite address cover");
        };
        assert_eq!(offsets.iter().copied().collect::<BTreeSet<_>>(), addresses);
        let ValueFact::Constants(targets) = &ir.value(load.outputs[0]).unwrap().fact else {
            panic!("known table entries must produce a complete finite target cover");
        };
        assert_eq!(
            targets
                .iter()
                .map(|target| usize::try_from(*target).unwrap())
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert!(invalid_targets.is_empty());
        assert_eq!(
            edges
                .iter()
                .map(|edge| ir
                    .block(ir.edge(*edge).unwrap().to)
                    .unwrap()
                    .source
                    .unwrap()
                    .pc)
                .collect::<BTreeSet<_>>(),
            expected
        );
        closed_tables += 1;
    }
    assert!(
        closed_tables > 0,
        "the supplied finite table was never recovered"
    );
    assert_eq!(open_tables, 0, "a loop context lost its dispatch table");
    decoder_memory::certify(&input, &artifact);
    let export = export_text(&artifact, &Cancellation::default());
    assert_eq!(export.status, TextExportStatus::Complete);
    println!(
        "supplied runtime: completion={:?}, closed_table_contexts={closed_tables}, open_table_contexts={open_tables}, blocks={}, edges={}",
        artifact.completion,
        ir.blocks.len(),
        ir.edges.len()
    );
}
