//! The supplied runtime's first table lookup must recover every known handler.
use evm_decompile::{
    Cancellation, Completion, Config, Fork, Input, Limits, StageKind, Terminator, TextExportStatus,
    U256, ValidationStatus, ValueFact, decompile, export_text,
};
use std::collections::BTreeSet;

#[test]
fn supplied_runtime_recovers_the_finite_table_without_claiming_every_loop_is_closed() {
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
    assert!(
        matches!(
            artifact.completion,
            Completion::Complete | Completion::Partial
        ),
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
        // Later loop contexts may lose table observations after unknown writes.
        // A recovered first dispatch must not silently close those contexts.
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
    let export = export_text(&artifact, &Cancellation::default());
    assert_eq!(export.status, TextExportStatus::Complete);
    println!(
        "supplied runtime: completion={:?}, closed_table_contexts={closed_tables}, open_table_contexts={open_tables}, blocks={}, edges={}",
        artifact.completion,
        ir.blocks.len(),
        ir.edges.len()
    );
}
