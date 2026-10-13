//! Recursive abstract execution evidence, separate from concrete graph edges.

record! {
    /// A known-code or abstract-environment relation in one joint fixed point.
    pub struct AbstractProcedure {
        /// Code identity; an abstract namespace is not an observed account.
        pub code_address: String,
        /// Captured code hash, without an invented unknown instruction stream.
        pub code_hash: Option<String>,
        /// Storage namespace used by the relation.
        pub storage_address: String,
        /// Whether this relation interprets bytecode or an abstract environment.
        pub kind: crate::CodeKind,
        /// Propagated static execution restriction.
        pub is_static: bool,
        /// Program-local blocks actually evaluated inside this relation.
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
    /// One unknown entry and the mutually recursive relations used to cover it.
    pub struct AbstractSummary {
        /// Native machine state requesting the relation.
        pub source_state: usize,
        /// All required transitions and dependencies reached a covered invariant.
        pub closed: bool,
        /// Known and unknown execution contexts participating in the relation.
        pub procedures: Vec<AbstractProcedure>,
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
