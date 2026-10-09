//! Concrete native admission errors; display text never carries the only facts.
record! {
    /// Invalid hexadecimal character after whitespace/prefix normalization.
    pub struct HexDigitFailure {
        /// Character index.
        pub index: usize,
        /// Rejected Unicode scalar.
        pub character: char,
    }
}
choice! {
    /// Malformed EIP-7702 marker.
    pub enum DelegationFailure {
        /// Marker must contain exactly 23 bytes.
        InvalidLength,
        /// Required magic prefix was absent.
        InvalidMagic,
        /// Marker version was unsupported.
        UnsupportedVersion,
    }
}
variant! {
    /// Runtime bytecode decoding failure.
    pub enum BytecodeFailure {
        /// Odd normalized hexadecimal digit count.
        OddHexLength(usize),
        /// Invalid normalized digit.
        InvalidHex(HexDigitFailure),
        /// EOF is outside the runtime model.
        UnsupportedEof,
        /// Bytecode mode cannot resolve this EIP-7702 destination.
        DelegatedCode(String),
        /// Malformed marker.
        InvalidDelegation(DelegationFailure),
    }
}
record! {
    /// Indexed blob observation contradicts its declared count.
    pub struct BlobIndexFailure {
        /// Observed word index.
        pub index: String,
        /// Declared count.
        pub count: String,
    }
}
record! {
    /// Execution destination contradicts its storage owner.
    pub struct DestinationFailure {
        /// State owner.
        pub expected: String,
        /// Submitted destination.
        pub observed: String,
    }
}
variant! {
    /// Native environment invariant.
    pub enum EnvironmentFailure {
        /// Block/blob observation table cap.
        TableLimit,
        /// Contradictory blob index.
        BlobIndex(BlobIndexFailure),
        /// Contradictory destination.
        Destination(DestinationFailure),
    }
}
variant! {
    /// Native analysis configuration invariant.
    pub enum ConfigurationFailure {
        /// Invalid environment.
        Environment(EnvironmentFailure),
        /// Constant capacity is zero.
        Constants,
        /// An execution budget is zero.
        Budget,
        /// Fact exchange policy is empty.
        Facts,
        /// Relational policy is empty.
        Relations,
    }
}
record! {
    /// A server admission range and rejected number.
    pub struct LimitFailure {
        /// Exact request field.
        pub field: String,
        /// Submitted number.
        pub value: u64,
        /// Inclusive lower bound.
        pub minimum: u64,
        /// Inclusive upper bound.
        pub maximum: u64,
    }
}
