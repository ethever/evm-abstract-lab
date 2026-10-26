//! Default application dispatch and qualified prefix delivery over real graphs.
use super::super::*;
use evm_abstract::{
    Fork,
    domain::{EvmValue, NumericValue},
    world::AddressInput,
};

fn fixture(
    code: &str,
    detail_work: usize,
) -> (analysis::HybridAnalysis<NumericValue>, api::AnalyzeRequest) {
    let request = api::AnalyzeRequest {
        limits: api::AnalysisLimits {
            max_states: 64,
            root_terminal_coverage: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
        input::spec(&request.limits).unwrap(),
    ));
    let address = Address::repeat_byte(0x11);
    let mut world = EvmWorld::offline(
        evm_abstract::Fork::Osaka,
        "app-hybrid",
        "fixed application fixture",
    );
    world
        .insert(
            address,
            EvmAccount::from_hex(code, evm_abstract::Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry {
        address,
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: AddressInput::Concrete(address),
            value: EvmValue::top(),
            ..Default::default()
        },
    };
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let source = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::new(world, entry).unwrap(),
            input::config(&request.limits).unwrap(),
            analysis::HybridConfig {
                max_detail_work: detail_work,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    (source, request)
}

#[test]
fn default_bytecode_uses_hybrid_and_concrete_small_input_selects_native() {
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "600160020100".into(),
            address: Some(Address::repeat_byte(0x33).to_string()),
        }),
        ..Default::default()
    };
    let report = analyze(request).unwrap();
    report.validate().unwrap();
    let hybrid = report.hybrid.as_ref().unwrap();
    assert_eq!(hybrid.coverage.chosen, api::HybridInterpretation::Native);
    assert!(hybrid.coverage.baseline_current && hybrid.coverage.detail_current);
    assert!(report.global.is_some() && report.native.is_some());
    let global = report.global.as_ref().unwrap();
    let native = report.native.as_ref().unwrap();
    assert!(global.transfers > 0 && native.transfers > global.transfers);
    assert_eq!(report.transfers, native.transfers);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(!hybrid.local_prefixes.is_empty());
    assert!(
        hybrid
            .local_prefixes
            .iter()
            .all(|prefix| prefix.namespace == api::AnalysisNamespace::Native)
    );
    let decoded: api::AnalysisReport =
        serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, report);
}

#[test]
fn current_environment_native_remains_inspectable_while_server_selects_global() {
    for unknown in [false, true] {
        let address = |id: u64| Address::from_word(evm_abstract::U256::from(id).into());
        let mut world = EvmWorld::new(Fork::Osaka, "current F1 server selection");
        let target = if unknown { 0x201 } else { 0x301 };
        let root = format!("60205f5f5f5f61{target:04x}5af15060205ff3");
        for (id, code) in [(0x101, root.as_str()), (0x301, "305f5260205ff3")] {
            let mut account = EvmAccount::from_hex(code, Fork::Osaka).unwrap();
            account.balance = EvmValue::constant(evm_abstract::U256::from(1_000_000));
            world.insert(address(id), account).unwrap();
        }
        let entry = EvmEntry::concrete(
            address(0x101),
            address(0x900),
            EvmValue::constant(evm_abstract::U256::ZERO),
            evm_abstract::world::EvmByteArray::empty(),
        );
        let request = api::AnalyzeRequest::default();
        let report = captured::analyze(
            AnalysisInput::new(world, entry).unwrap(),
            &request,
            &Control::default(),
        )
        .unwrap();
        report.validate().unwrap();
        let coverage = &report.hybrid.as_ref().unwrap().coverage;
        assert!(coverage.baseline_current && coverage.detail_current);
        assert_eq!(coverage.detail, api::HybridDetailState::Current);
        assert_eq!(
            coverage.detail_binding,
            api::HybridNativeBinding::OriginalOwner
        );
        assert!(report.global.is_some() && report.native.is_some());
        assert!(report.native.as_ref().unwrap().frontiers.is_empty());
        assert!(!report.hybrid.as_ref().unwrap().local_prefixes.is_empty());
        assert_eq!(
            report.selected_view,
            if unknown {
                api::AnalysisNamespace::Baseline
            } else {
                api::AnalysisNamespace::Native
            }
        );
        assert_eq!(
            coverage.chosen,
            if unknown {
                api::HybridInterpretation::ProcessWithDetail
            } else {
                api::HybridInterpretation::Native
            }
        );
    }
}

