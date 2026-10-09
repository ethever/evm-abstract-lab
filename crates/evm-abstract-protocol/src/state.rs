//! Typed world, frame, byte-array and account snapshots. Pools avoid duplicating
//! immutable stores and buffers across states, frames and outcomes.

choice! {
    /// Semantic names of immutable EVM inputs.
    #[derive(std::hash::Hash)]
    pub enum InputSymbolKind {
        /// To input.
        To,
        /// Caller input.
        Caller,
        /// Origin input.
        Origin,
        /// Coinbase input.
        Coinbase,
        /// CallValue input.
        CallValue,
        /// GasPrice input.
        GasPrice,
        /// Timestamp input.
        Timestamp,
        /// Number input.
        Number,
        /// Prevrandao input.
        Prevrandao,
        /// GasLimit input.
        GasLimit,
        /// ChainId input.
        ChainId,
        /// BaseFee input.
        BaseFee,
        /// BlobBaseFee input.
        BlobBaseFee,
        /// CalldataLength input.
        CalldataLength,
        /// CalldataWord input.
        CalldataWord,
        /// BlockHash input.
        BlockHash,
        /// BlobHash input.
        BlobHash,
    }
}
record! {
    /// An input name with an optional word index.
    #[derive(std::hash::Hash)]
    pub struct InputSymbol {
        /// Semantic field.
        pub kind: InputSymbolKind,
        /// Exact calldata offset, block height or blob index where required.
        pub index: Option<String>,
    }
}
record! {
    /// Immutable identity within one analysis input namespace.
    #[derive(std::hash::Hash)]
    pub struct InputIdentity {
        /// Equality namespace, meaningful only within this report.
        pub scope: u64,
        /// Named input in that namespace.
        pub symbol: InputSymbol,
    }
}
record! {
    /// A retained pure EVM expression.
    #[derive(std::hash::Hash)]
    pub struct ExpressionOperation {
        /// Raw opcode.
        pub opcode: u8,
        /// Indices into the owning ExpressionGraph, in EVM pop order.
        pub arguments: Vec<usize>,
    }
}
variant! {
    /// Structural symbolic value, never a guessed concrete word.
    #[derive(std::hash::Hash)]
    pub enum Expression {
        /// Exact word.
        Constant(String),
        /// Immutable input.
        Input(InputIdentity),
        /// Fresh runtime name within the report.
        Fresh(u64),
        /// Pure EVM operation.
        Operation(ExpressionOperation),
    }
}
record! {
    /// Flat symbolic DAG. JSON depth is independent of EVM expression depth.
    #[derive(std::hash::Hash)]
    pub struct ExpressionGraph {
        /// Nodes in dependency order; operation operands reference earlier nodes.
        pub nodes: Vec<Expression>,
        /// Index of the represented root expression.
        pub root: usize,
    }
}
record! {
    /// Inclusive word interval endpoints.
    #[derive(std::hash::Hash)]
    pub struct WordBounds {
        /// Lower endpoint as a hexadecimal word.
        pub lower: String,
        /// Upper endpoint as a hexadecimal word.
        pub upper: String,
    }
}
record! {
    /// A nonzero-modulus congruence.
    #[derive(std::hash::Hash)]
    pub struct CongruenceClass {
        /// Modulus.
        pub modulus: String,
        /// Canonical residue.
        pub residue: String,
    }
}
variant! {
    /// Congruence component of a product value.
    #[derive(std::hash::Hash)]
    pub enum CongruenceValue {
        /// No congruence restriction.
        Any,
        /// One word.
        Exact(String),
        /// A modular class.
        Modulo(CongruenceClass),
    }
}
choice! {
    /// Possible source category; this does not establish equality.
    #[derive(std::hash::Hash)]
    pub enum ValueOrigin {
        /// Constant source.
        Constant,
        /// Calldata source.
        Calldata,
        /// Memory source.
        Memory,
        /// Storage source.
        Storage,
        /// TransientStorage source.
        TransientStorage,
        /// Environment source.
        Environment,
        /// Address source.
        Address,
        /// CodeAddress source.
        CodeAddress,
        /// CallValue source.
        CallValue,
        /// Returndata source.
        Returndata,
        /// Arithmetic source.
        Arithmetic,
        /// Balance source.
        Balance,
        /// Nonce source.
        Nonce,
    }
}
record! {
    /// Typed abstract word with its numeric and symbolic guarantees.
    #[derive(std::hash::Hash)]
    pub struct ValueInfo {
        /// Convenient human rendering, supplementing all typed components.
        pub summary: String,
        /// Complete finite set, or None for an unrestricted finite-set component.
        pub constants: Option<Vec<String>>,
        /// Bits guaranteed zero.
        pub known_zero: String,
        /// Bits guaranteed one.
        pub known_one: String,
        /// Unsigned interval.
        pub unsigned: WordBounds,
        /// Signed interval endpoints as two-complement bit patterns.
        pub signed: WordBounds,
        /// Congruence component.
        pub congruence: CongruenceValue,
        /// Complete possible-source set, or None when unknown.
        pub origins: Option<Vec<ValueOrigin>>,
        /// Guaranteed use as a code-address value.
        pub code_address_role: bool,
        /// Retained immutable equality identity.
        pub identity: Option<InputIdentity>,
        /// Retained structural expression.
        pub expression: Option<ExpressionGraph>,
        /// Expression retention exceeded its declared bound.
        pub symbolic_limit: bool,
    }
}
variant! {
    /// Concrete or named symbolic frame address.
    pub enum AddressValue {
        /// Concrete 20-byte account.
        Concrete(String),
        /// Symbolic input identity.
        Symbolic(InputSymbol),
    }
}
record! {
    /// Explicit sparse byte-array entry.
    pub struct ByteCell {
        /// Byte offset.
        pub offset: usize,
        /// Index of the complete stored-byte facts in ByteArraySnapshot.values.
        pub value: usize,
    }
}
record! {
    /// Sparse byte sequence; default and length must be interpreted together.
    pub struct ByteArraySnapshot {
        /// Abstract byte length.
        pub length: ValueInfo,
        /// Value of unspecified in-range bytes.
        pub default: ValueInfo,
        /// Explicit sparse cells in ascending offset order.
        pub cells: Vec<ByteCell>,
        /// Complete scalar facts shared by cells with exactly equal components.
        pub values: Vec<ValueInfo>,
        /// True for word-rounded zero-initialized EVM memory.
        pub memory: bool,
    }
}
choice! {
    /// Account or frame executable form.
    pub enum CodeKind {
        /// Ordinary deployed bytecode.
        Runtime,
        /// Creation-time bytecode.
        Initcode,
        /// EIP-7702 delegation marker.
        Delegation,
        /// Known empty code.
        Empty,
        /// Code is unobserved.
        Unknown,
        /// Native precompile.
        Precompile,
        /// Unsupported nested delegation.
        InvalidDelegation,
    }
}
choice! {
    /// Observed account presence.
    pub enum AccountExistence {
        /// No observation establishes presence or absence.
        Unknown,
        /// Account exists.
        Present,
        /// Account absence is established.
        Absent,
    }
}
record! {
    /// Complete source for one captured executable identity.
    pub struct ProgramInfo {
        /// Index in the report program directory.
        pub id: usize,
        /// Executable account or creation namespace.
        pub code_address: String,
        /// Hash of the captured bytes.
        pub code_hash: String,
        /// Runtime or creation-time program.
        pub kind: CodeKind,
        /// Complete captured bytes in hexadecimal.
        pub bytecode: String,
        /// All decoded basic blocks, including unexecuted source.
        pub blocks: Vec<crate::DisasmBlock>,
    }
}
record! {
    /// One explicit storage observation or transaction value.
    pub struct StorageEntry {
        /// Complete 256-bit storage key.
        pub slot: String,
        /// Abstract value.
        pub value: ValueInfo,
    }
}
record! {
    /// One account in a transaction store, including unknown defaults.
    pub struct AccountState {
        /// Storage and balance owner.
        pub address: String,
        /// Account presence.
        pub existence: AccountExistence,
        /// Current balance.
        pub balance: ValueInfo,
        /// Current nonce.
        pub nonce: ValueInfo,
        /// Current code observation.
        pub code_kind: CodeKind,
        /// Observed code hash, if known.
        pub code_hash: Option<String>,
        /// Program-directory index for runtime code.
        pub program: Option<usize>,
        /// Authority implementation when code is a delegation marker.
        pub delegation_target: Option<String>,
        /// Default unspecified persistent slot.
        pub storage_default: ValueInfo,
        /// Explicit persistent slots.
        pub storage: Vec<StorageEntry>,
        /// Default unspecified transient slot.
        pub transient_default: ValueInfo,
        /// Explicit transient slots.
        pub transient: Vec<StorageEntry>,
        /// Whether created in this transaction, or None if unknown.
        pub created: Option<bool>,
        /// Whether deletion is pending, or None if unknown.
        pub pending_destruction: Option<bool>,
    }
}
record! {
    /// Possible log payload with its native emission site.
    pub struct LogSnapshot {
        /// Emitting storage owner.
        pub address: String,
        /// Executing code account.
        pub code_address: String,
        /// Opcode offset.
        pub pc: usize,
        /// Abstract topics.
        pub topics: Vec<ValueInfo>,
        /// Index in byte_arrays.
        pub data: usize,
    }
}
record! {
    /// Transaction store, with defaults covering unlisted accounts.
    pub struct StoreSnapshot {
        /// Global persistent default.
        pub storage_default: ValueInfo,
        /// Global transient default.
        pub transient_default: ValueInfo,
        /// Default balance of unlisted accounts.
        pub balance_default: ValueInfo,
        /// Accounts with explicit observations or effects.
        pub accounts: Vec<AccountState>,
        /// Possible logs retained by source site.
        pub logs: Vec<LogSnapshot>,
        /// Additional logs may exist.
        pub logs_unknown: bool,
    }
}
record! {
    /// Suspended caller continuation.
    pub struct ContinuationSnapshot {
        /// Caller continuation basic block.
        pub return_block: Option<usize>,
        /// Requested output memory offset.
        pub output_offset: ValueInfo,
        /// Requested output memory span.
        pub output_size: ValueInfo,
        /// Account being created, when applicable.
        pub creation: Option<String>,
    }
}
record! {
    /// Execution context in root-to-active call-stack order.
    pub struct FrameSnapshot {
        /// Frame index, zero for the root.
        pub index: usize,
        /// Captured executable program ID.
        pub program: Option<usize>,
        /// Account supplying executed code.
        pub code_address: String,
        /// Captured code identity.
        pub code_hash: String,
        /// Account owning persistent/transient storage.
        pub storage_address: String,
        /// Logical ADDRESS, distinct from an internal namespace.
        pub address_value: AddressValue,
        /// CALLER identity.
        pub caller: AddressValue,
        /// CALLVALUE in this frame.
        pub call_value: ValueInfo,
        /// Whether writes are prohibited.
        pub is_static: bool,
        /// Runtime, initcode, empty or precompile mode.
        pub kind: CodeKind,
        /// Current basic-block index.
        pub basic_block: usize,
        /// Retained jump history.
        pub context: Vec<usize>,
        /// Bottom-to-top abstract stack.
        pub stack: Vec<ValueInfo>,
        /// Index in byte_arrays.
        pub memory: usize,
        /// Index in byte_arrays.
        pub calldata: usize,
        /// Index in byte_arrays.
        pub returndata: usize,
        /// Store snapshot restored if this frame fails.
        pub rollback_store: usize,
        /// Parent continuation; absent for the root.
        pub continuation: Option<ContinuationSnapshot>,
    }
}
record! {
    /// Full typed state at a block entry or retained exit.
    pub struct MachineSnapshot {
        /// Call stack from root to active frame.
        pub frames: Vec<FrameSnapshot>,
        /// Index in stores.
        pub store: usize,
    }
}
record! {
    /// Typed payload for one native CFG state.
    pub struct StateDetails {
        /// Native state ID shared with cfg and SSA.
        pub state: usize,
        /// Current entry facts.
        pub entry: MachineSnapshot,
        /// Retained exit; coverage determines whether it is current.
        pub exit: Option<MachineSnapshot>,
    }
}
choice! {
    /// Terminal root execution result.
    pub enum OutcomeKind {
        /// Successful STOP, RETURN or reaching the end of code.
        Return,
        /// Explicit REVERT.
        Revert,
        /// Exceptional halt.
        Failure,
    }
}
record! {
    /// Terminal data and finalized transaction effects.
    pub struct Outcome {
        /// Native terminal state.
        pub state: usize,
        /// Termination category.
        pub kind: OutcomeKind,
        /// Return/revert bytes index in byte_arrays.
        pub data: usize,
        /// Final store index, including rollback/finalization.
        pub store: usize,
    }
}
record! {
    /// Identity and block observations actually obtained from RPC.
    pub struct ChainSnapshot {
        /// Observed chain identity.
        pub chain_id: String,
        /// Pinned canonical hash.
        pub block_hash: String,
        /// Observed number.
        pub number: Option<String>,
        /// Observed parent hash.
        pub parent_hash: Option<String>,
        /// Observed timestamp.
        pub timestamp: Option<String>,
        /// Observed beneficiary.
        pub coinbase: Option<String>,
        /// Observed randomness.
        pub prevrandao: Option<String>,
        /// Observed block gas limit.
        pub gas_limit: Option<String>,
        /// Observed base fee, or None if not supplied.
        pub base_fee: Option<String>,
        /// Observed derived blob base fee when available.
        pub blob_base_fee: Option<String>,
        /// Observed excess blob gas.
        pub excess_blob_gas: Option<String>,
        /// Observed blob gas used.
        pub blob_gas_used: Option<String>,
    }
}
record! {
    /// Effective execution environment after explicit overrides.
    pub struct EnvironmentSnapshot {
        /// Logical root ADDRESS.
        pub to: AddressValue,
        /// Root CALLER.
        pub caller: AddressValue,
        /// Transaction ORIGIN.
        pub origin: AddressValue,
        /// Root call value.
        pub call_value: ValueInfo,
        /// Root calldata index in byte_arrays.
        pub calldata: usize,
        /// Root static mode.
        pub is_static: bool,
        /// Initial GAS upper bound; None means unknown.
        pub gas_upper_bound: Option<String>,
        /// Effective transaction gas price.
        pub gas_price: ValueInfo,
        /// Effective beneficiary.
        pub coinbase: AddressValue,
        /// Effective timestamp.
        pub timestamp: ValueInfo,
        /// Effective block number.
        pub number: ValueInfo,
        /// Effective randomness.
        pub prevrandao: ValueInfo,
        /// Effective block gas limit.
        pub gas_limit: ValueInfo,
        /// Effective CHAINID, separate from snapshot identity.
        pub chain_id: ValueInfo,
        /// Effective block base fee.
        pub base_fee: ValueInfo,
        /// Effective blob base fee.
        pub blob_base_fee: ValueInfo,
        /// Known historical block hashes.
        pub block_hashes: Vec<crate::IndexedHash>,
        /// Known blob hashes.
        pub blob_hashes: Vec<crate::IndexedHash>,
        /// Abstract transaction blob count.
        pub blob_count: ValueInfo,
    }
}
record! {
    /// Reproducibility and entry metadata for one immutable report.
    pub struct ReportMetadata {
        /// Concrete root storage namespace.
        pub entry_address: String,
        /// Resolved root executable program, including root delegation.
        pub root_program: Option<usize>,
        /// Pinned chain identity and observed header, absent for bytecode input.
        pub snapshot: Option<ChainSnapshot>,
        /// Native immutable world fingerprint.
        pub fingerprint: String,
        /// Actual effective root inputs.
        pub environment: EnvironmentSnapshot,
        /// Effective execution, precision and acquisition settings.
        pub limits: crate::AnalysisLimits,
        /// Consumed logical execution work.
        pub work: u64,
    }
}
