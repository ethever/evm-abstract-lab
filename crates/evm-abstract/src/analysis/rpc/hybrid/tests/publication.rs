use super::{Server, address, entry, fixture, product_config};
use crate::{
    U256,
    analysis::{
        AnalysisEngine, EnginePolicies, HybridAnalysis, HybridConfig, RpcBaselineRelation,
        analyze_rpc_hybrid_with_publisher,
        control::Control,
        cooperation::NoExtension,
        publication::{ModelHandle, StagePublisher},
    },
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
};

#[derive(Default)]
struct Capture {
    model: Option<ModelHandle<NumericValue>>,
    calls: usize,
}

impl StagePublisher<NumericValue> for Capture {
    fn publish(&mut self, report: &HybridAnalysis<NumericValue>) {
        self.calls += 1;
        assert!(report.coverage().baseline_current);
        assert!(report.detail().is_none());
        assert_eq!(
            report.coverage().detail,
            crate::analysis::DetailState::Pending
        );
        assert_eq!(report.coverage().requested_detail_work, usize::MAX);
        self.model = Some(report.baseline_handle());
    }
}

#[test]
fn initial_rpc_publication_keeps_original_revision_across_real_storage_refinement() {
    std::thread::scope(|scope| {
        let mut fixture = fixture("5f54565b5b00");
        fixture
            .storage
            .insert((address(0x101), U256::ZERO), U256::from(4));
        let server = Server::new(scope, fixture);
        let algebra = ValueAlgebra::new(ProductAlgebra::default());
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let mut publisher = Capture::default();
        let config = product_config();
        let maximum = config.max_work;
        let report = analyze_rpc_hybrid_with_publisher(
            AnalysisEngine::new(&algebra, &policies, &mut extension),
            &server.input(),
            entry(),
            config,
            HybridConfig::default(),
            &Control::default(),
            &mut publisher,
        )
        .unwrap();
        assert_eq!(publisher.calls, 1);
        let initial = publisher.model.unwrap();
        assert!(initial.same_model(&report.baseline().baseline_handle()));
        assert_eq!(initial.snapshot(), &report.coverage().snapshot);
        assert_eq!(initial.revision(), report.coverage().baseline_revision);
        assert_ne!(initial.revision(), report.coverage().current_revision);
        assert_eq!(
            report.coverage().baseline_relation,
            RpcBaselineRelation::InitialFactRestriction
        );
        assert_eq!(server.count("eth_getStorageAt", address(0x101)), 1);
        assert!(report.work() > initial.work());
        assert!(report.work() <= maximum);
        assert!(report.coverage().precision_work <= report.coverage().max_precision_work);
    });
}
