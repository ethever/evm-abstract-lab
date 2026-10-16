//! Concrete scalar-fact causes carried by local reasoning frontiers.

record! {
    /// Pure-operation arity mismatch during scalar fact interpretation.
    pub struct FactOperationFailure {
        /// EVM opcode.
        pub opcode: u8,
        /// Required operand count.
        pub expected: usize,
        /// Observed operand count.
        pub actual: usize,
    }
}
record! {
    /// One fact insertion exceeded its semantic atom allowance.
    pub struct FactCapacityFailure {
        /// Configured capacity.
        pub max_atoms: usize,
        /// Capacity required for the complete insertion.
        pub required: usize,
    }
}
variant! {
    /// Exact local scalar-fact failure. Symbol IDs belong to the local fact
    /// exchange, independently of SSA values or immutable input identities.
    pub enum FactFailure {
        /// Bit index lies outside the EVM word.
        InvalidBitIndex(u16),
        /// A bit was declared both zero and one.
        ConflictingBits,
        /// Lower interval bound exceeds the upper bound.
        InvalidBounds,
        /// Finite membership was empty.
        EmptyFiniteSet,
        /// Congruence modulus was zero.
        InvalidModulus,
        /// Unsupported pure EVM operation.
        UnsupportedOperation(u8),
        /// Pure-operation operand count mismatch.
        InvalidOperation(FactOperationFailure),
        /// Contradictory numeric facts for this local fact symbol.
        Contradiction(u32),
        /// Contradictory origins for this local fact symbol.
        OriginContradiction(u32),
        /// Semantic atom allowance was exceeded.
        Capacity(FactCapacityFailure),
    }
}
