//! The server owns display projection; the core publishes a typed immutable model.
//! Baseline and final export share one application allowance for the whole job.

use super::{api, report};
use evm_abstract::{
    analysis::{
        HybridAnalysis,
        control::Control,
        progress::Phase,
        publication::{ModelHandle, StagePublisher},
    },
    domain::{EvmNumericAlgebra, ValueAlgebra},
    resource::WorkBudget,
};
use std::sync::Arc;

struct Published<N: evm_abstract::domain::WordElement> {
    model: ModelHandle<N>,
    report: Arc<api::AnalysisReport>,
}

pub(super) struct Projection<'a, D: EvmNumericAlgebra> {
    request: &'a api::AnalyzeRequest,
    scope: api::AnalysisScope,
    algebra: &'a ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &'a Control,
    publisher: Option<&'a crate::jobs::Publisher>,
    budget: &'a mut WorkBudget,
    published: Option<Published<D::Element>>,
}

impl<'a, D: EvmNumericAlgebra> Projection<'a, D> {
    pub(super) fn new(
        request: &'a api::AnalyzeRequest,
        scope: api::AnalysisScope,
        algebra: &'a ValueAlgebra<D>,
        numeric_model: api::NumericModel,
        control: &'a Control,
        publisher: Option<&'a crate::jobs::Publisher>,
        budget: &'a mut WorkBudget,
    ) -> Self {
        Self {
            request,
            scope,
            algebra,
            numeric_model,
            control,
            publisher,
            budget,
            published: None,
        }
    }

    pub(super) fn finish(
        self,
        final_global: &HybridAnalysis<D::Element>,
    ) -> Option<Arc<api::AnalysisReport>> {
        self.published
            .filter(|published| published.model.same_model(&final_global.baseline_handle()))
            .map(|published| published.report)
    }
}

impl<D: EvmNumericAlgebra> StagePublisher<D::Element> for Projection<'_, D> {
    fn publish(&mut self, model: &HybridAnalysis<D::Element>) {
        let Some(publisher) = self.publisher else {
            return;
        };
        // Projection has its own persistent application allowance. An optional
        // display failure cannot poison execution or revoke model closure.
        // Final projection returns its own error if delivery remains impossible.
        if let Ok(mut report) = report::hybrid::project_with_budget(
            model,
            self.request,
            self.scope,
            self.algebra,
            self.numeric_model,
            self.control,
            self.budget,
        ) {
            report.timings = super::timing::project(self.control.observer().timing(), 0);
            if self.budget.charge(2) {
                report.postprocessing_work = self.budget.used() as u64;
                let report = Arc::new(report);
                if publisher.publish_shared(Arc::clone(&report)) {
                    self.published = Some(Published {
                        model: model.baseline_handle(),
                        report,
                    });
                }
            }
        }
        self.control.observer().phase(Phase::Analyzing);
    }
}
