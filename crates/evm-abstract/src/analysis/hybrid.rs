//! Paid process closure first, then bounded native detail of the original input.
//!
//! Both phases use the existing kernel and one cumulative ledger/counter set.
//! Detail never imports an erased process summary. Its partial graph is separate
//! evidence and cannot invalidate or silently replace the certified baseline.

mod input;
mod policy;
mod run;

use super::publication::ModelHandle;
use super::{EvmMachineState, EvmWorldAnalysis, Status};
use crate::domain::WordElement;
use serde::{Serialize, Serializer, ser::SerializeStruct};

/// Optional detail is bounded by this allowance and all original execution
/// state/context/transfer limits. `usize::MAX` permits the remaining root work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct HybridConfig {
    /// Maximum additional work, including original-input copy and certification.
    pub max_detail_work: usize,
    /// At most this many frozen captured-code regions retain native context.
    /// All other regions erase optional history before choosing actual keys.
    pub max_detail_regions: usize,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            max_detail_work: usize::MAX,
            max_detail_regions: 1,
        }
    }
}

/// Captured code chosen for bounded native context precision. Owners, callers,
/// modes and rollback state remain distinct in the original full machine key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct HybridRegion {
    /// Declared input or observed physical executable, independent of owner.
    pub source: super::ProcessSource,
    /// Kernel matching coordinate, including a conditional root namespace.
    /// This is not an observed account; public labels use `source` instead.
    #[serde(skip)]
    pub code_address: crate::Address,
    /// Complete identity of the captured runtime bytes, not a short generation.
    pub code_hash: alloy_primitives::B256,
}

/// A local observation does not certify an exhaustive regional relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HybridLocalScope {
    /// Current transfer prefix of exactly the retained native abstract entry.
    /// Pending predecessors may grow it; uninstalled successors remain covered
    /// by the baseline. Started/OperandsConsumed/Dispatched remain distinct.
    RetainedInputPrefix,
    /// Symbolic-root observations additionally condition on the retained native
    /// owner namespace; they do not cover every original logical-owner alias.
    NativeOwnerNamespacePrefix,
}

/// Native kernel currency and original owner-binding coverage are distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HybridNativeBinding {
    /// Original concrete owner agrees with the native entry namespace.
    OriginalOwner,
    /// Additional condition: the logical root owner equals this retained native
    /// namespace. This is an imposed subset binding, not an observed physical
    /// destination, and does not establish all original symbolic-root aliases.
    SymbolicOwnerNamespace {
        /// Native root coordinate used as the explicit additional owner binding.
        namespace: crate::Address,
    },
}

/// Which complete interpretation supplies the selected control/effect relation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum HybridInterpretation {
    /// The process baseline; its own frontiers/status remain authoritative.
    Process,
    /// Closed process coverage with bounded current native local observations.
    /// These never exclude a baseline route or imply whole-native closure.
    ProcessWithDetail,
    /// Current Native detail under the original binding, additionally qualified
    /// for complete known-code control without an Environment omission.
    Native,
}

/// Detail qualification is independent of the process baseline's completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DetailState {
    /// No work was requested for optional precision.
    Disabled,
    /// The closed global model is published before optional Native preparation.
    Pending,
    /// Baseline has a frontier or lacks a current source/closure certificate.
    BaselinePending,
    /// Caller supplied process policies rather than a native precision policy.
    ProcessPolicy,
    /// Original-input copy could not be completed within the detail allowance.
    Work,
    /// Native detail retains its actual incomplete frontiers or stale evidence.
    Incomplete,
    /// Native detail is closed and current under its explicit input binding.
    /// Complete known-code control and original binding are separate conditions
    /// for selecting it instead of Process.
    Current,
}

/// Actual accepted charges, partitioned without making a second work ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HybridWork {
    /// Accepted work already spent by the same session before this invocation.
    pub prior: usize,
    /// Coarse execution and its currentness check.
    pub baseline: usize,
    /// Copy of original world/entry and bounded local-site publication invoice.
    pub preparation: usize,
    /// Native execution and currentness check, excluding input copy.
    pub detail: usize,
    /// Effective upper bound for preparation + detail after the coarse phase.
    pub detail_allowance: usize,
    /// Sum of all actual-spent fields; at most ExecutionConfig.max_work.
    pub total: usize,
}

