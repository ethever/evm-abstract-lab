//! Conservative process evidence preserves captured code identity and control obligations.
mod creation;
mod frame;
pub use creation::*;
pub use frame::*;

choice! {
    /// Interpretation independently of the original code kind and hash.
    pub enum TransferSemantics {
        /// Captured instructions, empty code or a native precompile.
        Native,
        /// Operational abstraction of an unobserved implementation.
        Environment,
        /// Conservative process for captured runtime code.
        KnownProcess,
        /// Constructor execution before returned code is deployed.
        ConstructorProcess,
        /// Empty, native precompile or invalid-delegation process boundary.
        BoundaryProcess,
    }
}

choice! {
    /// EVM action, independently of the callee's code kind.
    pub enum ProcessAction {
        /// CALL changes the child's state owner to its target.
        Call,
        /// STATICCALL changes owner and propagates read-only execution.
        StaticCall,
        /// DELEGATECALL preserves owner, inherited caller and value.
        DelegateCall,
        /// CALLCODE preserves owner and uses the current owner as caller.
        CallCode,
        /// CREATE includes constructor and deployment obligations.
        Create,
        /// CREATE2 also includes salt-dependent allocation obligations.
        Create2,
    }
}

variant! {
    /// Target before code resolution; a residual class is never a concrete address.
    pub enum ProcessOwner {
        /// Original root state namespace; may alias any physical class.
        RootNamespace,
        /// Exact owner in the invocation's named vocabulary.
        Named(String),
        /// Remaining owners and unresolved aliases.
        Residual,
    }
}

variant! {
    /// An uncovered process rule, retained as an incomplete frontier.
    pub enum ProcessBoundary {
        /// A precise source requires a new catalogue epoch and admission.
        Catalogue,
        /// Residual owner and alias representation is not covered.
        ResidualOwner(ProcessAction),
        /// Constructor or deployment semantics is not covered.
        Creation(ProcessAction),
        /// Foreign cooperative inputs are not covered.
        CooperativeInputs,
        /// Symbolic logical owner has no covering owner representation.
        SymbolicOwner,
        /// Process input cannot be interpreted as a native machine input.
        NativeInstantiation,
    }
}

record! {
    /// Actual conservative rules and remaining obligations, without instruction evidence.
    pub struct ProcessTransfer {
        /// Actions may repeat before a represented exit.
        pub repeat: bool,
        /// Primitive rules selected by the native capability scan.
        pub actions: Vec<ProcessAction>,
        /// Local allocation guards actually recorded before constructor admission.
        pub creations: Vec<ProcessCreationTransfer>,
        /// Rules still requiring coverage; these do not certify closure.
        pub unresolved: Vec<ProcessBoundary>,
    }
}
