//! Positions into an immutable report. Native IDs are not vector offsets.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use evm_abstract_protocol::{
    AnalysisReport, CfgBlock, CfgEdge, DeferredEdge, ProcedureSummaryKind, SsaBlock, SsaTransition,
    StateDetails, SummarySourceStatus,
};

use super::termination::{TerminationSummary, Terminations};

#[derive(Default)]
pub(super) struct ReportIndex {
    cfg: BTreeMap<usize, usize>,
    ssa: BTreeMap<usize, usize>,
    details: BTreeMap<usize, usize>,
    edges: BTreeMap<usize, usize>,
    transitions: BTreeMap<usize, usize>,
    deferred: BTreeMap<usize, usize>,
    outgoing: BTreeMap<usize, Vec<usize>>,
    incoming: BTreeMap<usize, Vec<usize>>,
    frontiers: BTreeMap<usize, usize>,
    recursive_summaries: BTreeMap<usize, SummarySourceStatus>,
    known_call_summaries: BTreeMap<usize, SummarySourceStatus>,
    process_summaries: BTreeMap<usize, SummarySourceStatus>,
    environment_summaries: BTreeMap<usize, SummarySourceStatus>,
    terminations: Terminations,
}

impl ReportIndex {
    pub(super) fn new(report: &AnalysisReport) -> Self {
        let latest: BTreeMap<_, _> = report
            .abstract_summaries
            .iter()
            .enumerate()
            .map(|(index, summary)| (summary.source_state, index))
            .collect();
        let mut index = Self {
            cfg: report
                .cfg
                .iter()
                .enumerate()
                .map(|(pos, item)| (item.id, pos))
                .collect(),
            ssa: report
                .ssa
                .blocks
                .iter()
                .enumerate()
                .map(|(pos, item)| (item.state, pos))
                .collect(),
            details: report
                .states
                .iter()
                .enumerate()
                .map(|(pos, item)| (item.state, pos))
                .collect(),
            edges: report
                .edges
                .iter()
                .enumerate()
                .map(|(pos, item)| (item.id, pos))
                .collect(),
            transitions: report
                .ssa
                .transitions
                .iter()
                .enumerate()
                .map(|(pos, item)| (item.edge, pos))
                .collect(),
            deferred: report
                .ssa
                .deferred_edges
                .iter()
                .enumerate()
                .map(|(pos, item)| (item.edge, pos))
                .collect(),
            terminations: Terminations::new(report),
            ..Self::default()
        };
        // One ordering index avoids rescanning the report for every observation.
        // Only the native certificate can qualify the latest source as current.
        for (position, summary) in report.abstract_summaries.iter().enumerate() {
            let source = summary.source_status(latest[&summary.source_state] != position);
            let map = match summary.kind {
                ProcedureSummaryKind::KnownProcess => &mut index.process_summaries,
                ProcedureSummaryKind::KnownRecursion => &mut index.recursive_summaries,
                ProcedureSummaryKind::KnownCall => &mut index.known_call_summaries,
                ProcedureSummaryKind::UnknownEnvironment => &mut index.environment_summaries,
            };
            map.insert(summary.source_state, source);
        }
        for (position, edge) in report.edges.iter().enumerate() {
            index.outgoing.entry(edge.from).or_default().push(position);
            index.incoming.entry(edge.to).or_default().push(position);
        }
        for frontier in &report.frontiers {
            if let Some(state) = frontier.from {
                *index.frontiers.entry(state).or_default() += 1;
            }
        }
        index
    }

    pub(super) fn cfg<'a>(&self, report: &'a AnalysisReport, state: usize) -> Option<&'a CfgBlock> {
        report.cfg.get(*self.cfg.get(&state)?)
    }

    pub(super) fn summary_source(
        &self,
        state: usize,
        kind: ProcedureSummaryKind,
    ) -> Option<SummarySourceStatus> {
        match kind {
            ProcedureSummaryKind::KnownProcess => self.process_summaries.get(&state),
            ProcedureSummaryKind::KnownRecursion => self.recursive_summaries.get(&state),
            ProcedureSummaryKind::KnownCall => self.known_call_summaries.get(&state),
            ProcedureSummaryKind::UnknownEnvironment => self.environment_summaries.get(&state),
        }
        .copied()
    }

    pub(super) fn ssa<'a>(&self, report: &'a AnalysisReport, state: usize) -> Option<&'a SsaBlock> {
        report.ssa.blocks.get(*self.ssa.get(&state)?)
    }

    pub(super) fn details<'a>(
        &self,
        report: &'a AnalysisReport,
        state: usize,
    ) -> Option<&'a StateDetails> {
        report.states.get(*self.details.get(&state)?)
    }

    pub(super) fn edge<'a>(&self, report: &'a AnalysisReport, edge: usize) -> Option<&'a CfgEdge> {
        report.edges.get(*self.edges.get(&edge)?)
    }

    pub(super) fn transition<'a>(
        &self,
        report: &'a AnalysisReport,
        edge: usize,
    ) -> Option<&'a SsaTransition> {
        report.ssa.transitions.get(*self.transitions.get(&edge)?)
    }

    pub(super) fn deferred<'a>(
        &self,
        report: &'a AnalysisReport,
        edge: usize,
    ) -> Option<&'a DeferredEdge> {
        report.ssa.deferred_edges.get(*self.deferred.get(&edge)?)
    }

    /// Original edge positions, so parallel edges and their order are retained.
    pub(super) fn outgoing(&self, state: usize) -> &[usize] {
        self.outgoing.get(&state).map_or(&[], Vec::as_slice)
    }

    pub(super) fn incoming(&self, state: usize) -> &[usize] {
        self.incoming.get(&state).map_or(&[], Vec::as_slice)
    }

    pub(super) fn frontier_count(&self, state: usize) -> usize {
        self.frontiers.get(&state).copied().unwrap_or(0)
    }

    pub(super) fn has_frontier(&self, state: usize) -> bool {
        self.frontier_count(state) > 0
    }

    pub(super) fn termination_summary(&self, members: &[usize]) -> TerminationSummary {
        self.terminations.summary(members)
    }
}
