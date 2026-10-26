//! Application result preserves the selected relation and optional observations.
use evm_abstract::{
    analysis::{EvmWorldAnalysis, HybridAnalysis, RpcHybridAnalysis, Status},
    domain::WordElement,
};

#[derive(Debug)]
pub(crate) enum WorldAnalysis<N: WordElement> {
    Hybrid(Box<HybridAnalysis<N>>),
    Rpc(Box<RpcHybridAnalysis<N>>),
}

impl<N: WordElement> From<HybridAnalysis<N>> for WorldAnalysis<N> {
    fn from(report: HybridAnalysis<N>) -> Self {
        Self::Hybrid(Box::new(report))
    }
}
impl<N: WordElement> From<RpcHybridAnalysis<N>> for WorldAnalysis<N> {
    fn from(report: RpcHybridAnalysis<N>) -> Self {
        Self::Rpc(Box::new(report))
    }
}

impl<N: WordElement> WorldAnalysis<N> {
    pub(crate) fn native_chosen(&self) -> bool {
        matches!(self, Self::Hybrid(report) if report.coverage().chosen == evm_abstract::analysis::HybridInterpretation::Native)
    }
    pub(crate) fn chosen(&self) -> &EvmWorldAnalysis<N> {
        match self {
            Self::Hybrid(report) => report.chosen(),
            Self::Rpc(report) => report.chosen(),
        }
    }
    pub(crate) fn status(&self) -> Status {
        match self {
            Self::Hybrid(report) => report.status(),
            Self::Rpc(report) => report.status(),
        }
    }

    /// Optional native companion has its own IDs, currentness and SSA. It is
    /// never substituted for the original global relation.
    pub(crate) fn companion_detail(&self) -> Option<&EvmWorldAnalysis<N>> {
        match self {
            Self::Hybrid(report) => (report.coverage().chosen
                != evm_abstract::analysis::HybridInterpretation::Native)
                .then(|| report.detail())
                .flatten(),
            Self::Rpc(report) => report.detail(),
        }
    }
}

pub(crate) fn text<N: WordElement>(source: &HybridAnalysis<N>) -> String {
    let coverage = source.coverage();
    let mut text = format!(
        "Global coverage: {:?} · {:?}\nDetailed paths: {:?} · {:?} · {} current input prefixes\nDetail regions: {} requested · {} selected\nNative terminal auxiliary requested: {}\nWork: {} total · {} global · {} preparation · {} detail\n",
        source.status(),
        coverage.chosen,
        coverage.detail,
        coverage.detail_binding,
        coverage.local_detail_sites,
        coverage.requested_detail_regions,
        coverage.detail_regions.len(),
        coverage.requested_native_terminal_coverage,
        source.work(),
        coverage.work.baseline,
        coverage.work.preparation,
        coverage.work.detail
    );
    text.push_str(&prefixes(source));
    text.push_str(&pending(source));
    text.push_str("Selected control/effect graph:\n");
    text
}

pub(crate) fn rpc_text<N: WordElement>(source: &RpcHybridAnalysis<N>) -> String {
    let rpc = source.coverage();
    let precision = source
        .detail_report()
        .unwrap_or(source.baseline())
        .coverage();
    let mut text = format!(
        "Global coverage: {:?} · {:?}\nOriginal facts {} · current facts {} · detailed facts {:?}\nOptional precision: {:?} · {} work / {} allowance · {} total work\n",
        source.status(),
        rpc.baseline_relation,
        rpc.baseline_revision.value(),
        rpc.current_revision.value(),
        rpc.detail_revision.map(|revision| revision.value()),
        rpc.precision,
        rpc.precision_work,
        rpc.max_precision_work,
        rpc.work
    );
    text.push_str(&format!(
        "Detail regions: {} requested · {} selected\n",
        precision.requested_detail_regions,
        precision.detail_regions.len()
    ));
    text.push_str(&format!(
        "RPC acquisition: rounds={} | requests={} | fetched accounts={:?} | fetched storage={:?}\n",
        rpc.acquisition.rounds,
        rpc.acquisition.requests,
        rpc.acquisition.fetched_accounts,
        rpc.acquisition.fetched_storage
    ));
    if let Some(detail) = source.detail_report() {
        text.push_str(&prefixes(detail));
        text.push_str(&pending(detail));
    }
    if let Some(failure) = source.preparation_failure() {
        text.push_str(&format!("Precision preparation: {failure}\n"));
    }
    for failure in source.failures() {
        text.push_str(&format!("RPC precision: {failure}\n"));
    }
    text.push_str(
        "Original global control/effect graph (source guards keep their original revision):\n",
    );
    text
}

fn prefixes<N: WordElement>(source: &HybridAnalysis<N>) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for prefix in source.local_detail() {
        writeln!(&mut text, "  Native state {} · {:?} · source {} / {} · owner namespace {} · context {:?}{} · facts {}",
            prefix.state_id, prefix.scope, prefix.frame.code_address, prefix.frame.code_hash,
            prefix.frame.address, prefix.frame.jump_history,
            if prefix.context_selected { " (selected source)" } else { " (joined context)" },
            prefix.revision.value()).expect("writing to a String is infallible");
        if let evm_abstract::analysis::HybridNativeBinding::SymbolicOwnerNamespace { namespace } =
            prefix.binding
        {
            writeln!(
                &mut text,
                "    Additional condition: root ADDRESS = {namespace}"
            )
            .expect("writing to a String is infallible");
        }
        for (pc, phase) in prefix.executed_pcs.iter().zip(prefix.instructions) {
            writeln!(&mut text, "    PC 0x{pc:x} · {phase:?}")
                .expect("writing to a String is infallible");
        }
        for (state, kind) in prefix.successors {
            writeln!(
                &mut text,
                "    Installed -> Native state {state} · {kind:?}"
            )
            .expect("writing to a String is infallible");
        }
        for frontier in prefix.frontiers() {
            writeln!(&mut text, "    Pending: {:?}", frontier.reason)
                .expect("writing to a String is infallible");
        }
    }
    text
}

fn pending<N: WordElement>(source: &HybridAnalysis<N>) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    for frontier in source
        .detail()
        .into_iter()
        .flat_map(|detail| detail.frontiers())
    {
        writeln!(
            &mut text,
            "Pending native detail: state {:?} · PC {:?} · {:?}",
            frontier.from, frontier.pc, frontier.reason
        )
        .expect("writing to a String is infallible");
    }
    text
}
