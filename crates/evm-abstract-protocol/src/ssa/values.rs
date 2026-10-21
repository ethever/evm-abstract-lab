//! Normalized computations and report-local value facts. Observed state facts
//! do not assert a definition-wide constant across unknown incoming paths.

variant! {
    /// One flat SSA operand; expressions are never nested here.
    pub enum SsaOperand {
        /// Final report-local value definition.
        Value(usize),
        /// Exact EVM word in lossless hexadecimal form.
        Constant(String),
    }
}
record! {
    /// An opcode with normalized operands in EVM pop order.
    pub struct SsaOpcodeOperation {
        /// EVM opcode byte.
        pub opcode: u8,
        /// Opcode mnemonic.
        pub name: String,
        /// Values or proven constants, without expression-tree expansion.
        pub operands: Vec<SsaOperand>,
    }
}
variant! {
    /// Value computation after basic normalization. Effects and instruction
    /// phases remain on the containing original execution record.
    pub enum SsaOperation {
        /// An operation which still computes its result.
        Opcode(SsaOpcodeOperation),
        /// A proven exact result.
        Constant(String),
        /// A result equal to one operand; its definition is retained.
        Copy(SsaOperand),
    }
}
record! {
    /// Original frame entry parameter. Phi definitions are retained.
    pub struct SsaPhiOrigin {
        /// Native machine state.
        pub state: usize,
        /// Frame index, oldest suspended caller first.
        pub frame: usize,
        /// Entry stack slot, bottom to top.
        pub slot: usize,
    }
}
record! {
    /// Original executed instruction defining a value.
    pub struct SsaInstructionOrigin {
        /// Native machine state.
        pub state: usize,
        /// Index within the state's current instruction receipt.
        pub instruction_index: usize,
        /// Original bytecode position.
        pub pc: usize,
    }
}
record! {
    /// Value defined by a native continuation transition.
    pub struct SsaTransitionOrigin {
        /// Native edge identity.
        pub edge: usize,
        /// Native source state.
        pub from: usize,
        /// Native destination state.
        pub to: usize,
    }
}
variant! {
    /// Definition identity independent of final numeric display numbering.
    pub enum SsaValueOrigin {
        /// Frame entry parameter.
        Phi(SsaPhiOrigin),
        /// Instruction result.
        Instruction(SsaInstructionOrigin),
        /// CALL/CREATE continuation result.
        Transition(SsaTransitionOrigin),
    }
}
choice! {
    /// Availability of a value fact, separate from its numeric precision.
    pub enum SsaFactStatus {
        /// Fact is valid within its explicit scope in this report.
        Current,
        /// No fact was retained or derived for this definition.
        Uncollected,
        /// The execution receipt no longer matches the current state entry.
        Stale,
        /// The state has not supplied an execution receipt.
        Unexecuted,
    }
}
variant! {
    /// Where a fact is justified. State/edge observations must not be promoted
    /// to unconditional definition facts by the consumer.
    pub enum SsaFactScope {
        /// Holds for the definition, within this analysis report's inputs.
        Definition,
        /// Holds for the recorded current input of one native state.
        State(usize),
        /// Holds on one supported original transition.
        Edge(usize),
    }
}
choice! {
    /// How a fact was obtained; source categories do not establish equality.
    pub enum SsaFactSource {
        /// No evidence supplied.
        None,
        /// Original literal value.
        Literal,
        /// A completed instruction's execution receipt.
        Execution,
        /// A machine frame's entry value.
        Entry,
        /// A supported continuation transition.
        Transition,
        /// Evaluation using proven operand facts.
        Propagation,
        /// An EVM algebraic identity.
        Algebraic,
    }
}
record! {
    /// Scoped word guarantees with independent exact proofs and observations.
    /// A proved constant does not require a retained ValueInfo; availability
    /// is determined by status rather than the presence of either payload.
    pub struct SsaValueFacts {
        /// Whether this fact is available and current.
        pub status: SsaFactStatus,
        /// Explicit scope of the guarantee.
        pub scope: SsaFactScope,
        /// Derivation category.
        pub source: SsaFactSource,
        /// Exact word proved by a native observation or independent derivation.
        pub constant: Option<String>,
        /// Retained numeric properties and source categories, independently optional.
        pub value: Option<crate::ValueInfo>,
    }
}
choice! {
    /// Transformation applied to a value definition.
    pub enum SsaTransformation {
        /// Original definition retained.
        Original,
        /// A PUSH literal represented as a constant definition.
        Literal,
        /// Exact pure EVM evaluation of proven constant operands.
        ConstantFold,
        /// Proven identity yields an existing operand or a constant.
        AlgebraicIdentity,
        /// A current, sufficiently covered execution fact supplies a constant.
        AnalyzedConstant,
    }
}
record! {
    /// One final SSA value. Numeric IDs are local to this report; provenance
    /// carries original definition identity through normalization/renumbering.
    pub struct SsaValue {
        /// Contiguous final value ID; also the index in SsaReport.values.
        pub id: usize,
        /// Definition ID assigned by the temporary source SSA builder.
        pub original_value: usize,
        /// Original native definition location.
        pub origin: SsaValueOrigin,
        /// Scoped facts for queries and display. Definition guarantees and
        /// state observations are separate entries, never implicitly merged.
        pub facts: Vec<SsaValueFacts>,
        /// Rule applied to the definition.
        pub transformation: SsaTransformation,
        /// Equal final value when an identity produces a copy.
        pub equivalent: Option<usize>,
        /// Final values used to derive this result; dependency does not imply equality.
        pub dependencies: Vec<usize>,
    }
}