#[test]
fn closed_global_routes_survive_partial_native_prefix_with_phases_and_frontiers() {
    let code = format!("5f5f5f5f60045afa50{}00", "5f50".repeat(500));
    let (source, request) = fixture(&code, 100_000);
    assert_eq!(
        source.coverage().chosen,
        analysis::HybridInterpretation::ProcessWithDetail
    );
    assert_eq!(
        source.detail().unwrap().status(),
        analysis::Status::Incomplete
    );
    let report = captured::project(&source, &request, &Control::default()).unwrap();
    report.validate().unwrap();
    let hybrid = report.hybrid.as_ref().unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert_eq!(
        report.selected_graph().unwrap().abstract_summaries.len(),
        source.baseline().procedure_summaries().len()
    );
    assert!(
        report
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .iter()
            .any(|summary| !summary.regional_cfg.calls.is_empty())
    );
    let native = report.graph(api::AnalysisNamespace::Native).unwrap();
    assert_eq!(
        report.transfers,
        source.detail().unwrap().transfers() as u64
    );
    assert_eq!(
        native.transfers,
        source.detail().unwrap().transfers() as u64
    );
    assert_eq!(
        report.global.as_ref().unwrap().transfers,
        source.baseline().transfers() as u64
    );
    assert!(!native.frontiers.is_empty());
    assert_eq!(hybrid.local_prefixes.len(), source.local_detail().count());
    for (prefix, actual) in hybrid.local_prefixes.iter().zip(source.local_detail()) {
        assert_eq!(prefix.state, actual.state_id);
        assert_eq!(prefix.revision.0, actual.revision.value());
        assert_eq!(prefix.frame.context, actual.frame.jump_history);
        assert_eq!(prefix.instructions.len(), actual.instructions.len());
        for (instruction, phase) in prefix.instructions.iter().zip(actual.instructions) {
            assert_eq!(instruction.phase, mapping::instruction_progress(*phase));
        }
        assert_eq!(prefix.frontiers.len(), actual.frontiers().count());
    }
    assert!(
        hybrid
            .local_prefixes
            .iter()
            .any(|prefix| !prefix.frontiers.is_empty())
    );
    assert!(
        hybrid
            .local_prefixes
            .iter()
            .flat_map(|prefix| &prefix.instructions)
            .any(|instruction| instruction.phase != api::InstructionProgress::Completed)
    );
    let mut forged = report.clone();
    forged.hybrid.as_mut().unwrap().local_prefixes[0].namespace = api::AnalysisNamespace::Baseline;
    assert_eq!(
        forged.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Namespace
        ))
    );
    let mut forged = report.clone();
    forged.hybrid.as_mut().unwrap().local_prefixes[0].revision = api::DecimalU64(u64::MAX);
    assert_eq!(
        forged.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source
        ))
    );
    let mut forged = report;
    let prefix = forged
        .hybrid
        .as_mut()
        .unwrap()
        .local_prefixes
        .iter_mut()
        .find(|prefix| !prefix.frontiers.is_empty())
        .unwrap();
    prefix.frontiers.clear();
    assert_eq!(
        forged.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Frontier
        ))
    );
}

