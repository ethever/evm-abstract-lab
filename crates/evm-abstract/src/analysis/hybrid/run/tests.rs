use crate::analysis::{
    AnalysisEngine, AnalysisInput, DetailState, EnginePolicies, ExecutionConfig, HybridAnalysis,
    HybridConfig,
    cooperation::{CooperationError, NoExtension},
    engine,
    machine::fixtures,
    publication::{ModelHandle, StagePublisher},
    transfer::StorageReadPolicy,
};
use crate::{
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};

mod cache;

#[derive(Default)]
struct FirstTransfers(Option<(ModelHandle<NumericValue>, usize)>);

impl StagePublisher<NumericValue> for FirstTransfers {
    fn publish(&mut self, report: &HybridAnalysis<NumericValue>) {
        assert!(self.0.is_none());
        assert!(report.detail().is_none());
        assert_eq!(
            report.coverage().work.preparation + report.coverage().work.detail,
            0
        );
        assert_eq!(report.transfers(), report.baseline().transfers());
        self.0 = Some((report.baseline_handle(), report.transfers()));
    }
}

#[test]
fn hybrid_transfers_use_the_latest_actual_shared_counter_snapshot_once() {
    for prior in [0, 7] {
        for (code, detail_work, regions, max_work, expected) in [
            ("00", 0, 1, 20_000_000, DetailState::Disabled),
            ("00", usize::MAX, 0, 20_000_000, DetailState::Disabled),
            ("00", 1, 1, 20_000_000, DetailState::Work),
            (
                "600160020100",
                usize::MAX,
                1,
                20_000_000,
                DetailState::Current,
            ),
            (
                "5f5f5f5f5f6102015af100",
                usize::MAX,
                1,
                2_000_000_000,
                DetailState::Incomplete,
            ),
            ("00", usize::MAX, 1, 1, DetailState::BaselinePending),
        ] {
            let algebra = ValueAlgebra::new(ProductAlgebra::default());
            let policies = EnginePolicies::default();
            let mut extension = NoExtension;
            let (world, entry) = fixtures::fixture(&[(0x101, code)]);
            let input = AnalysisInput::new(world, entry).unwrap();
            let snapshot = input.snapshot().clone();
            let revision = input.revision();
            let config = ExecutionConfig {
                max_work,
                ..Default::default()
            };
            let mut budget = WorkBudget::new(max_work);
            let mut counters = engine::Counters::default();
            counters.transfers = prior;
            let mut first = FirstTransfers::default();
            let report = AnalysisEngine::new(&algebra, &policies, &mut extension)
                .run_hybrid_metered_with_publisher(
                    input,
                    config.clone(),
                    HybridConfig {
                        max_detail_work: detail_work,
                        max_detail_regions: regions,
                    },
                    (&mut budget, &mut counters),
                    StorageReadPolicy::Discover,
                    &mut first,
                )
                .unwrap();
            assert_eq!(report.coverage().detail, expected);
            assert_eq!(report.transfers(), counters.transfers);
            assert_eq!(report.work(), budget.used());
            assert_eq!(report.baseline().snapshot(), &snapshot);
            assert_eq!(report.baseline().revision(), revision);
            assert_eq!(
                serde_json::to_value(&report.baseline().config().analysis).unwrap(),
                serde_json::to_value(&config.analysis).unwrap()
            );
            assert_eq!(report.coverage().requested_detail_work, detail_work);
            assert_eq!(report.coverage().requested_detail_regions, regions);
            if let Some((retained, transfers)) = first.0 {
                assert!(retained.same_model(&report.baseline_handle()));
                assert_eq!(retained.transfers(), transfers);
                assert!(transfers > prior);
            } else {
                assert_eq!(expected, DetailState::BaselinePending);
                assert_eq!(report.transfers(), prior);
            }
            if let Some(native) = report.detail() {
                assert!(native.transfers() > report.baseline().transfers());
                assert_eq!(native.transfers(), counters.transfers);
                assert_eq!(native.snapshot(), &snapshot);
                assert_eq!(native.revision(), revision);
                assert_eq!(
                    serde_json::to_value(native.config()).unwrap(),
                    serde_json::to_value(&config).unwrap()
                );
            } else {
                assert_eq!(report.baseline().transfers(), counters.transfers);
            }
            eprintln!(
                "TRANSFER_COUNTER prior={prior} detail={expected:?} global={} native={:?} total={} actual_shared={}",
                report.baseline().transfers(),
                report.detail().map(|native| native.transfers()),
                report.transfers(),
                counters.transfers,
            );
        }
    }
}

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

#[test]
fn abstract_baseline_closes_before_native_discovery_requests_unlisted_code() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let (world, entry) = fixtures::fixture(&[(0x101, "5f5f5f5f5f6102015af100")]);
    let input = AnalysisInput::new(world, entry).unwrap();
    let mut budget = WorkBudget::new(2_000_000_000);
    let mut counters = engine::Counters::default();
    let report = engine
        .run_hybrid_metered(
            input,
            ExecutionConfig {
                max_work: 2_000_000_000,
                ..ExecutionConfig::default()
            },
            HybridConfig::default(),
            &mut budget,
            &mut counters,
            StorageReadPolicy::Discover,
        )
        .unwrap();
    assert!(report.coverage().baseline_current);
    assert!(report.baseline().frontiers().is_empty());
    let native = report.detail().unwrap();
    assert_eq!(native.status(), crate::analysis::Status::Incomplete);
    assert!(
        native
            .frontiers()
            .iter()
            .any(|f| matches!(f.reason, crate::analysis::FrontierReason::MissingCode(_)))
    );
    assert_eq!(report.status(), crate::analysis::Status::Converged);
    assert_eq!(report.work(), budget.used());
    assert!(
        report.coverage().work.preparation + report.coverage().work.detail
            <= report.coverage().work.detail_allowance
    );
}

#[test]
fn every_cancellation_identity_returns_typed_error_without_normal_publication() {
    for owner in [false, true] {
        let original = analysis_control::Cancellation::new();
        let ambient = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&original, || WorkBudget::new(20_000_000));
        assert!(budget.charge(13));
        if owner {
            original.cancel();
        } else {
            ambient.cancel();
        }
        let algebra = ValueAlgebra::new(ProductAlgebra::default());
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
        let mut counters = engine::Counters::default();
        let result = analysis_control::with_cancellation(&ambient, || {
            AnalysisEngine::new(&algebra, &policies, &mut extension).run_hybrid_metered(
                AnalysisInput::new(world, entry).unwrap(),
                ExecutionConfig::default(),
                HybridConfig::default(),
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            )
        });
        assert!(matches!(result, Err(CooperationError::Cancelled)));
        assert_eq!(
            (budget.used(), counters.states, counters.transfers),
            (13, 0, 0)
        );
    }
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let control = crate::analysis::control::Control::new(Default::default());
    control.cancellation().cancel();
    assert!(matches!(
        AnalysisEngine::new(&algebra, &policies, &mut extension).run_hybrid(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig::default(),
            HybridConfig::default(),
            &control,
        ),
        Err(CooperationError::Cancelled)
    ));
}
