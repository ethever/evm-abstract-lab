//! Current Hybrid selection, independent concrete completion and actual receipts.
use super::*;
use evm_abstract::{
    U256,
    analysis::{
        EvmWorldAnalysis, OutcomeKind, ProcedureSummaryKind, ProcessLocation, ProcessSource,
    },
};
use support::{C, Fixture, USER};

const WORK: usize = 2_000_000_000;

fn entry(root: u64) -> EvmEntry<NumericValue> {
    EvmEntry::concrete(
        address(root),
        address(USER),
        support::word(0),
        EvmByteArray::empty(),
    )
}

fn call(target: u64, operation: u8) -> Fixture {
    let mut code = Assembly::default();
    code.call(operation, target as u16, 0, (0, 0), (0, 32))
        .op(opcode::POP)
        .return_range(0, 32);
    code.finish()
}

fn world(root: Option<&Fixture>) -> EvmWorld<NumericValue> {
    let mut leaf = Assembly::default();
    leaf.op(opcode::ADDRESS).return_top();
    let mut world = new_world();
    insert(&mut world, C, &leaf.finish(), &[]);
    if let Some(root) = root {
        insert(&mut world, A, root, &[]);
    }
    world
}

fn hybrid(
    world: EvmWorld<NumericValue>,
    entry: EvmEntry<NumericValue>,
) -> HybridAnalysis<NumericValue> {
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let original = entry.clone();
    let fingerprint = world.fingerprint().unwrap();
    let report = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: WORK,
                ..Default::default()
            },
            HybridConfig::default(),
            &Control::default(),
        )
        .unwrap();
    assert_eq!(report.baseline().entry(), &original);
    assert_eq!(
        report.baseline().world().fingerprint().unwrap(),
        fingerprint
    );
    assert!(
        report.coverage().baseline_current,
        "{:?}",
        report.coverage()
    );
    assert_accounting(&report, WORK);
    eprintln!(
        "F1_CURRENT {}",
        serde_json::json!({
            "coverage": report.coverage(),
            "baseline_status": report.baseline().status(),
            "native_status": report.detail().map(|native| native.status()),
            "native_frontiers": report.detail().map(|native| native.frontiers()),
            "root": original.address,
            "native_missing_c_under_u": report.detail().map(|native| !native_context(native)),
            "process_c_under_u": process_context(report.baseline()),
            "native_records": report.detail().map(|native| native.procedure_summaries().iter().map(|record|
                serde_json::json!({"kind": record.kind, "closed": record.closed, "current": record.source_current,
                    "procedures": record.procedures.iter().map(|p| serde_json::json!({"semantics": p.semantics, "frame": p.frame})).collect::<Vec<_>>()})
            ).collect::<Vec<_>>())
        })
    );
    report
}

fn native_context(report: &EvmWorldAnalysis<NumericValue>) -> bool {
    let frame = |frame: &evm_abstract::analysis::FrameKey| {
        frame.code_address == address(C)
            && frame
                .address_value
                .as_concrete()
                .is_none_or(|owner| owner == address(U))
    };
    report.states().iter().any(|state| frame(state.active()))
        || report.procedure_summaries().iter().any(|record| {
            record.procedures.iter().any(|p| frame(&p.frame))
                || record
                    .regional_cfg
                    .nodes
                    .iter()
                    .any(|node| frame(&node.frame))
        })
}

fn process_context(report: &EvmWorldAnalysis<NumericValue>) -> bool {
    report.procedure_summaries().iter().any(|record| {
        record.closed
            && record.source_current
            && record.regional_cfg.nodes.iter().any(|node| {
                !node.excluded &&
            node.frame.process.as_ref().is_some_and(|key| {
                matches!(key.source, ProcessSource::Runtime {source, ..} if source == address(C))
                    && match key.owner {
                        ProcessOwner::Named(owner) => owner == address(U),
                        ProcessOwner::Residual => report.world().account(address(U)).is_none(),
                        ProcessOwner::RootNamespace => false,
                    }
                    && key.location == ProcessLocation::Block(0)
            }) && record.procedures[node.procedure]
                .process_input
                .as_ref()
                .is_some_and(|input| input.address.contains(U256::from(U)))
                && node
                    .transfer
                    .as_ref()
                    .is_some_and(|transfer| transfer.current && transfer.complete
                        && transfer.semantics == TransferSemantics::KnownProcess
                        && transfer.process.is_some())
            })
    })
}

