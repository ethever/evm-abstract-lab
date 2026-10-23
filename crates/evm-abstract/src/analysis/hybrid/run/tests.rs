use crate::analysis::{
    AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, HybridConfig,
    cooperation::{CooperationError, NoExtension},
    engine,
    machine::fixtures,
    transfer::StorageReadPolicy,
};
use crate::{
    domain::{ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};

#[test]
fn metered_hybrid_retains_prior_actual_work_and_rejects_sticky_rescue() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut budget = WorkBudget::new(20_000_000);
    assert!(budget.charge(13));
    let mut counters = engine::Counters::default();
    let report = engine
        .run_hybrid_metered(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig::default(),
            HybridConfig {
                max_detail_work: 0,
                ..HybridConfig::default()
            },
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
        .unwrap();
    assert!(report.coverage().baseline_current);
    let work = report.coverage().work;
    assert_eq!(work.prior, 13);
    assert_eq!(
        work.total,
        work.prior + work.baseline + work.preparation + work.detail
    );
    assert_eq!(report.baseline().work(), budget.used());
    assert_eq!(budget.used(), work.total);
    let accepted = budget.used();
    assert!(!budget.charge(20_000_000));
    let old_states = counters.states;
    let old_transfers = counters.transfers;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let refused = engine.run_hybrid_metered(
        AnalysisInput::new(world, entry).unwrap(),
        ExecutionConfig::default(),
        HybridConfig::default(),
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(matches!(
        refused,
        Err(CooperationError::Protocol(
            analysis_kernel::KernelError::Stopped(_)
        ))
    ));
    assert_eq!(budget.used(), accepted);
    assert_eq!(
        (counters.states, counters.transfers),
        (old_states, old_transfers)
    );
}
