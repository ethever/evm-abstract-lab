//! Native termination evidence, independent of display folding and filters.
//!
//! Root outcomes describe whole executions. Only an edge that pops exactly one
//! child frame describes frame completion; a rejected CALL can instead resume
//! at the same depth. An empty outgoing set proves no termination, and a
//! non-current receipt keeps even observed termination evidence neutral.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use evm_abstract_protocol::{AnalysisReport, BlockCoverage, EdgeKind, OutcomeKind};

/// Counts native states with each kind, rather than outcome payloads or edges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TerminationCounts {
    pub returns: usize,
    pub reverts: usize,
    pub failures: usize,
}

impl TerminationCounts {
    fn add(&mut self, kinds: Kinds) {
        self.returns += usize::from(kinds.returns);
        self.reverts += usize::from(kinds.reverts);
        self.failures += usize::from(kinds.failures);
    }

    fn has_any(self) -> bool {
        self.returns > 0 || self.reverts > 0 || self.failures > 0
    }
}

/// Each condition counts member states and can overlap the ending counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TerminationSummary {
    pub members: usize,
    pub root: TerminationCounts,
    pub frame: TerminationCounts,
    /// Members with at least one root or child-frame ending.
    pub ending: usize,
    /// Members with at least one observed edge that does not end their frame.
    pub continuing: usize,
    /// Members with a frontier or a malformed, dangling native edge.
    pub unresolved: usize,
    /// Members whose execution receipt is stale, unexecuted, or absent.
    pub non_current: usize,
    /// Members without either an ending or an observed continuation.
    pub unobserved: usize,
}

impl TerminationSummary {
    pub(super) fn has_termination(self) -> bool {
        self.root.has_any() || self.frame.has_any()
    }

    /// A single fill is justified only when every member ends in that kind and
    /// there is no observed continuation or unresolved/currentness boundary.
    pub(super) fn single_kind(self) -> Option<OutcomeKind> {
        if self.members == 0
            || self.ending != self.members
            || self.continuing > 0
            || self.unresolved > 0
            || self.non_current > 0
            || self.unobserved > 0
        {
            return None;
        }
        match (
            self.root.returns > 0 || self.frame.returns > 0,
            self.root.reverts > 0 || self.frame.reverts > 0,
            self.root.failures > 0 || self.frame.failures > 0,
        ) {
            (true, false, false) => Some(OutcomeKind::Return),
            (false, true, false) => Some(OutcomeKind::Revert),
            (false, false, true) => Some(OutcomeKind::Failure),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Kinds {
    returns: bool,
    reverts: bool,
    failures: bool,
}

impl Kinds {
    fn add(&mut self, kind: OutcomeKind) {
        match kind {
            OutcomeKind::Return => self.returns = true,
            OutcomeKind::Revert => self.reverts = true,
            OutcomeKind::Failure => self.failures = true,
        }
    }

    fn has_any(self) -> bool {
        self.returns || self.reverts || self.failures
    }
}

#[derive(Clone, Copy, Default)]
struct Evidence {
    root: Kinds,
    frame: Kinds,
    continuing: bool,
    unresolved: bool,
    non_current: bool,
}

/// One immutable-report cache; sparse native IDs never become vector offsets.
#[derive(Default)]
pub(super) struct Terminations {
    states: BTreeMap<usize, Evidence>,
}

impl Terminations {
    pub(super) fn new(report: &AnalysisReport) -> Self {
        let depths: BTreeMap<_, _> = report
            .cfg
            .iter()
            .map(|block| (block.id, block.frame_depth))
            .collect();
        let coverage: BTreeMap<_, _> = report
            .ssa
            .blocks
            .iter()
            .map(|block| (block.state, block.coverage))
            .collect();
        let mut states: BTreeMap<_, _> = report
            .cfg
            .iter()
            .map(|block| {
                (
                    block.id,
                    Evidence {
                        non_current: coverage.get(&block.id) != Some(&BlockCoverage::Current),
                        ..Evidence::default()
                    },
                )
            })
            .collect();
        for outcome in &report.outcomes {
            if let Some(evidence) = states.get_mut(&outcome.state) {
                evidence.root.add(outcome.kind);
            }
        }
        for edge in &report.edges {
            let (Some(&source_depth), Some(&target_depth)) =
                (depths.get(&edge.from), depths.get(&edge.to))
            else {
                // Either observed endpoint may otherwise look purely terminal.
                for state in [edge.from, edge.to] {
                    if let Some(evidence) = states.get_mut(&state) {
                        evidence.unresolved = true;
                    }
                }
                continue;
            };
            let evidence = states.get_mut(&edge.from).expect("indexed native source");
            let kind = match edge.kind {
                EdgeKind::Return | EdgeKind::SummaryReturn => Some(OutcomeKind::Return),
                EdgeKind::Revert | EdgeKind::SummaryRevert => Some(OutcomeKind::Revert),
                EdgeKind::Failure | EdgeKind::SummaryFailure => Some(OutcomeKind::Failure),
                EdgeKind::Fallthrough
                | EdgeKind::Jump
                | EdgeKind::BranchTrue
                | EdgeKind::BranchFalse
                | EdgeKind::Call => None,
            };
            if let Some(kind) =
                kind.filter(|_| source_depth > 1 && target_depth == source_depth - 1)
            {
                evidence.frame.add(kind);
            } else {
                evidence.continuing = true;
            }
        }
        for frontier in &report.frontiers {
            if let Some(evidence) = frontier.from.and_then(|state| states.get_mut(&state)) {
                evidence.unresolved = true;
            }
        }
        Self { states }
    }

    pub(super) fn summary(&self, members: &[usize]) -> TerminationSummary {
        let mut summary = TerminationSummary::default();
        let mut seen = BTreeSet::new();
        for &state in members {
            if !seen.insert(state) {
                continue;
            }
            summary.members += 1;
            let Some(evidence) = self.states.get(&state) else {
                summary.unresolved += 1;
                summary.non_current += 1;
                summary.unobserved += 1;
                continue;
            };
            let ending = evidence.root.has_any() || evidence.frame.has_any();
            summary.root.add(evidence.root);
            summary.frame.add(evidence.frame);
            summary.ending += usize::from(ending);
            summary.continuing += usize::from(evidence.continuing);
            summary.unresolved += usize::from(evidence.unresolved);
            summary.non_current += usize::from(evidence.non_current);
            summary.unobserved += usize::from(!ending && !evidence.continuing);
        }
        summary
    }
}
