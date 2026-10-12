//! Real opcode execution is projected, encoded and parsed before assertions.
use super::*;

fn analyze_hex(hex: &str) -> api::AnalysisReport {
    crate::analyze::analyze(api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: hex.into(),
            address: Some(Address::repeat_byte(0x11).to_string()),
        }),
        ..api::AnalyzeRequest::default()
    })
    .unwrap()
}
fn roundtrip(report: api::AnalysisReport) -> api::AnalysisReport {
    report.validate().unwrap();
    let bytes = serde_json::to_vec(&report).unwrap();
    let parsed: api::AnalysisReport = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed, report);
    parsed.validate().unwrap();
    parsed
}
#[test]
fn actual_symbolic_storage_write_read_reaches_protocol_and_ssa() {
    let report = roundtrip(analyze_hex("5f3580600790555400"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(
        report
            .symbolic
            .nodes
            .iter()
            .any(|node| matches!(node, api::Expression::Store(_)))
    );
    assert!(
        report
            .states
            .iter()
            .filter_map(|state| state.exit.as_ref())
            .any(|machine| machine.frames.last().is_some_and(|frame| frame
                .stack
                .last()
                .is_some_and(|v| v.constants == Some(vec!["0x7".into()]))))
    );
    assert!(report.ssa.values.iter().any(|v| {
        matches!(&v.origin,api::SsaValueOrigin::Instruction(origin) if origin.pc==7)
            && v.facts.iter().any(|fact| {
                fact.status == api::SsaFactStatus::Current
                    && fact.constant.as_deref() == Some("0x7")
            })
    }));
    for store in &report.stores {
        for account in &store.accounts {
            report
                .symbolic
                .reference(
                    None,
                    api::SymbolicReference {
                        sort: api::SymbolicSort::Slots,
                        node: account.storage_root.node,
                    },
                )
                .unwrap();
        }
    }
}

#[test]
fn actual_storage_alias_keeps_both_ordered_values_and_the_read_address() {
    let report = roundtrip(analyze_hex("5f358060079055602035600990555400"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    // select(store(store(S,k,7),q,9),k) preserves the conditional aliasing
    // directly; the native graph need not materialize a separate scalar Ite.
    let read = report
        .symbolic
        .nodes
        .iter()
        .find_map(|node| match node {
            api::Expression::SlotRead(read) => Some(read),
            _ => None,
        })
        .unwrap();
    let api::Expression::Store(latest) = &report.symbolic.nodes[read.state.node] else {
        panic!("read retains the latest update")
    };
    let api::Expression::Store(earlier) = &report.symbolic.nodes[latest.previous.node] else {
        panic!("read retains the earlier update")
    };
    assert_eq!(read.key, earlier.key);
    assert_ne!(read.key, latest.key);
    assert_eq!(
        report.symbolic.nodes[latest.value.node],
        api::Expression::Constant("0x9".into())
    );
    assert_eq!(
        report.symbolic.nodes[earlier.value.node],
        api::Expression::Constant("0x7".into())
    );
    assert!(
        report
            .states
            .iter()
            .filter_map(|state| state.exit.as_ref())
            .any(|machine| {
                machine.frames.last().is_some_and(|frame| {
                    frame.stack.last().is_some_and(|value| {
                        value.constants == Some(vec!["0x7".into(), "0x9".into()])
                            && value.expression.is_some()
                    })
                })
            })
    );
    assert_eq!(
        report
            .symbolic
            .nodes
            .iter()
            .filter(|node| matches!(node, api::Expression::Store(_)))
            .count(),
        2
    );
}

#[test]
fn consumed_symbolic_memory_address_keeps_complete_non_singleton_numeric_facts() {
    let report = roundtrip(analyze_hex("5f356010166007905200"));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    let facts = report
        .byte_arrays
        .iter()
        .filter(|bytes| bytes.memory)
        .flat_map(|bytes| &bytes.symbolic_facts);
    assert!(
        facts.into_iter().any(|fact| {
            fact.value.unsigned.lower == "0x0"
                && fact.value.unsigned.upper == "0x10"
                && fact.value.known_zero != "0x0"
                && fact.value.expression == Some(fact.expression)
        }),
        "the consumed p = calldata_word & 0x10 must retain its interval, masks and exact expression binding"
    );
}
#[test]
fn unknown_length_copy_retains_guard_and_pre_effect_state_in_json() {
    let report = roundtrip(analyze_hex("366000803700"));
    assert!(
        report
            .symbolic
            .nodes
            .iter()
            .any(|node| matches!(node, api::Expression::Copy(_)))
    );
    let range = report
        .frontiers
        .iter()
        .find_map(|frontier| frontier.range.as_ref())
        .expect("unbounded input must retain the memory-policy side");
    assert_eq!(range.request.kind, api::RangeKind::MemoryPolicy);
    assert!(range.condition.expression.is_some());
    assert_eq!(range.nonzero, Some(false));
    let pending = &report.pending_machines[range.payload];
    let frame = pending.frames.last().unwrap();
    let view = &report.symbolic.nodes[report.byte_arrays[frame.memory].symbolic.node];
    let api::Expression::View(view) = view else {
        panic!("memory must retain a typed view")
    };
    assert!(matches!(
        report.symbolic.nodes[view.state.node],
        api::Expression::BytesZero
    ));
    assert!(pending.frames.last().unwrap().stack.is_empty());
    assert!(
        report
            .ssa
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| instruction.opcode == 0x37
                && instruction.progress == api::InstructionProgress::Completed)
    );
}
#[test]
fn legal_twenty_four_kib_codecopy_exports_one_lazy_copy_and_exact_source() {
    let source = format!("616000600a5f3900{}", "00".repeat(24 * 1024 - 8));
    let report = roundtrip(analyze_hex(&source));
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert_eq!(
        report
            .symbolic
            .nodes
            .iter()
            .filter(|node| matches!(node, api::Expression::Copy(_)))
            .count(),
        1
    );
    assert!(
        report.symbolic.nodes.iter().any(
            |node| matches!(node,api::Expression::BytesExact(bytes) if bytes.len()==2+24*1024*2)
        )
    );
    assert!(report.symbolic.nodes.len() < 64);
    assert!(report.metadata.export_usage.exact_bytes >= 24 * 1024);
}
#[test]
fn symbolic_export_limits_and_cancellation_are_typed_without_changing_native_values() {
    use evm_abstract::domain::{AbstractValue, symbolic::ExprId};
    let control = Control::default();
    let source = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    let retained = source.clone();
    let mut pool = super::super::symbolic::Pool::new(&control);
    pool.limits.max_nodes = 0;
    let failure = pool.value(&source).unwrap_err();
    assert!(matches!(
        failure.details,
        api::ErrorDetails::Export(api::ExportFailure {
            kind: api::ExportFailureKind::Nodes,
            ..
        })
    ));
    assert_eq!(source, retained);
    let control = Control::new(Default::default());
    control.cancellation().cancel();
    let mut pool = super::super::symbolic::Pool::new(&control);
    let failure = pool.value(&source).unwrap_err();
    assert_eq!(failure.code, api::ApiErrorCode::Cancelled);
    assert!(matches!(
        failure.details,
        api::ErrorDetails::Export(api::ExportFailure {
            kind: api::ExportFailureKind::Cancelled,
            ..
        })
    ));
    assert_eq!(source, retained);
}
