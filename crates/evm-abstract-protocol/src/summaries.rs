//! Procedure relations and revision-qualified regional flow, separate from native CFG/SSA.

choice! {
    /// Why the main execution requested a procedure fixed point.
    pub enum ProcedureSummaryKind {
        /// Captured runtime selected for conservative process interpretation.
        KnownProcess,
        /// Unobserved execution interacting with captured bytecode.
        UnknownEnvironment,
        /// A recursive call between captured bytecode procedures.
        KnownRecursion,
        /// A known bytecode callee summarized from its first admission.
        KnownCall,
    }
}

record! {
    /// A known-code or abstract-environment relation in one joint fixed point.
    pub struct AbstractProcedure {
        /// Explicit process identity; native-only anchor fields are absent when present.
        pub process: Option<crate::ProcessFrameKey>,
        /// Actual joined process invocation; sharing a class does not imply pairwise equality.
        pub process_input: Option<crate::ProcessOperands>,
        /// Interpretation independently of captured code kind and identity.
        pub semantics: crate::TransferSemantics,
        /// Code identity; an abstract namespace is not an observed account.
        pub code_address: Option<String>,
        /// Captured code hash, without an invented unknown instruction stream.
        pub code_hash: Option<String>,
        /// Storage namespace used by the relation.
        pub storage_address: Option<String>,
        /// Original executable kind, retained under conservative interpretation.
        pub kind: crate::CodeKind,
        /// Propagated static execution restriction.
        pub is_static: bool,
        /// Cumulative historical block observations, not current-input coverage.
        pub covered_blocks: Vec<usize>,
        /// Termination categories currently represented by output bounds.
        pub outcomes: Vec<crate::OutcomeKind>,
        /// Indexes of callee relations within the containing summary.
        pub dependencies: Vec<usize>,
        /// Strict input expansions before this observation.
        pub input_updates: usize,
        /// Strict output expansions before this observation.
        pub output_updates: usize,
    }
}

record! {
    /// One summarized entry and the mutually recursive relations used to cover it.
    pub struct AbstractSummary {
        /// Native machine state requesting the relation.
        pub source_state: usize,
        /// Certified final World source evaluation. False alone does not prove
        /// this observation is old; its source may remain uncertified.
        pub source_current: bool,
        /// Actual admission reason, independently of recursive dependencies.
        pub kind: ProcedureSummaryKind,
        /// Requested root within procedures, absent if admission was interrupted.
        pub root_procedure: Option<usize>,
        /// Closure for this record's observed input snapshot, independently of
        /// whether that snapshot is certified for the final World source.
        pub closed: bool,
        /// Known and unknown execution contexts participating in the relation.
        pub procedures: Vec<AbstractProcedure>,
        /// Compact native regional evidence; IDs are local to this summary.
        pub regional_cfg: RegionalCfg,
        /// Relative block states, independently of the main CFG size.
        pub state_count: usize,
        /// Metered transfer count for this observation.
        pub transfers: usize,
        /// Output updates delivered to suspended continuations.
        pub propagations: usize,
        /// Calls sharing an existing relation entry.
        pub reused: usize,
    }
}

