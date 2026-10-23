//! The CLI's native envelopes retain regional evidence without a default-policy switch.
use alloy_primitives::Address;
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, CorrelatedJoin, EnginePolicies, ExecutionConfig,
        FeedbackAcceleration, RegionalSummaries, SemanticPolicies, StructuralPartition,
        control::Control, cooperation::NoExtension,
    },
    domain::{DomainSpec, NumericValue, ProductAlgebra, ValueAlgebra},
    render,
    resource::WorkBudget,
    ssa,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use serde_json::Value;

#[test]
fn known_call_terminal_frontier_text_keeps_the_legacy_tag_and_region_explanation() {
    let root = Address::repeat_byte(0x11);
    let callee = Address::repeat_byte(0x22);
    let mut world = EvmWorld::<NumericValue>::new(
        Fork::Osaka,
        "offline known region without terminal evidence",
    );
    world
        .insert(
            root,
            EvmAccount::from_hex(&format!("5f5f5f5f5f73{}5af1", "22".repeat(20)), Fork::Osaka)
                .unwrap(),
        )
        .unwrap();
    world
        .insert(callee, EvmAccount::from_hex("5b5f56", Fork::Osaka).unwrap())
        .unwrap();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        RegionalSummaries::with_early_known_admission(),
    ));
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let native = engine
        .run(
            AnalysisInput::new(world, EvmEntry::new(root)).unwrap(),
            ExecutionConfig {
                root_terminal_coverage: false,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
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
fn regional_evidence_survives_plain_and_ssa_cli_envelopes_with_native_13() {
    let root = Address::repeat_byte(0x11);
    let callee = Address::repeat_byte(0x22);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "offline CLI regional envelope");
    world
        .insert(
            root,
            EvmAccount::from_hex(
                &format!("5f5f5f5f5f73{}5af15000", "22".repeat(20)),
                Fork::Osaka,
            )
            .unwrap(),
        )
        .unwrap();
    world
        .insert(callee, EvmAccount::from_hex("5f5ffd", Fork::Osaka).unwrap())
        .unwrap();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        RegionalSummaries::with_early_known_admission(),
    ));
    for max_transfers in [None, Some(2)] {
        let mut config = ExecutionConfig {
            root_terminal_coverage: false,
            ..Default::default()
        };
        if let Some(limit) = max_transfers {
            config.analysis.max_transfers = limit;
        }
        let mut extension = NoExtension;
        let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
        let native = engine
            .run(
                AnalysisInput::new(world.clone(), EvmEntry::new(root)).unwrap(),
                config,
                &Control::default(),
            )
            .unwrap();
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
        assert_eq!(plain["schema_version"], 13);
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
        assert_eq!(wrapped["analysis"]["schema_version"], 13);
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
