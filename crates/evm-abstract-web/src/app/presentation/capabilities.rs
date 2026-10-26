//! Availability comes from retained evidence, independently of decoded source.

use super::{Capabilities, Capability, Presentation, UnavailableReason};
use crate::app::model::navigation::{ContextFocus, Focus, NavigationError, SourceFocus};
use evm_abstract_protocol::{AnalysisNamespace, BlockCoverage, CodeKind};

impl<'a> Presentation<'a> {
    pub(crate) fn capabilities(&self, focus: &Focus) -> Result<Capabilities, NavigationError> {
        self.validate(focus)?;
        let code = if focus.source.as_ref().is_some_and(|source| match source {
            SourceFocus::Captured(_) => true,
            SourceFocus::Uncaptured { source, .. } => self.report.process_program(source).is_some(),
        }) {
            Capability::Available
        } else {
            Capability::Unavailable(UnavailableReason::NoCapturedSource)
        };
        let mut result = Capabilities {
            code,
            cfg: Capability::Unavailable(UnavailableReason::SelectContext),
            native_state: Capability::Unavailable(UnavailableReason::SelectContext),
            ssa: Capability::Unavailable(UnavailableReason::SelectContext),
        };
        match focus.context {
            Some(ContextFocus::Minimum { .. }) => {
                result.cfg = Capability::Available;
                result.native_state =
                    Capability::Unavailable(UnavailableReason::ConservativeEvidence);
                result.ssa = Capability::Unavailable(UnavailableReason::ConservativeEvidence);
            }
            Some(ContextFocus::Graph { state }) => {
                result.cfg = Capability::Available;
                let graph = self.report.graph(focus.view.namespace()).unwrap();
                let positions = self.index.graph(focus.view.namespace()).unwrap();
                let block = &graph.cfg[positions.cfg[&state]];
                let ssa = positions
                    .ssa
                    .get(&state)
                    .and_then(|&index| graph.ssa.blocks.get(index));
                result.native_state = if focus.view.namespace() != AnalysisNamespace::Native
                    || block.process.is_some()
                    || block
                        .program
                        .and_then(|id| self.program(id))
                        .is_some_and(|program| program.kind == CodeKind::AbstractEnvironment)
                {
                    Capability::Unavailable(UnavailableReason::ConservativeEvidence)
                } else if positions.states.contains_key(&state) {
                    Capability::Available
                } else {
                    Capability::Unavailable(UnavailableReason::NoNativeState)
                };
                result.ssa = if block.process.is_some()
                    || focus.view.namespace() != AnalysisNamespace::Native
                {
                    Capability::Unavailable(UnavailableReason::ConservativeEvidence)
                } else {
                    // Retained Current/Stale receipts permit browsing, without
                    // requiring latest inputs or whole-graph closure. An entry
                    // that never executed still has no execution receipt.
                    match ssa.map(|ssa| ssa.coverage) {
                        Some(BlockCoverage::Current | BlockCoverage::Stale) => {
                            Capability::Available
                        }
                        Some(BlockCoverage::Unexecuted) => {
                            Capability::Unavailable(UnavailableReason::Unexecuted)
                        }
                        None => Capability::Unavailable(UnavailableReason::NoSsaReceipt),
                    }
                };
            }
            None => {}
        }
        Ok(result)
    }

    pub(super) fn native_at_current_revision(&self) -> bool {
        self.report
            .hybrid
            .as_ref()
            .and_then(|hybrid| hybrid.rpc.as_ref())
            .is_none_or(|rpc| rpc.detail_at_current_revision)
    }
}