record! {
    /// Admitted regional topology and actual transfer/delivery receipts.
    #[derive(Default)]
    pub struct RegionalCfg {
        /// Native table node IDs, independent of machine-state and SSA IDs.
        pub nodes: Vec<RegionalNode>,
        /// Separate caller continuations subscribed to callee outputs.
        pub calls: Vec<RegionalCall>,
    }
}
record! {
    /// Body identity without payloads, instruction traces or SSA definitions.
    pub struct RegionalFrame {
        /// Explicit process identity; native-only anchor fields are absent when present.
        pub process: Option<crate::ProcessFrameKey>,
        /// Captured executable account, distinct from the state owner.
        pub code_address: Option<String>,
        /// Captured hash; absent for an abstract environment.
        pub code_hash: Option<String>,
        /// Persistent/transient state owner.
        pub storage_address: Option<String>,
        /// Logical ADDRESS, independently of the owner namespace.
        pub address_value: Option<crate::AddressValue>,
        /// Concrete or symbolic CALLER.
        pub caller: Option<crate::AddressValue>,
        /// Actual executable mode.
        pub kind: crate::CodeKind,
        /// Propagated static restriction.
        pub is_static: bool,
        /// Native runtime/initcode block index, including synthetic end-of-code;
        /// absent for process interpretations and all non-program modes.
        /// This is not a PC.
        pub basic_block: Option<usize>,
        /// Input stack shape, without invented values.
        pub stack_height: Option<usize>,
        /// Selected finite internal jump context.
        pub context: Vec<usize>,
    }
}
record! {
    /// Admission does not certify a block visit or current coverage.
    pub struct RegionalNode {
        /// Index in the containing RegionalCfg.nodes.
        pub id: usize,
        /// Index in the containing AbstractSummary.procedures.
        pub procedure: usize,
        /// This body's own source, owner, mode and block identity.
        pub frame: RegionalFrame,
        /// Input-slot version at this observation, losslessly encoded.
        pub input_revision: crate::DecimalU64,
        /// Cooperative input currently excludes this admitted body.
        pub excluded: bool,
        /// Accumulated admitted topology, not current execution evidence.
        pub successors: Vec<usize>,
        /// Latest actual transfer, absent for admitted/pending bodies.
        pub transfer: Option<RegionalTransfer>,
    }
}
record! {
    /// One actual interpreter/environment transfer at a recorded revision.
    pub struct RegionalTransfer {
        /// Actual interpretation independently of original code identity.
        pub semantics: crate::TransferSemantics,
        /// Conservative rules; absent when none were recorded by this transfer.
        pub process: Option<crate::ProcessTransfer>,
        /// Input version actually interpreted.
        pub input_revision: crate::DecimalU64,
        /// Current within this record's observation. Final-source currentness
        /// also requires AbstractSummary.source_current and no superseding record.
        pub current: bool,
        /// No obligation of this transfer was refused or deferred.
        pub complete: bool,
        /// At least one captured instruction was visited; false for processes.
        pub visited_block: bool,
        /// Successfully admitted successors of this transfer, in native order.
        pub successors: Vec<RegionalEdge>,
        /// Actual call IDs admitted by this transfer.
        pub calls: Vec<usize>,
        /// Actual emitted terminal alternatives; not output-closure proof.
        pub outcomes: Vec<crate::OutcomeKind>,
    }
}
record! {
    /// A real successor in the containing regional namespace.
    pub struct RegionalEdge {
        /// Regional destination node ID, never a machine state or PC.
        pub to: usize,
        /// Actual native transfer/delivery category.
        pub kind: crate::EdgeKind,
    }
}
record! {
    /// A retained caller continuation, distinct from its callee relation.
    pub struct RegionalCall {
        /// Actual native control scope; None does not establish primitive scope.
        pub scope: Option<crate::ProcessControlScope>,
        /// Conservative action; absent for an ordinary native subscription.
        pub action: Option<crate::ProcessAction>,
        /// Original action target, independently of executable code and state owner.
        pub target: Option<crate::ProcessOwner>,
        /// Index in the containing RegionalCfg.calls.
        pub id: usize,
        /// Caller regional node ID.
        pub from: usize,
        /// Callee procedure index in the containing summary.
        pub callee: usize,
        /// Real callee entry nodes, which may still be pending.
        pub entries: Vec<usize>,
        /// Caller-template input version at this observation.
        pub input_revision: crate::DecimalU64,
        /// Actual deliveries, possibly historical after growth.
        pub deliveries: Vec<RegionalReturn>,
    }
}
record! {
    /// A real callee-output delivery to this call's continuation.
    pub struct RegionalReturn {
        /// Actual CREATE/CREATE2 completion cases; empty for ordinary calls.
        pub creation: Vec<crate::ProcessCompletionCase>,
        /// Delivered termination alternative.
        pub kind: crate::OutcomeKind,
        /// Caller-template revision actually instantiated.
        pub input_revision: crate::DecimalU64,
        /// Callee outcome generation actually delivered.
        pub output_generation: usize,
        /// Subscriber/output stamps match within this record's observation.
        /// Final-source currentness also requires the containing source certificate.
        pub current: bool,
        /// Actual caller continuation targets, with their native edge categories.
        pub successors: Vec<RegionalEdge>,
    }
}

/// Presentation scope for one observation; this is not a new wire status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummarySourceStatus {
    /// The source evaluation is certified for the final World result.
    Current,
    /// A later observation exists for this same native source state.
    Historical,
    /// Latest observation, without a final-source certificate.
    Uncertified,
}

impl AbstractSummary {
    /// Ordering identifies superseded observations; it cannot certify the latest
    /// evaluation. Consumers compute superseded from the report's record order.
    pub fn source_status(&self, superseded: bool) -> SummarySourceStatus {
        if superseded {
            SummarySourceStatus::Historical
        } else if self.source_current {
            SummarySourceStatus::Current
        } else {
            SummarySourceStatus::Uncertified
        }
    }
}

choice! {
    /// Structural or currentness inconsistency in regional evidence.
    pub enum RegionalValidationKind {
        /// Native node/call IDs must equal their record-local table indexes.
        Identity,
        /// A transfer's call belongs to a different source node.
        CallOrigin,
        /// A callee entry belongs to a different procedure.
        CalleeEntry,
        /// A receipt marked current has a different input revision.
        Revision,
        /// An excluded body cannot have a current transfer.
        Excluded,
        /// Environment/native execution cannot claim a captured block visit.
        BlockVisit,
        /// Code identity, procedure interpretation and receipt interpretation disagree.
        Semantics,
    }
}
record! {
    /// Invalid regional evidence, qualified by its summary and local identity.
    pub struct RegionalValidationFailure {
        /// Containing abstract summary index.
        pub summary: usize,
        /// Local node or call whose evidence is inconsistent.
        pub id: usize,
        /// Whether id denotes a call instead of a node.
        pub call: bool,
        /// Authoritative structural/currentness cause.
        pub kind: RegionalValidationKind,
    }
}
