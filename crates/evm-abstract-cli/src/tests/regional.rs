//! The CLI's native envelopes retain regional evidence without a default-policy switch.
use alloy_primitives::Address;
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, CorrelatedJoin, EnginePolicies, EvmWorldAnalysis,
        ExecutionConfig, FeedbackAcceleration, RegionalSummaries, SemanticPolicies,
        StructuralPartition, control::Control, cooperation::NoExtension,
    },
    domain::{DomainSpec, NumericValue, ProductAlgebra, ValueAlgebra},
    render,
    resource::WorkBudget,
    ssa,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use serde_json::Value;

fn analyze_region(
    root_code: &str,
    callee_code: &str,
    max_transfers: Option<usize>,
) -> (EvmWorldAnalysis<NumericValue>, ValueAlgebra<ProductAlgebra>) {
    analyze_region_with_policy(
        root_code,
        callee_code,
        max_transfers,
        RegionalSummaries::with_early_known_admission(),
    )
}

fn analyze_region_with_policy<S: evm_abstract::analysis::ProcedureSummaryPolicy<ProductAlgebra>>(
    root_code: &str,
    callee_code: &str,
    max_transfers: Option<usize>,
    summary_policy: S,
) -> (EvmWorldAnalysis<NumericValue>, ValueAlgebra<ProductAlgebra>) {
    let root = Address::repeat_byte(0x11);
    let callee = Address::repeat_byte(0x22);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "offline CLI regional evidence");
    world
        .insert(root, EvmAccount::from_hex(root_code, Fork::Osaka).unwrap())
        .unwrap();
    world
        .insert(
            callee,
            EvmAccount::from_hex(callee_code, Fork::Osaka).unwrap(),
        )
        .unwrap();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        summary_policy,
    ));
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let mut config = ExecutionConfig {
        root_terminal_coverage: false,
        ..Default::default()
    };
    if let Some(limit) = max_transfers {
        config.analysis.max_transfers = limit;
    }
    let native = engine
        .run(
            AnalysisInput::new(world, EvmEntry::new(root)).unwrap(),
            config,
            &Control::default(),
        )
        .unwrap();
    (native, algebra)
}

#[test]
fn native_process_receipts_survive_plain_and_ssa_cli_envelopes() {
    let (native, _) = analyze_region_with_policy(
        "5b5f56",
        "00",
        None,
        evm_abstract::analysis::KnownProcessSummaries,
    );
    assert_eq!(native.status(), evm_abstract::analysis::Status::Converged);
    let original = serde_json::to_value(render::world::evidence(&native).unwrap()).unwrap();
    let mut budget = WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK);
    let ir = ssa::build_normalized_partial_world(&native, &mut budget).unwrap();
    for normalized in [None, Some(&ir)] {
        let encoded: Value = serde_json::from_str(
            &crate::json::world(
                &native,
                normalized,
                crate::numeric::NumericModel::Product,
                &budget,
            )
            .unwrap(),
        )
        .unwrap();
        let analysis = if normalized.is_some() {
            &encoded["analysis"]
        } else {
            &encoded
        };
        assert_eq!(analysis["schema_version"], 16);
        assert_eq!(analysis["domain_spec"]["schema_version"], 3);
        assert_eq!(analysis["domain_spec"]["cost_version"], 9);
        assert_eq!(
            analysis["procedure_summaries"],
            original["procedure_summaries"]
        );
        let record = analysis["procedure_summaries"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(record["kind"], "KnownProcess");
        let key = &record["procedures"][0]["frame"];
        assert_eq!(key.as_object().unwrap().len(), 1);
        assert!(key["process"]["source"]["Runtime"]["hash"].is_string());
        assert_eq!(key["process"]["epoch"].as_str().unwrap().len(), 66);
        assert!(record["procedures"][0]["process_input"].is_object());
        let receipt = &record["regional_cfg"]["nodes"][0]["transfer"];
        assert_eq!(receipt["semantics"], "KnownProcess");
        assert_eq!(receipt["visited_block"], false);
        assert_eq!(receipt["outcomes"], serde_json::json!(["Failure"]));
        assert_eq!(receipt["process"]["actions"], serde_json::json!([]));
    }
}

#[test]
fn known_call_terminal_frontier_text_keeps_the_legacy_tag_and_region_explanation() {
    let (native, algebra) = analyze_region(
        &format!("5f5f5f5f5f73{}5af1", "22".repeat(20)),
        "5b5f56",
        None,
    );
    assert_eq!(native.status(), evm_abstract::analysis::Status::Incomplete);
    assert!(
        native.frontiers().iter().any(|frontier| frontier.reason
            == evm_abstract::analysis::FrontierReason::RecursiveNoTerminal)
    );
    assert!(
        native
            .procedure_summaries()
            .iter()
            .any(|summary| summary.kind == evm_abstract::analysis::ProcedureSummaryKind::KnownCall)
    );
    let verbose = render::world::text(&native, &algebra);
    let mut budget = WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK);
    let teaching = render::world::explain_with_partial_ssa(&native, &algebra, &mut budget).unwrap();
    for text in [&verbose, &teaching] {
        assert!(text.contains("RecursiveNoTerminal"));
        assert!(text.contains("Known bytecode region lacks terminal evidence."));
        assert!(!text.contains("Recursive execution has no covered terminal behavior"));
    }
}

#[test]
fn regional_evidence_survives_plain_and_ssa_cli_envelopes_with_native_15() {
    for max_transfers in [None, Some(2)] {
        let (native, _) = analyze_region(
            &format!("5f5f5f5f5f73{}5af15000", "22".repeat(20)),
            "5f5ffd",
            max_transfers,
        );
        let mut budget = WorkBudget::new(crate::defaults::DEFAULT_POSTPROCESS_WORK);
        let plain: Value = serde_json::from_str(
            &crate::json::world(
                &native,
                None,
                crate::numeric::NumericModel::Product,
                &budget,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(plain["schema_version"], 16);
        assert_eq!(plain["domain_spec"]["cost_version"], 9);
        for (record, encoded) in native
            .procedure_summaries()
            .iter()
            .zip(plain["procedure_summaries"].as_array().unwrap())
        {
            assert_eq!(encoded["source_current"], record.source_current);
            assert_eq!(encoded["closed"], record.closed);
        }
        if max_transfers.is_none() {
            assert_eq!(plain["procedure_summaries"][0]["kind"], "KnownCall");
            assert!(
                !plain["procedure_summaries"][0]["regional_cfg"]["nodes"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        let original: Value =
            serde_json::to_value(render::world::evidence(&native).unwrap()).unwrap();
        assert_eq!(
            plain["procedure_summaries"],
            original["procedure_summaries"]
        );
        let ir = ssa::build_normalized_partial_world(&native, &mut budget).unwrap();
        let wrapped: Value = serde_json::from_str(
            &crate::json::world(
                &native,
                Some(&ir),
                crate::numeric::NumericModel::Product,
                &budget,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(wrapped["analysis"]["schema_version"], 16);
        assert_eq!(
            wrapped["analysis"]["procedure_summaries"],
            plain["procedure_summaries"]
        );
        assert_eq!(wrapped["analysis"]["status"], plain["status"]);
        assert_eq!(wrapped["analysis"]["frontiers"], plain["frontiers"]);
        assert!(wrapped[if ir.complete() { "ssa" } else { "partial_ssa" }].is_object());
        assert!(wrapped.get("regional_cfg").is_none());
    }
}