/// Source-qualified coverage; a partial detail never certifies the baseline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HybridCoverage {
    /// Baseline kernel closure and all retained procedure source guards hold.
    pub baseline_current: bool,
    /// Native kernel closure and source guards hold under `detail_binding`.
    /// This alone does not establish original binding or complete known-code
    /// control, and therefore does not authorize whole-native substitution.
    pub detail_current: bool,
    /// Qualification of the original root owner in native preparation.
    pub detail_binding: HybridNativeBinding,
    /// Current native sites layered on the baseline, even with partial detail.
    pub local_detail_sites: usize,
    /// Frozen region choices actually applied by native partition admission.
    pub detail_regions: Vec<HybridRegion>,
    /// Complete relation selected for normal control/effect consumers.
    pub chosen: HybridInterpretation,
    /// Why detail was selected, withheld or left partial.
    pub detail: DetailState,
    /// Original requested cap, before remaining-work admission.
    pub requested_detail_work: usize,
    /// Original source-region bound, independent of the actually admitted list.
    pub requested_detail_regions: usize,
    /// Original explicit auxiliary option; produced coverage, if any, belongs
    /// only to detail and never establishes either phase's completion.
    pub requested_native_terminal_coverage: bool,
    /// Actual shared-ledger accounting.
    pub work: HybridWork,
}

/// Separate native reports prevent process action receipts from becoming PCs or
/// SSA. Snapshot, fact revision and source identities are preserved across both.
#[derive(Debug)]
pub struct HybridAnalysis<N: WordElement> {
    baseline: ModelHandle<N>,
    detail: Option<EvmWorldAnalysis<N>>,
    coverage: HybridCoverage,
}

/// Borrowed current native control for exactly one retained abstract entry.
/// It can grow if a pending predecessor is completed; it cannot exclude a
/// baseline edge or stand for all calls reaching this bytecode region. IDs and
/// successors belong exclusively to `HybridAnalysis::detail()`.
#[derive(Debug, Serialize)]
pub struct HybridLocalDetail<'a, N: WordElement> {
    /// Explicit qualification, including when the full detail graph is partial.
    pub scope: HybridLocalScope,
    /// Explicit original-owner or additional namespace binding of this prefix.
    pub binding: HybridNativeBinding,
    /// Actual native state and its retained input; no fabricated process PC.
    #[serde(skip)]
    pub state: &'a EvmMachineState<N>,
    /// Native detail state ID, separate from the baseline namespace.
    pub state_id: usize,
    /// Actual source/owner/static/stack/context coordinates for this entry.
    pub frame: &'a super::FrameKey,
    /// Instruction PCs actually visited by this receipt, in order.
    pub executed_pcs: &'a [usize],
    /// True when this code region retained the caller's bounded native context.
    /// Other frames have zero optional jump history in their actual input/key.
    pub context_selected: bool,
    /// Actual phases from this state's current receipt, in visited-PC order.
    pub instructions: &'a [super::InstructionProgress],
    /// Installed successors from this current receipt, not accumulated edges.
    pub successors: &'a [(usize, super::MachineEdgeKind)],
    /// The identical original snapshot used by both phases.
    pub snapshot: &'a super::layers::SnapshotId,
    /// Original initial-fact revision, preserved by move across both phases.
    pub revision: super::layers::FactRevision,
    #[serde(skip)]
    frontiers: &'a [super::EvmMachineFrontier<N>],
}

impl<N: WordElement> HybridLocalDetail<'_, N> {
    /// Actual native frontiers originating at this state; absence alone does
    /// not prove that all predecessors of its retained entry have completed.
    pub fn frontiers(&self) -> impl Iterator<Item = &super::EvmMachineFrontier<N>> {
        self.frontiers
            .iter()
            .filter(|frontier| frontier.from == Some(self.state.id))
    }
}

impl<N: WordElement> Serialize for HybridAnalysis<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Local<'a, N: WordElement>(&'a HybridAnalysis<N>);
        impl<N: WordElement> Serialize for Local<'_, N> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_seq(self.0.local_detail())
            }
        }
        let mut report = serializer.serialize_struct("HybridAnalysis", 4)?;
        report.serialize_field("baseline", &self.baseline)?;
        report.serialize_field("detail", &self.detail)?;
        report.serialize_field("coverage", &self.coverage)?;
        report.serialize_field("local_detail", &Local(self))?;
        report.end()
    }
}

fn local<N: WordElement>(state: &EvmMachineState<N>) -> bool {
    state.active().process.is_none()
        && !state.executed_pcs.is_empty()
        && state
            .execution_evidence()
            .is_some_and(|evidence| evidence.is_current())
}

