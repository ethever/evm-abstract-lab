use super::{AnalysisNamespace, AnalysisReport, Kind, ReportValidationFailure, invalid};
use crate::{HybridInterpretation, HybridReport, RpcBaselineRelation};

impl AnalysisReport {
    pub(in super::super) fn validate_rpc_hybrid(
        &self,
        hybrid: &HybridReport,
        max_work: u64,
        complete_export: bool,
    ) -> Result<(), ReportValidationFailure> {
        let Some(rpc) = &hybrid.rpc else {
            if hybrid.precision.is_some() || hybrid.coverage.work.total != self.work {
                return Err(invalid(Kind::Work));
            }
            return Ok(());
        };
        let original = &hybrid.coverage;
        if hybrid.namespace() != AnalysisNamespace::Baseline
            || original.chosen != HybridInterpretation::Process
            || original.snapshot != rpc.snapshot
            || original.baseline_revision != rpc.baseline_revision
            || (complete_export && hybrid.precision.is_some() != self.native.is_some())
        {
            return Err(invalid(Kind::Source));
        }
        if let Some(precision) = &hybrid.precision {
            if precision.snapshot != rpc.snapshot
                || precision.detail_revision != rpc.detail_revision
                || precision.work.total > rpc.work
            {
                return Err(invalid(Kind::Source));
            }
        } else if rpc.detail_revision.is_some() {
            return Err(invalid(Kind::Source));
        }
        if rpc.work != self.work
            || rpc.work > max_work
            || original.work.total > rpc.work
            || rpc.precision_work > rpc.max_precision_work.0
            || rpc.precision_work > rpc.work
        {
            return Err(invalid(Kind::Work));
        }
        if rpc.detail_at_current_revision != (rpc.detail_revision == Some(rpc.current_revision)) {
            return Err(invalid(Kind::Source));
        }
        let mut revision = rpc.baseline_revision;
        for restriction in &rpc.restrictions {
            if restriction.previous != revision {
                return Err(invalid(Kind::Source));
            }
            revision = restriction.current;
        }
        if revision != rpc.current_revision
            || match rpc.baseline_relation {
                RpcBaselineRelation::Uncertified => original.baseline_current,
                RpcBaselineRelation::SameRevision => {
                    !original.baseline_current
                        || rpc.current_revision != rpc.baseline_revision
                        || !rpc.restrictions.is_empty()
                }
                RpcBaselineRelation::InitialFactRestriction => {
                    !original.baseline_current
                        || rpc.current_revision == rpc.baseline_revision
                        || rpc.restrictions.is_empty()
                }
            }
        {
            return Err(invalid(Kind::Coverage));
        }
        if rpc
            .preparation_failure
            .as_ref()
            .is_some_and(|error| error.code == crate::ApiErrorCode::Cancelled)
            || rpc
                .failures
                .iter()
                .any(|failure| failure.kind == crate::RpcFailureKind::Cancelled)
        {
            return Err(invalid(Kind::Coverage));
        }
        Ok(())
    }
}
