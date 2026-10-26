use super::{ModelHandle, StagePublisher};
use crate::{
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, HybridAnalysis,
        HybridConfig, Status,
        control::Control,
        cooperation::{CooperationError, NoExtension},
        machine::fixtures,
    },
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
};

#[derive(Default)]
struct Retain {
    model: Option<ModelHandle<NumericValue>>,
    calls: usize,
    cancel: Option<Control>,
}

impl StagePublisher<NumericValue> for Retain {
    fn publish(&mut self, report: &HybridAnalysis<NumericValue>) {
        self.calls += 1;
        assert_eq!(
            self.calls, 1,
            "only the closed baseline is emitted during execution"
        );
        assert_eq!(report.status(), Status::Converged);
        assert!(report.coverage().baseline_current);
        assert!(report.detail().is_none());
        assert_eq!(report.coverage().work.preparation, 0);
        assert_eq!(report.coverage().work.detail, 0);
        self.model = Some(report.baseline_handle());
        if let Some(control) = &self.cancel {
            control.cancellation().cancel();
        }
    }
}

#[test]
fn baseline_is_retained_before_native_preparation_and_shared_after_completion() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = fixtures::fixture(&[(0x101, "600160020100")]);
    let input = AnalysisInput::new(world, entry).unwrap();
    let snapshot = input.snapshot().clone();
    let revision = input.revision();
    let mut publisher = Retain::default();
    let report = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            input,
            ExecutionConfig::default(),
            HybridConfig::default(),
            &Control::default(),
            &mut publisher,
        )
        .unwrap();
    assert!(report.detail().is_some());
    assert!(report.coverage().work.preparation > 0);
    assert!(report.coverage().work.detail > 0);
    let retained = publisher.model.unwrap();
    assert!(std::ptr::eq(&*retained, report.baseline()));
    assert_eq!(retained.snapshot(), &snapshot);
    assert_eq!(retained.revision(), revision);
    assert_eq!(retained.root_binding(), report.baseline().root_binding());
    let work = retained.work();
    drop(report);
    assert_eq!(retained.work(), work);
    assert_eq!(retained.status(), Status::Converged);
}

#[test]
fn cancellation_after_publication_keeps_closed_original_model_without_native_rescue() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let control = Control::new(crate::analysis::progress::Observer::default());
    let mut publisher = Retain {
        cancel: Some(control.clone()),
        ..Default::default()
    };
    let result = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: 4_000_000_000,
                ..Default::default()
            },
            HybridConfig::default(),
            &control,
            &mut publisher,
        );
    assert!(
        matches!(result, Err(CooperationError::Cancelled)),
        "{result:?}"
    );
    let retained = publisher.model.unwrap();
    assert_eq!(retained.status(), Status::Converged);
    assert!(retained.frontiers().is_empty());
    assert!(retained.work() < 4_000_000_000);
}

#[test]
fn incomplete_baseline_never_publishes_a_closed_model() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut publisher = Retain::default();
    let report = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            AnalysisInput::new(world, entry).unwrap(),
            ExecutionConfig {
                max_work: 1,
                ..Default::default()
            },
            HybridConfig::default(),
            &Control::default(),
            &mut publisher,
        )
        .unwrap();
    assert_eq!(report.status(), Status::Incomplete);
    assert_eq!(publisher.calls, 0);
    assert!(publisher.model.is_none());
}

#[test]
fn session_publications_preserve_the_cumulative_payer_and_distinct_model_identity() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let input = AnalysisInput::new(world, entry).unwrap();
    let snapshot = input.snapshot().clone();
    let mut session = crate::analysis::layers::AnalysisSession::new(
        AnalysisEngine::new(&algebra, &policies, &mut extension),
        input,
        ExecutionConfig::default(),
    )
    .unwrap();
    let hybrid = HybridConfig {
        max_detail_work: 0,
        ..Default::default()
    };
    let mut first = Retain::default();
    let report = session
        .run_hybrid_with_publisher(hybrid, &Control::default(), &mut first)
        .unwrap();
    let prior = session.work_used();
    assert_eq!(prior, report.work());
    let mut second = Retain::default();
    let next = session
        .run_hybrid_with_publisher(hybrid, &Control::default(), &mut second)
        .unwrap();
    // Input projection also uses the retained payer before the next kernel run.
    assert!(next.coverage().work.prior > prior);
    assert!(session.work_used() > prior);
    assert_eq!(next.work(), session.work_used());
    let first = first.model.unwrap();
    let second = second.model.unwrap();
    assert_eq!(first.snapshot(), &snapshot);
    assert_eq!(second.snapshot(), &snapshot);
    assert!(
        !first.same_model(&second),
        "a new execution is a distinct immutable model"
    );
    assert!(first.same_model(&first.clone()));
}