impl<N: WordElement> HybridAnalysis<N> {
    /// An outer RPC adapter first runs only Global, then admits Native under
    /// its retained payer. Qualify that initial publication with the outer
    /// request before exposing it; the immutable graph itself never changes.
    pub(in crate::analysis) fn defer_detail(&mut self, requested: HybridConfig) {
        debug_assert!(self.detail.is_none());
        self.coverage.requested_detail_work = requested.max_detail_work;
        self.coverage.requested_detail_regions = requested.max_detail_regions;
        if self.coverage.baseline_current {
            self.coverage.detail =
                if requested.max_detail_work == 0 || requested.max_detail_regions == 0 {
                    DetailState::Disabled
                } else {
                    DetailState::Pending
                };
        }
    }

    /// Retain this graph independently of optional precision and worker lifetime.
    /// Its snapshot, revision, root binding and source evidence remain unchanged.
    pub fn baseline_handle(&self) -> ModelHandle<N> {
        self.baseline.clone()
    }

    /// Conservative process graph, with real action/callback dependencies.
    pub fn baseline(&self) -> &EvmWorldAnalysis<N> {
        &self.baseline
    }
    /// Optional native graph, including partial execution and original frontiers.
    pub fn detail(&self) -> Option<&EvmWorldAnalysis<N>> {
        self.detail.as_ref()
    }
    /// Coverage and accounting for both phases of the same root request.
    pub fn coverage(&self) -> &HybridCoverage {
        &self.coverage
    }
    /// Outer machine instruction receipts, available independently of
    /// whole-detail completion. Each state's receipt supplies its current
    /// instruction phases and installed successors. Unselected relative callees
    /// use `regional_cfg` in `detail()`'s `procedure_summaries()` records. Qualify
    /// each record by `version`/`source_current` and each transfer by `current`
    /// and `input_revision`. IDs there are regional; block visits do not supply
    /// instruction-PC or SSA proof. Preserve the coarse graph.
    pub fn local_detail(&self) -> impl Iterator<Item = HybridLocalDetail<'_, N>> {
        self.detail.iter().flat_map(|detail| {
            detail
                .states()
                .iter()
                .filter(|state| local(state))
                .take(self.coverage.local_detail_sites)
                .map(|state| {
                    let evidence = state
                        .execution_evidence()
                        .expect("selected current native receipt");
                    HybridLocalDetail {
                        scope: match self.coverage.detail_binding {
                            HybridNativeBinding::OriginalOwner => {
                                HybridLocalScope::RetainedInputPrefix
                            }
                            HybridNativeBinding::SymbolicOwnerNamespace { .. } => {
                                HybridLocalScope::NativeOwnerNamespacePrefix
                            }
                        },
                        binding: self.coverage.detail_binding,
                        state,
                        state_id: state.id,
                        frame: state.active(),
                        executed_pcs: &state.executed_pcs,
                        context_selected: policy::selected(
                            &self.coverage.detail_regions,
                            state.active(),
                        ),
                        instructions: evidence.instructions(),
                        successors: evidence.successors(),
                        snapshot: detail.snapshot(),
                        revision: detail.revision(),
                        frontiers: detail.frontiers(),
                    }
                })
        })
    }
    /// Certified native detail when available; otherwise the process baseline.
    pub fn chosen(&self) -> &EvmWorldAnalysis<N> {
        match self.coverage.chosen {
            HybridInterpretation::Process | HybridInterpretation::ProcessWithDetail => {
                &self.baseline
            }
            HybridInterpretation::Native => self
                .detail
                .as_ref()
                .expect("current native detail is retained"),
        }
    }
    /// Complete current control exists iff at least the baseline is certified.
    pub fn status(&self) -> Status {
        if self.coverage.baseline_current {
            Status::Converged
        } else {
            Status::Incomplete
        }
    }
    /// Actual cumulative work, including refused detail's successful prefix.
    pub fn work(&self) -> usize {
        self.coverage.work.total
    }
    /// Actual cumulative transfers, including earlier use of the shared counters.
    /// Native retains that counter's latest snapshot, which already includes
    /// the baseline. Without Native execution the baseline snapshot is latest.
    pub fn transfers(&self) -> usize {
        self.detail().unwrap_or_else(|| self.baseline()).transfers()
    }
    /// Transfer ownership to applications without cloning either native graph.
    pub fn into_parts(self) -> (ModelHandle<N>, Option<EvmWorldAnalysis<N>>, HybridCoverage) {
        (self.baseline, self.detail, self.coverage)
    }
}