#[test]
fn unknown_root_with_captured_callback_also_runs_current_native() {
    let world = world(None);
    let report = hybrid(world.clone(), entry(U));
    let native = report
        .detail()
        .expect("current selection accepts a captured callback region");
    assert!(report.coverage().detail_current);
    assert_eq!(report.coverage().detail, DetailState::Current);
    assert!(!native_context(native));
    assert!(process_context(report.baseline()));
    let concrete = oracle::concrete_unknown(&world, U, &call(C, opcode::DELEGATECALL));
    let observation = oracle::execute(&concrete, &entry(U));
    assert_eq!(observation.output_word(0), U256::from(U));
    assert!(observation.is_covered(native));
    assert_eq!(report.coverage().chosen, HybridInterpretation::Process);
    assert_eq!(report.coverage().detail_regions.len(), 1);
}

#[test]
fn known_call_record_with_nested_environment_cannot_select_complete_native() {
    let mut world = world(Some(&call(support::V, opcode::CALL)));
    insert(&mut world, support::V, &call(U, opcode::CALL), &[]);
    let report = hybrid(world, entry(A));
    assert!(report.coverage().detail_current, "{:?}", report.coverage());
    let records = report.detail().unwrap().procedure_summaries();
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|record| record.kind == ProcedureSummaryKind::KnownCall)
    );
    assert!(records.iter().any(|record| {
        record
            .procedures
            .iter()
            .any(|p| p.semantics == TransferSemantics::Environment)
    }));
    assert_eq!(report.coverage().detail, DetailState::Current);
    assert_eq!(
        report.coverage().chosen,
        HybridInterpretation::ProcessWithDetail
    );
    assert!(std::ptr::eq(report.chosen(), report.baseline()));
}

#[test]
fn current_native_environment_keeps_process_for_unknown_delegated_known_control() {
    let abstract_world = world(Some(&call(U, opcode::CALL)));
    let original_entry = entry(A);
    // Only revm sees the concrete completion. The Hybrid World keeps U unknown.
    assert!(!abstract_world.accounts().contains_key(&address(U)));
    let report = hybrid(abstract_world.clone(), original_entry.clone());
    let native = report.detail().expect("captured root admits Native");
    assert!(report.coverage().detail_current, "{:?}", report.coverage());
    assert_eq!(report.coverage().detail, DetailState::Current);
    assert_eq!(
        report.coverage().detail_binding,
        evm_abstract::analysis::HybridNativeBinding::OriginalOwner
    );
    assert_eq!(native.status(), Status::Converged);
    assert!(native.frontiers().is_empty());
    assert!(!native_context(native));
    assert!(process_context(report.baseline()));
    assert!(report.local_detail().next().is_some());
    for operation in [opcode::DELEGATECALL, opcode::CALLCODE] {
        let concrete_world = oracle::concrete_unknown(&abstract_world, U, &call(C, operation));
        let observation = oracle::execute(&concrete_world, &original_entry);
        assert_eq!(observation.kind, OutcomeKind::Return);
        assert_eq!(observation.output_word(0), U256::from(U));
        assert!(observation.steps.iter().any(|step| step.code == address(C)
            && step.owner == address(U)
            && step.pc == 0
            && step.opcode == opcode::ADDRESS));
        // Numeric inclusion is a separate, passing control, not the defect.
        assert!(observation.is_covered(native));
        assert!(observation.is_covered(report.baseline()));
    }
    assert_eq!(
        report.coverage().chosen,
        HybridInterpretation::ProcessWithDetail
    );
    assert!(std::ptr::eq(report.chosen(), report.baseline()));
}

#[test]
fn all_known_current_original_binding_still_selects_native() {
    let report = hybrid(world(Some(&call(C, opcode::CALL))), entry(A));
    assert!(report.coverage().detail_current);
    assert_eq!(report.coverage().detail, DetailState::Current);
    assert_eq!(report.coverage().chosen, HybridInterpretation::Native);
    assert!(std::ptr::eq(report.chosen(), report.detail().unwrap()));
    assert!(
        report
            .detail()
            .unwrap()
            .procedure_summaries()
            .iter()
            .all(
                |record| record.kind != ProcedureSummaryKind::UnknownEnvironment
                    && record
                        .procedures
                        .iter()
                        .all(|p| p.semantics != TransferSemantics::Environment)
            )
    );
}
