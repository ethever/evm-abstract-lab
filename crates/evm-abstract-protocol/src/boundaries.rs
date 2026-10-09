//! Concrete model boundaries retained independently of display explanations.
choice! {
    /// Native boundary category.
    pub enum ReductionReason {
        /// Stable boundary.
        Stable,
        /// RoundLimit boundary.
        RoundLimit,
        /// FactLimit boundary.
        FactLimit,
        /// Empty boundary.
        Empty,
        /// OriginConflict boundary.
        OriginConflict,
        /// SymbolicLimit boundary.
        SymbolicLimit,
    }
}
variant! {
    /// Complete typed native reason.
    pub enum QueryBoundary {
        /// Cooperative cancellation.
        Cancelled,
        /// Disabled boundary.
        Disabled,
        /// Configuration boundary.
        Configuration,
        /// ConstraintLimit boundary.
        ConstraintLimit,
        /// ScalarFactLimit boundary.
        ScalarFactLimit,
        /// ScalarFactError boundary.
        ScalarFactError(crate::FactFailure),
        /// ExpressionLimit boundary.
        ExpressionLimit,
        /// Unsupported boundary.
        Unsupported(u8),
        /// ResourceLimit boundary.
        ResourceLimit,
        /// SolverUnknown boundary.
        SolverUnknown(crate::SolverUnknown),
        /// Native binding or encoding failed, independently of UNKNOWN.
        SolverError(crate::SolverFailure),
        /// ModelUnavailable boundary.
        ModelUnavailable,
    }
}
variant! {
    /// Complete typed native reason.
    pub enum CreationReason {
        /// UnknownCreator boundary.
        UnknownCreator,
        /// UnknownNonce boundary.
        UnknownNonce(String),
        /// UnknownCollision boundary.
        UnknownCollision(String),
        /// UnknownEndowment boundary.
        UnknownEndowment,
        /// UnknownInitCode boundary.
        UnknownInitCode,
        /// UnknownRuntimeCode boundary.
        UnknownRuntimeCode,
        /// UnknownSalt boundary.
        UnknownSalt,
        /// NonceOverflow boundary.
        NonceOverflow(String),
        /// ReservedAddress boundary.
        ReservedAddress(String),
    }
}
variant! {
    /// Complete typed native reason.
    pub enum FrontierDetails {
        /// Budget boundary.
        Budget(crate::FrontierKind),
        /// Work boundary.
        Work,
        /// SummaryWork boundary.
        SummaryWork,
        /// CallDepth boundary.
        CallDepth,
        /// Memory boundary.
        Memory,
        /// Relations boundary.
        Relations(QueryBoundary),
        /// UnknownTarget boundary.
        UnknownTarget,
        /// MissingCode boundary.
        MissingCode(String),
        /// MissingStorage boundary.
        MissingStorage(crate::StorageLocation),
        /// RpcAcquisition boundary.
        RpcAcquisition(Box<crate::RpcFailure>),
        /// Precompile boundary.
        Precompile(String),
        /// PrecompileInput boundary.
        PrecompileInput(String),
        /// Creation boundary.
        Creation(CreationReason),
        /// UnsupportedOpcode boundary.
        UnsupportedOpcode(u8),
    }
}
