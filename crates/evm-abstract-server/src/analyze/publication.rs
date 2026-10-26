//! The server owns display projection; the core publishes a typed immutable model.
//! Baseline and final export share one application allowance for the whole job.

use super::{api, report};
use evm_abstract::{
    analysis::{
        HybridAnalysis,
        control::{Cancelled, Control},
        progress::Phase,
        publication::{ModelHandle, StagePublisher},
    },
    domain::{EvmNumericAlgebra, ValueAlgebra},
    resource::WorkBudget,
};
use std::sync::Arc;

pub(super) struct Options<'a> {
    pub publisher: Option<&'a crate::jobs::Publisher>,
    pub max_work: usize,
}

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
    failure: Option<api::ApiError>,
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
            failure: None,
        }
    }

    pub(super) fn finish(
        self,
        final_global: &HybridAnalysis<D::Element>,
    ) -> Result<Option<Arc<api::AnalysisReport>>, api::ApiError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        match self.published {
            Some(published) if published.model.same_model(&final_global.baseline_handle()) => {
                Ok(Some(published.report))
            }
            None if !final_global.coverage().baseline_current => Ok(None),
            _ => Err(identity_error()),
        }
    }

    /// A failed mandatory publication stops the core through its control
    /// channel. Restore the actual export/channel cause before mapping that
    /// internal stop into a public job outcome.
    pub(super) fn resolve<T>(
        &mut self,
        result: Result<T, api::ApiError>,
    ) -> Result<T, api::ApiError> {
        match self.failure.take() {
            Some(error) => Err(error),
            None => result,
        }
    }
}

fn identity_error() -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: "required minimum does not retain the final Global producer".into(),
        details: api::ErrorDetails::Report(api::ReportValidationFailure::Minimum(
            api::MinimumValidationKind::Identity,
        )),
    }
}

impl<D: EvmNumericAlgebra> StagePublisher<D::Element> for Projection<'_, D> {
    fn checkpoint(&self) -> Result<(), Cancelled> {
        if self.failure.is_some() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    fn publish(&mut self, model: &HybridAnalysis<D::Element>) {
        if self.published.is_some() || self.failure.is_some() {
            return;
        }
        // The minimum is mandatory even for synchronous callers. Remember its
        // first refusal: a smaller later charge or a rich-only export cannot
        // bypass a failed required stage on this same payer.
        let prepared = (|| {
            let mut report = report::minimum::project(
                model,
                self.algebra,
                self.control,
                self.budget,
                report::ProjectionInput {
                    request: self.request,
                    scope: self.scope,
                    numeric_model: self.numeric_model,
                },
            )?;
            report.timings = super::timing::project(self.control.observer().timing(), 0);
            if !self.budget.charge(2) {
                let minimum = report.minimum.as_ref().ok_or_else(identity_error)?;
                return Err(api::ApiError {
                    code: api::ApiErrorCode::Export,
                    message: "required minimum retention exceeds the postprocessing allowance"
                        .into(),
                    details: api::ErrorDetails::Export(Box::new(api::ExportFailure {
                        kind: api::ExportFailureKind::Work,
                        required: 2,
                        limits: minimum.export_limits,
                        usage: minimum.export_usage,
                        written: None,
                    })),
                });
            }
            report.postprocessing_work = self.budget.used() as u64;
            let report = Arc::new(report);
            if let Some(publisher) = self.publisher
                && !publisher.publish_shared(Arc::clone(&report))
            {
                return Err(crate::jobs::worker_error(
                    api::WorkerErrorKind::Channel,
                    "required minimum publication was not retained",
                ));
            }
            Ok(Published {
                model: model.baseline_handle(),
                report,
            })
        })();
        match prepared {
            Ok(published) => self.published = Some(published),
            Err(error) => self.failure = Some(error),
        }
        self.control.observer().phase(Phase::Analyzing);
    }
}

#[cfg(test)]
mod tests;
