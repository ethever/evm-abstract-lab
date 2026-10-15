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
        /// Recursive execution has no covered terminal behavior in this model.
        RecursiveNoTerminal,
        /// CallDepth boundary.
        CallDepth,
        /// A byte-array representation rejected the original range.
        ByteRange(ByteRangeBoundary),
        /// Complete incoming state could not be safely joined.
        StateJoin(JoinBoundary),
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

variant! {
    /// Atomic machine join boundary, not an EVM exceptional halt.
    pub enum JoinBoundary {
        /// Frame or continuation shapes disagree.
        Shape,
        /// A unique branch selector could not be allocated.
        IdentityExhausted,
        /// Retained complete alternatives reached this limit.
        Alternatives(usize),
    }
}

record! {
    /// Concrete host range arithmetic rejected this access.
    pub struct ByteRangeOverflow {
        /// Concrete destination byte offset.
        pub offset: usize,
        /// Concrete requested byte count.
        pub size: usize,
    }
}
record! {
    /// Concrete logical or materialization policy rejected this range.
    pub struct ByteRangeLimit {
        /// Exact Word256 requested count or endpoint.
        pub requested: String,
        /// Declared byte cap.
        pub limit: usize,
    }
}
variant! {
    /// Byte-state representation failure, distinct from logical memory partitioning.
    pub enum ByteRangeBoundary {
        /// No finite size or trusted symbolic term was available.
        UnknownSize,
        /// Concrete destination exceeds a host index.
        OffsetTooLarge(String),
        /// Host arithmetic overflow.
        Overflow(ByteRangeOverflow),
        /// Declared concrete range cap.
        TooLarge(ByteRangeLimit),
        /// Source binding requested after input identity was established.
        InputAlreadyBound,
    }
}
