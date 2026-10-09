//! Nested RPC and snapshot failures, without endpoint URLs or opaque JSON.
choice! {
    /// Native RPC invariant category.
    pub enum RpcConfigurationReason {
        /// Limits observation.
        Limits,
        /// InitialAccounts observation.
        InitialAccounts,
        /// StorageAccountMissing observation.
        StorageAccountMissing,
    }
}
choice! {
    /// Native RPC invariant category.
    pub enum RpcHeaderField {
        /// ParentHash observation.
        ParentHash,
        /// Number observation.
        Number,
        /// Timestamp observation.
        Timestamp,
        /// Miner observation.
        Miner,
        /// MixHash observation.
        MixHash,
        /// GasLimit observation.
        GasLimit,
        /// BaseFee observation.
        BaseFee,
        /// ExcessBlobGas observation.
        ExcessBlobGas,
        /// BlobGasUsed observation.
        BlobGasUsed,
    }
}
variant! {
    /// Exact native failure reason.
    pub enum RpcHeaderValue {
        /// Absent failure.
        Absent,
        /// Quantity failure.
        Quantity(String),
        /// Hash failure.
        Hash(String),
        /// Address failure.
        Address(String),
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct WordMismatch {
        /// expected fact.
        pub expected: String,
        /// observed fact.
        pub observed: String,
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct BlobHeaderFields {
        /// excess blob gas fact.
        pub excess_blob_gas: bool,
        /// blob gas used fact.
        pub blob_gas_used: bool,
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct FeeHistoryMismatch {
        /// expected fact.
        pub expected: String,
        /// observed fact.
        pub observed: String,
        /// fees fact.
        pub fees: usize,
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct ResponseIdMismatch {
        /// expected fact.
        pub expected: u64,
        /// observed fact.
        pub observed: Option<u64>,
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct HeaderChange {
        /// field fact.
        pub field: RpcHeaderField,
        /// expected fact.
        pub expected: RpcHeaderValue,
        /// observed fact.
        pub observed: RpcHeaderValue,
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct CountMismatch {
        /// expected fact.
        pub expected: usize,
        /// observed fact.
        pub observed: usize,
    }
}
variant! {
    /// Exact native failure reason.
    pub enum RpcResponseReason {
        /// BlockNumber failure.
        BlockNumber(WordMismatch),
        /// BlobHeaderFields failure.
        BlobHeaderFields(BlobHeaderFields),
        /// FeeHistory failure.
        FeeHistory(FeeHistoryMismatch),
        /// Schema failure.
        Schema,
        /// Version failure.
        Version,
        /// Id failure.
        Id(ResponseIdMismatch),
        /// ResultAndError failure.
        ResultAndError,
        /// NullError failure.
        NullError,
        /// HeaderChanged failure.
        HeaderChanged(HeaderChange),
        /// MissingBlockNumber failure.
        MissingBlockNumber,
        /// StorageLength failure.
        StorageLength(CountMismatch),
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct ForkMismatch {
        /// world fact.
        pub world: crate::Fork,
        /// account fact.
        pub account: crate::Fork,
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct CodeHashMismatch {
        /// address fact.
        pub address: String,
        /// expected fact.
        pub expected: String,
        /// observed fact.
        pub observed: String,
    }
}
variant! {
    /// Exact native failure reason.
    pub enum WorldFailure {
        /// MixedFork failure.
        MixedFork(ForkMismatch),
        /// UnsupportedDelegation failure.
        UnsupportedDelegation(crate::Fork),
        /// Conflict failure.
        Conflict(String),
        /// CodeHash failure.
        CodeHash(CodeHashMismatch),
        /// UnknownCodeHash failure.
        UnknownCodeHash(String),
        /// InvalidAbsence failure.
        InvalidAbsence(String),
    }
}
record! {
    /// Typed contextual RPC facts.
    pub struct RpcTransportFailure {
        /// timeout fact.
        pub timeout: bool,
        /// connect fact.
        pub connect: bool,
        /// builder fact.
        pub builder: bool,
        /// request fact.
        pub request: bool,
        /// body fact.
        pub body: bool,
        /// decode fact.
        pub decode: bool,
        /// redirect fact.
        pub redirect: bool,
    }
}
variant! {
    /// Exact native failure reason.
    pub enum RpcFailureCause {
        /// Configuration failure.
        Configuration(RpcConfigurationReason),
        /// Response failure.
        Response(RpcResponseReason),
        /// Code failure.
        Code(crate::BytecodeFailure),
        /// World failure.
        World(WorldFailure),
        /// ChainMismatch failure.
        ChainMismatch(String),
        /// BlockMismatch failure.
        BlockMismatch(String),
        /// Transport failure.
        Transport(RpcTransportFailure),
        /// Runtime failure.
        Runtime(Option<i32>),
    }
}