#[test]
fn symbolic_default_owner_retains_namespace_conditioned_detail_and_global_relation() {
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "60095f5500".into(),
            address: None,
        }),
        ..Default::default()
    };
    let report = analyze(request).unwrap();
    report.validate().unwrap();
    let hybrid = report.hybrid.as_ref().unwrap();
    assert!(
        matches!(&hybrid.coverage.detail_binding, api::HybridNativeBinding::SymbolicOwnerNamespace(binding)
        if binding.namespace == report.selected_graph().unwrap().metadata.entry_address)
    );
    assert_ne!(hybrid.coverage.chosen, api::HybridInterpretation::Native);
    assert!(
        hybrid
            .local_prefixes
            .iter()
            .all(|prefix| prefix.scope == api::HybridLocalScope::NativeOwnerNamespacePrefix)
    );
    assert!(report.graph(api::AnalysisNamespace::Baseline).is_some());
    assert!(report.graph(api::AnalysisNamespace::Native).is_some());
    let mut forged = report.clone();
    let hybrid = forged.hybrid.as_mut().unwrap();
    hybrid.coverage.detail_binding = api::HybridNativeBinding::OriginalOwner;
    for prefix in &mut hybrid.local_prefixes {
        prefix.binding = api::HybridNativeBinding::OriginalOwner;
        prefix.scope = api::HybridLocalScope::RetainedInputPrefix;
    }
    assert_eq!(
        forged.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source
        ))
    );
    forged.hybrid.as_mut().unwrap().coverage.chosen = api::HybridInterpretation::Native;
    forged.selected_view = api::AnalysisNamespace::Native;
    assert_eq!(
        forged.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source
        ))
    );
    let mut wrong_namespace = report;
    let hybrid = wrong_namespace.hybrid.as_mut().unwrap();
    hybrid.coverage.detail_binding =
        api::HybridNativeBinding::SymbolicOwnerNamespace(api::HybridOwnerNamespace {
            namespace: Address::repeat_byte(0x77).to_string(),
        });
    for prefix in &mut hybrid.local_prefixes {
        prefix.binding = hybrid.coverage.detail_binding.clone();
    }
    let encoded = serde_json::to_vec(&wrong_namespace).unwrap();
    let decoded: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        decoded.validate(),
        Err(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source
        ))
    );
}

#[test]
fn failed_baseline_spends_no_precision_and_cancelled_default_publishes_no_report() {
    let (source, request) = fixture("00", 0);
    assert_eq!(source.coverage().detail, analysis::DetailState::Disabled);
    let report = captured::project(&source, &request, &Control::default()).unwrap();
    report.validate().unwrap();
    assert_eq!(report.transfers, source.baseline().transfers() as u64);
    assert!(report.hybrid.as_ref().unwrap().local_prefixes.is_empty());
    let mut request = api::AnalyzeRequest::default();
    request.limits.max_work = 1;
    let report = analyze(request).unwrap();
    assert_eq!(report.status, api::AnalysisStatus::Incomplete);
    assert_eq!(report.transfers, 0);
    assert!(
        report.selected_graph().unwrap().states.is_empty(),
        "one work unit must stop before state admission"
    );
    let api::RootInputBinding::Namespace(binding) =
        &report.selected_graph().unwrap().metadata.root_binding
    else {
        panic!("default bytecode must retain its symbolic root binding")
    };
    assert!(matches!(
        report
            .selected_graph()
            .unwrap()
            .metadata
            .declared_root_source,
        api::ProcessSource::InputProgram(_)
    ));
    let template = report
        .selected_graph()
        .unwrap()
        .accounts
        .iter()
        .find(|row| row.address == binding.coordinate)
        .unwrap();
    let program = report
        .programs
        .iter()
        .find(|program| Some(program.id) == template.program)
        .unwrap();
    assert_eq!(program.kind, api::CodeKind::Runtime);
    assert!(program.code_address.is_none());
    assert!(
        report
            .programs
            .iter()
            .all(|program| program.code_address.as_deref() != Some(binding.coordinate.as_str()))
    );
    report.validate().unwrap();
    let mut forged = report.clone();
    std::sync::Arc::make_mut(&mut forged.programs[program.id]).code_address =
        Some(binding.coordinate.clone());
    assert_eq!(
        forged.validate(),
        Err(api::ReportValidationFailure::Process(
            api::ProcessValidationKind::Source
        ))
    );
    let hybrid = report.hybrid.as_ref().unwrap();
    assert!(!hybrid.coverage.baseline_current);
    assert_eq!(
        hybrid.coverage.work.preparation + hybrid.coverage.work.detail,
        0
    );
    assert!(report.native.is_none());
    let control = Control::new(analysis::progress::Observer::default());
    control.cancellation().cancel();
    assert_eq!(
        analyze_with_control(api::AnalyzeRequest::default(), &control)
            .unwrap_err()
            .code,
        api::ApiErrorCode::Cancelled
    );
}
