//! Explain global coverage separately from the progress of detailed paths.
use evm_abstract_protocol::{AnalysisReport, HybridInterpretation, HybridNativeBinding};

pub(super) fn summary(report: &AnalysisReport) -> Option<String> {
    let hybrid = report.hybrid.as_ref()?;
    let coverage = &hybrid.coverage;
    let precision = hybrid.native_coverage();
    Some(if !coverage.baseline_current {
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
    })
}
