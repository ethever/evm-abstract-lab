//! Global coverage and conditioned native observations have separate namespaces.
mod rpc;
pub use rpc::*;

choice! {
    /// Graph namespace, independent of report-local numeric state IDs.
    pub enum AnalysisNamespace {
        /// Conservative process graph.
        Baseline,
        /// Original native preparation, including pending detail.
        Native,
    }
}
choice! {
    /// Complete control/effect relation selected by the engine.
    pub enum HybridInterpretation {
        /// Conservative global process relation.
        Process,
        /// Process relation with current conditioned native observations.
        ProcessWithDetail,
        /// Current native relation with the original owner binding.
        Native,
    }
}
record! {
    /// Additional condition on logical root owner, not a physical observation.
    pub struct HybridOwnerNamespace {
        /// Retained native namespace to which this conditional view is restricted.
        pub namespace: String,
    }
}
variant! {
    /// Kernel currency does not establish logical-owner alias coverage.
    pub enum HybridNativeBinding {
        /// Native owner agrees with the concrete original owner.
        OriginalOwner,
        /// Native entry is conditioned on one retained owner namespace.
        SymbolicOwnerNamespace(HybridOwnerNamespace),
    }
}
choice! {
    /// Qualification of optional native precision.
    pub enum HybridDetailState {
        /// The Global model is closed; optional Native preparation has not started.
        Pending,
        /// No optional precision was requested.
        Disabled,
        /// Global coverage has not been certified.
        BaselinePending,
        /// The supplied policy uses process interpretation.
        ProcessPolicy,
        /// Original-input preparation could not be funded.
        Work,
        /// Native detail has pending or uncertified work.
        Incomplete,
        /// Native detail is current under its recorded owner binding.
        Current,
    }
}
choice! {
    /// Scope of a current receipt, never whole-region closure.
    pub enum HybridLocalScope {
        /// Prefix for exactly this retained native abstract input.
        RetainedInputPrefix,
        /// Prefix additionally conditioned on the native owner namespace.
        NativeOwnerNamespacePrefix,
    }
}
record! {
    /// Captured executable selected for bounded jump context.
    pub struct HybridRegion {
        /// Declared source; an input program has no observed physical account.
        pub source: crate::ProcessSource,
        /// Physical account only for Runtime, independently of the state owner.
        pub code_address: Option<String>,
        /// Full captured code hash.
        pub code_hash: String,
    }
}
record! {
    /// One cumulative engine ledger, partitioned into actual phase charges.
    pub struct HybridWork {
        /// Work accepted before the Hybrid invocation in the same session.
        pub prior: u64,
        /// Process execution and its currentness check.
        pub baseline: u64,
        /// Original input copy and bounded publication preparation.
        pub preparation: u64,
        /// Native execution and its currentness check.
        pub detail: u64,
        /// Effective cap for preparation plus native execution.
        pub detail_allowance: u64,
        /// Cumulative work including prior session charges.
        pub total: u64,
    }
}
record! {
    /// Native engine qualifications; applications do not mint closure certificates.
    pub struct HybridCoverage {
        /// Exact initial snapshot, including its offline namespace.
        pub snapshot: crate::RootTerminalSnapshot,
        /// Fact revision of the immutable process baseline.
        pub baseline_revision: crate::DecimalU64,
        /// Fact revision of the retained native graph, when present.
        pub detail_revision: Option<crate::DecimalU64>,
        /// Actual baseline policy, including its disabled auxiliary envelope.
        pub baseline_config: crate::RootTerminalConfig,
        /// Actual native phase policy, including the requested auxiliary flag.
        pub detail_config: Option<crate::RootTerminalConfig>,
        /// Global kernel closure and all final-source guards hold.
        pub baseline_current: bool,
        /// Whole native kernel and sources are current under detail_binding.
        pub detail_current: bool,
        /// Actual original-owner qualification.
        pub detail_binding: HybridNativeBinding,
        /// Actual requested auxiliary option; baseline always keeps it disabled.
        pub requested_native_terminal_coverage: bool,
        /// Current retained-input sites in the source analysis. Rich export
        /// may withhold their receipts under an explicit non-Complete outcome.
        pub local_detail_sites: usize,
        /// Frozen selected sources; other regions erase optional jump history.
        pub detail_regions: Vec<HybridRegion>,
        /// Qualified semantic interpretation, independent of selected_view.
        pub chosen: HybridInterpretation,
        /// Independent optional precision state.
        pub detail: HybridDetailState,
        /// Original requested precision allowance, losslessly encoded.
        pub requested_detail_work: crate::DecimalU64,
        /// Actual cumulative accounting.
        pub work: HybridWork,
    }
}
record! {
    /// A visited opcode and the phase actually reached.
    pub struct HybridInstruction {
        /// Actual instruction offset in the prefix's source.
        pub pc: usize,
        /// Dispatch does not imply all successors were installed.
        pub phase: crate::InstructionProgress,
    }
}
record! {
    /// One successor installed by this current receipt.
    pub struct HybridSuccessor {
        /// Destination in the Native graph namespace.
        pub state: usize,
        /// Actual edge kind.
        pub kind: crate::EdgeKind,
    }
}
record! {
    /// Source-qualified observation of exactly one retained native input.
    pub struct HybridLocalPrefix {
        /// Always Native; baseline IDs can have the same numeric value.
        pub namespace: AnalysisNamespace,
        /// Native state whose entry is the retained abstract input.
        pub state: usize,
        /// Actual scope, including any native-owner conditioning.
        pub scope: HybridLocalScope,
        /// Original root condition, retained even when this prefix is in a child.
        pub binding: HybridNativeBinding,
        /// Exact snapshot, never inferred from a display label.
        pub snapshot: crate::RootTerminalSnapshot,
        /// Fact revision of this particular native preparation.
        pub revision: crate::DecimalU64,
        /// Actual source, logical owner, caller, mode, stack and context.
        pub frame: crate::RegionalFrame,
        /// Whether this source retained the requested bounded jump history.
        pub context_selected: bool,
        /// Ordered current PCs and phases; no process PC is synthesized.
        pub instructions: Vec<HybridInstruction>,
        /// Current installed successors, separate from accumulated topology.
        pub successors: Vec<HybridSuccessor>,
        /// All frontiers originating at this state, indexing Native.frontiers.
        pub frontiers: Vec<usize>,
    }
}
record! {
    /// Qualifications and receipts; graphs have fixed envelope positions.
    pub struct HybridReport {
    /// Authoritative combined coverage and accounting.
    pub coverage: HybridCoverage,
    /// Current conditioned observations, including honest pending control.
    pub local_prefixes: Vec<HybridLocalPrefix>,
    /// Qualification of separately retained RPC native detail, if any.
    pub precision: Option<HybridCoverage>,
    /// Same-snapshot fact restrictions and persistent-session accounting.
    pub rpc: Option<RpcHybridCoverage>,
    }
}
impl HybridReport {
    /// Native observations can precede the latest accepted RPC fact revision.
    pub fn native_coverage(&self) -> &HybridCoverage {
        self.precision.as_ref().unwrap_or(&self.coverage)
    }
    /// Preferred rich namespace of the semantic interpretation. The envelope
    /// may select an available minimum or another retained rich view instead.
    pub fn namespace(&self) -> AnalysisNamespace {
        match self.coverage.chosen {
            HybridInterpretation::Native => AnalysisNamespace::Native,
            HybridInterpretation::Process | HybridInterpretation::ProcessWithDetail => {
                AnalysisNamespace::Baseline
            }
        }
    }
}
choice! {
    /// Structural contradictions in delivered Hybrid evidence.
    pub enum HybridValidationKind {
        /// Graph namespace, presence or nested Hybrid mismatch.
        Namespace,
        /// Coverage, chosen relation or owner qualification mismatch.
        Coverage,
        /// Invalid cumulative phase accounting.
        Work,
        /// Snapshot or fact revision mismatch.
        Source,
        /// Prefix does not describe the retained native entry.
        Input,
        /// Current PCs/phases or installed successor references are invalid.
        Receipt,
        /// Per-state native frontier references are missing or invalid.
        Frontier,
    }
}
