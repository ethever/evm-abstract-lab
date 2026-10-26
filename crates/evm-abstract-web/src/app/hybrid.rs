//! Explain global coverage separately from the progress of detailed paths.
use evm_abstract_protocol::{
    AnalysisReport, HybridDetailState, HybridInterpretation, HybridNativeBinding,
};

pub(super) fn summary(report: &AnalysisReport) -> Option<String> {
    let hybrid = report.hybrid.as_ref()?;
    let coverage = &hybrid.coverage;
    let precision = hybrid.native_coverage();
    let status = if coverage.detail == HybridDetailState::Disabled
        || coverage.requested_detail_regions == 0
    {
        format!(
            "Global coverage {} · detailed paths disabled",
            if coverage.baseline_current {
                "closed"
            } else {
                "incomplete"
            }
        )
    } else if !coverage.baseline_current {
        "Global coverage incomplete · detail awaits coverage".into()
    } else if coverage.chosen == HybridInterpretation::Native {
        "Global coverage closed · detailed paths current".into()
    } else {
        format!(
            "Global coverage closed · {} observed input prefixes · {}{}",
            hybrid.local_prefixes.len(),
            if precision.detail_current
                && hybrid
                    .rpc
                    .as_ref()
                    .is_none_or(|rpc| rpc.detail_at_current_revision)
            {
                "detailed paths current"
            } else {
                "detailed paths pending"
            },
            if matches!(
                &precision.detail_binding,
                HybridNativeBinding::SymbolicOwnerNamespace(_)
            ) {
                " · root address condition"
            } else {
                ""
            }
        )
    };
    Some(format!("{status} · {}", source_policy(report)?))
}

pub(super) fn source_policy(report: &AnalysisReport) -> Option<String> {
    let coverage = &report.hybrid.as_ref()?.coverage;
    Some(format!(
        "Detailed code sources: {} selected · limit {}",
        coverage.detail_regions.len(),
        coverage.requested_detail_regions
    ))
}
