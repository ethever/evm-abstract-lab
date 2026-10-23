//! Source changes replace proofs; outside code stays in the bounded complement.
#![allow(dead_code)]

#[path = "support/native.rs"]
mod native;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "unknown_world/support.rs"]
mod support;

mod process_source_currency {
    pub(super) mod catalogue;
    pub(super) mod coverage;
    pub(super) mod currency;
    pub(super) mod discovery;
}

use alloy_primitives::B256;
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, BoundedJoin, EnginePolicies, EvmWorldAnalysis,
        ExecutionConfig, FeedbackAcceleration, FirstResumeStatus, KnownProcessSummaries,
        RecentDistinctJumps, SemanticPolicies, Status, StructuralPartition, control::Control,
        cooperation::NoExtension,
    },
    domain::NumericValue,
    world::{EvmByteArray, EvmWorld},
};
use support::{A, Assembly, address, entry, insert, new_world};

type ProcessPolicies = EnginePolicies<
    RecentDistinctJumps,
    FirstResumeStatus,
    SemanticPolicies<StructuralPartition, BoundedJoin, FeedbackAcceleration, KnownProcessSummaries>,
>;

fn policies() -> ProcessPolicies {
    EnginePolicies::new(RecentDistinctJumps, FirstResumeStatus).with_state(SemanticPolicies::new(
        StructuralPartition,
        BoundedJoin,
        FeedbackAcceleration,
        KnownProcessSummaries,
    ))
}

fn config() -> ExecutionConfig {
    ExecutionConfig {
        root_terminal_coverage: false,
        max_work: 2_000_000_000,
        ..ExecutionConfig::default()
    }
}

fn run(world: EvmWorld<NumericValue>) -> EvmWorldAnalysis<NumericValue> {
    let values = native::product();
    let policies = policies();
    let mut extension = NoExtension;
    AnalysisEngine::new(&values, &policies, &mut extension)
        .run(
            AnalysisInput::new(world, entry(EvmByteArray::empty())).unwrap(),
            config(),
            &Control::default(),
        )
        .unwrap()
}

fn root_epoch(analysis: &EvmWorldAnalysis<NumericValue>) -> B256 {
    let record = analysis.procedure_summaries().last().unwrap();
    record.procedures[record.root_procedure.unwrap()]
        .frame
        .process
        .as_ref()
        .unwrap()
        .epoch
}

fn assert_current(analysis: &EvmWorldAnalysis<NumericValue>) {
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    assert!(analysis.frontiers().is_empty());
    assert!(analysis.root_terminal_coverage().is_none());
    assert!(!analysis.procedure_summaries().is_empty());
    for record in analysis.procedure_summaries() {
        assert!(record.closed && record.source_current);
        for node in &record.regional_cfg.nodes {
            let transfer = node.transfer.as_ref().expect("no pending body at closure");
            assert!(transfer.current && transfer.complete);
            assert_eq!(transfer.input_revision, node.input_revision);
            assert!(!transfer.visited_block);
            assert!(transfer.process.is_some());
            assert_eq!(
                node.frame.process.as_ref().unwrap().epoch,
                root_epoch(analysis)
            );
        }
        for call in &record.regional_cfg.calls {
            for delivery in &call.deliveries {
                assert!(delivery.current);
                assert_eq!(delivery.input_revision, call.input_revision);
                assert!(!delivery.successors.is_empty());
            }
        }
    }
}

fn fixture(bytes: &[u8]) -> support::Fixture {
    support::Fixture {
        bytes: bytes.to_vec(),
        labels: Default::default(),
    }
}

fn evidence(label: &str, analysis: &EvmWorldAnalysis<NumericValue>) {
    eprintln!(
        "PROCESS_SOURCE_CURRENCY {}",
        serde_json::json!({
            "case": label, "status": analysis.status(), "work": analysis.work(),
            "frontiers": analysis.frontiers().iter().map(|f| &f.reason).collect::<Vec<_>>(),
            "records": analysis.procedure_summaries(),
        })
    );
}
