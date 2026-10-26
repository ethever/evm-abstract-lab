//! Read-only indexes over one accepted publication. Numeric rows remain in their
//! original namespace; grouping source aliases never joins their analysis facts.

mod capabilities;
mod focus;
mod index;
mod validate;

use super::{ReportGeneration, ReportViewId};
use evm_abstract_protocol::{
    AbstractProcedure, AnalysisGraph, AnalysisReport, BlockCoverage, CfgBlock, DisasmBlock,
    DisasmInstruction, FrameSnapshot, HybridNativeBinding, JobId, JobPublication,
    MinimumProcedureBounds, MinimumSummary, ProgramInfo, PublicationVersion, SummarySourceStatus,
};
pub(crate) use index::ReportIndex;

/// Shared code grouping keeps every physical source alias independently named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CodeGroup {
    pub(crate) programs: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextTarget {
    pub(crate) view: ReportViewId,
    pub(crate) context: super::navigation::ContextFocus,
}

#[derive(Default)]
pub(crate) struct ProgramRows {
    starts: Vec<usize>,
}

#[derive(Clone, Copy)]
pub(crate) enum SourceRow<'a> {
    Block(&'a DisasmBlock),
    Instruction {
        block: &'a DisasmBlock,
        instruction: &'a DisasmInstruction,
    },
}

/// HTTP publication, graph versions and producer generations are separate axes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PublicationIdentity {
    pub(crate) report: ReportGeneration,
    pub(crate) job: Option<JobId>,
    pub(crate) publication: Option<PublicationVersion>,
    pub(crate) minimum: Option<PublicationVersion>,
    pub(crate) global: Option<PublicationVersion>,
    pub(crate) native: Option<PublicationVersion>,
}

#[derive(Clone, Copy)]
pub(crate) struct Presentation<'a> {
    report: &'a AnalysisReport,
    index: &'a ReportIndex,
    view: ReportViewId,
    publication: Option<&'a JobPublication>,
}

impl<'a> Presentation<'a> {
    pub(crate) fn new(
        report: &'a AnalysisReport,
        index: &'a ReportIndex,
        view: ReportViewId,
        publication: Option<&'a JobPublication>,
    ) -> Self {
        Self {
            report,
            index,
            view,
            publication,
        }
    }

    pub(crate) fn report(self) -> &'a AnalysisReport {
        self.report
    }

    pub(crate) fn index(self) -> &'a ReportIndex {
        self.index
    }

    pub(crate) fn identity(self) -> PublicationIdentity {
        self.index.identity()
    }

    pub(crate) fn program(self, id: usize) -> Option<&'a ProgramInfo> {
        self.index.program(self.report, id)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ResolvedContext<'a> {
    Minimum {
        summary: &'a MinimumSummary,
        procedure: &'a AbstractProcedure,
        bounds: &'a MinimumProcedureBounds,
        source_status: SummarySourceStatus,
    },
    Graph {
        /// Retained input revision and SSA completeness belong to this graph,
        /// independently of whether its evidence is available for browsing.
        graph: &'a AnalysisGraph,
        state: &'a CfgBlock,
        frame: Option<&'a FrameSnapshot>,
        coverage: Option<BlockCoverage>,
        /// Input currency and local block coverage are independent of closure.
        at_current_revision: bool,
        binding: Option<&'a HybridNativeBinding>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnavailableReason {
    NoCapturedSource,
    SelectContext,
    ConservativeEvidence,
    NoNativeState,
    NoSsaReceipt,
    Unexecuted,
}

impl UnavailableReason {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NoCapturedSource => "Source bytes were not captured for this context",
            Self::SelectContext => "Select an execution context to inspect its evidence",
            Self::ConservativeEvidence => "MAY control has no Native instruction or SSA receipt",
            Self::NoNativeState => "No Native state was retained for this context",
            Self::NoSsaReceipt => "No SSA receipt was retained for this state",
            Self::Unexecuted => "This state has no executed instruction receipt",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capability {
    Available,
    Unavailable(UnavailableReason),
}

impl Capability {
    pub(crate) fn available(self) -> bool {
        self == Self::Available
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub(crate) code: Capability,
    /// Scoped analysis CFG requires an explicitly chosen execution context.
    pub(crate) cfg: Capability,
    pub(crate) native_state: Capability,
    /// Retained executed SSA is browseable even under historical/partial scope.
    pub(crate) ssa: Capability,
}

#[cfg(test)]
mod tests;
