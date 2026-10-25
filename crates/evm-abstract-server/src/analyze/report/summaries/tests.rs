//! Final-source qualification is copied, independently of snapshot/local closure.
use super::*;
use alloy_primitives::Address;
use evm_abstract::{
    Fork,
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, cooperation::NoExtension,
    },
    domain::{DomainSpec, NumericValue, ProductAlgebra, ValueAlgebra},
    world::{EvmEntry, EvmWorld},
};

#[test]
fn source_current_projection_preserves_both_flags_and_local_observation_evidence() {
    let world = EvmWorld::<NumericValue>::new(Fork::Osaka, "source certificate projection");
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(DomainSpec::default()));
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let native = engine
        .run(
            AnalysisInput::new(world, EvmEntry::new(Address::repeat_byte(0x11))).unwrap(),
            ExecutionConfig {
                root_terminal_coverage: false,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    let mut observations = native.procedure_summaries().to_vec();
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = false;
    let base = crate::analyze::report::report(
        &native,
        &request,
        api::AnalysisScope::RpcWorld,
        &algebra,
        api::NumericModel::Product,
        &Control::default(),
    )
    .unwrap();
    assert!(!observations.is_empty());
    assert!(observations.iter().any(|record| record.closed
        && record.regional_cfg.nodes.iter().any(|node| {
            node.transfer
                .as_ref()
                .is_some_and(|receipt| receipt.current)
        })));
    for source_current in [false, true] {
        for record in &mut observations {
            record.source_current = source_current;
        }
        let control = Control::default();
        let mut symbolic = Pool::new(&control);
        let mapped = project(&observations, &control, &mut symbolic).unwrap();
        let mut report = base.clone();
        std::sync::Arc::make_mut(report.native.as_mut().unwrap()).abstract_summaries =
            mapped.clone();
        report.validate().unwrap();
        for (record, mapped) in observations.iter().zip(&mapped) {
            assert_eq!(mapped.source_current, source_current);
            assert_eq!(mapped.closed, record.closed);
            for (node, mapped) in record
                .regional_cfg
                .nodes
                .iter()
                .zip(&mapped.regional_cfg.nodes)
            {
                assert_eq!(
                    mapped.transfer.as_ref().map(|receipt| receipt.current),
                    node.transfer.as_ref().map(|receipt| receipt.current)
                );
            }
            let decoded: api::AbstractSummary =
                serde_json::from_slice(&serde_json::to_vec(mapped).unwrap()).unwrap();
            assert_eq!(decoded, *mapped);
        }
    }
}
