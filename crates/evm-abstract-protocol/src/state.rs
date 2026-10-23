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
        pub scope: crate::DecimalU64,
        /// Named input in that namespace.
        pub symbol: InputSymbol,
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
        /// Guaranteed nonzero, including a retained exclusion of zero.
        pub nonzero: bool,
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
        pub expression: Option<crate::ExpressionRef>,
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
        /// Complete lazy source, range and update graph root.
        pub symbolic: crate::ByteViewRef,
        /// State-local complete facts for retained symbolic operands.
        pub symbolic_facts: Vec<crate::SymbolicFact>,
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
        /// A conservative abstract process covering unknown-world execution.
        AbstractEnvironment,
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
        pub code_address: Option<String>,
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
        /// Complete ordered persistent storage root.
        pub storage_root: crate::SlotStateRef,
        /// Complete ordered transient storage root.
        pub transient_root: crate::SlotStateRef,
        /// Explicit persistent slots.
        pub storage: Vec<StorageEntry>,
        /// Default unspecified transient slot.
        pub transient_default: ValueInfo,
        /// Explicit transient slots.
        pub transient: Vec<StorageEntry>,
        /// Whether created in this transaction, or None if unknown.
        pub created: Option<bool>,
        /// Transaction touch guarantee: true means must touch, false untouched,
        /// and None includes both possibilities.
        pub touched: Option<bool>,
        /// Whether deletion is pending, or None if unknown.
        pub pending_destruction: Option<bool>,
    }
}
choice! {
    /// Execution phase of the captured bytecode containing a LOG instruction.
    #[derive(std::hash::Hash)]
    pub enum LogSourceKind {
        /// Initialization before returned runtime code is installed.
        Init,
        /// Deployed runtime, including delegated execution.
        Runtime,
    }
}
record! {
    /// Possible log payload with its complete captured emission identity.
    pub struct LogSnapshot {
        /// Emitting storage owner.
        pub address: String,
        /// Executing code account.
        pub code_address: String,
        /// Exact hash of the emitting frame's captured code, as hexadecimal text.
        pub code_hash: String,
        /// Initialization or deployed runtime of those captured bytes.
        pub source_kind: LogSourceKind,
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
        /// Actual unscoped persistent base template. Reads scope it by owner;
        /// equal templates do not equate slots of distinct accounts.
        pub storage_root: crate::SlotStateRef,
        /// Actual unscoped transient base template, with the same owner rule.
        pub transient_root: crate::SlotStateRef,
        /// Default balance of unlisted accounts.
        pub balance_default: ValueInfo,
        /// Actual nonce of unlisted accounts.
        pub nonce_default: ValueInfo,
        /// Actual omitted-account code; None is unobserved, not original code.
        pub code_default: Option<AccountCode>,
        /// Actual presence guarantee for unlisted accounts.
        pub existence_default: AccountExistence,
        /// Creation guarantee for unlisted accounts.
        pub created_default: Option<bool>,
        /// Destruction guarantee for unlisted accounts.
        pub pending_destruction_default: Option<bool>,
        /// Touch guarantee for unlisted accounts; None preserves may-touch.
        pub touched_default: Option<bool>,
        /// Accounts with explicit observations or effects.
        pub accounts: Vec<AccountState>,
        /// Possible logs retained by source site.
        pub logs: Vec<LogSnapshot>,
        /// Additional logs may exist.
        pub logs_unknown: bool,
        /// Persistent operand facts local to this transaction snapshot.
        pub storage_facts: Vec<crate::SymbolicFact>,
        /// Transient operand facts local to this transaction snapshot.
        pub transient_facts: Vec<crate::SymbolicFact>,
    }
}
variant! {
    /// Complete code default without assigning an arbitrary account owner.
    pub enum AccountCode {
        /// Complete captured runtime bytes in hexadecimal.
        Runtime(String),
        /// EIP-7702 implementation address.
        Delegation(String),
        /// Known empty code.
        Empty,
        /// Explicitly unobserved code.
        Unknown,
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
        pub creation: Option<crate::CreationContinuation>,
    }
}
record! {
    /// Execution context in root-to-active call-stack order.
    pub struct FrameSnapshot {
        /// Explicit process identity; native-only anchor fields are absent when present.
        pub process: Option<crate::ProcessFrameKey>,
        /// Actual process ADDRESS/CALLER/value operands, with no anchor reconstruction.
        pub process_operands: Option<crate::ProcessOperands>,
        /// Constructor descriptor retained independently of native continuation coordinates.
        pub process_creation: Option<crate::CreationContinuation>,
        /// Frame index, zero for the root.
        pub index: usize,
        /// Captured executable program ID.
        pub program: Option<usize>,
        /// Account supplying executed code.
        pub code_address: Option<String>,
        /// Captured code identity.
        pub code_hash: Option<String>,
        /// Account owning persistent/transient storage.
        pub storage_address: Option<String>,
        /// Logical ADDRESS, distinct from an internal namespace.
        pub address_value: Option<AddressValue>,
        /// CALLER identity.
        pub caller: Option<AddressValue>,
        /// Execution-derived logical ADDRESS when no named input captures it.
        pub address_operand: Option<ValueInfo>,
        /// Execution-derived CALLER when no named input captures it.
        pub caller_operand: Option<ValueInfo>,
        /// CALLVALUE in this frame.
        pub call_value: ValueInfo,
        /// Whether writes are prohibited.
        pub is_static: bool,
        /// Runtime, initcode, empty or precompile mode.
        pub kind: CodeKind,
        /// Current basic-block index.
        pub basic_block: Option<usize>,
        /// Retained jump history.
        pub context: Vec<usize>,
        /// Bottom-to-top abstract stack.
        pub stack: Vec<ValueInfo>,
        /// Index in byte_arrays.
        pub memory: Option<usize>,
        /// Index in byte_arrays.
        pub calldata: Option<usize>,
        /// Index in byte_arrays.
        pub returndata: Option<usize>,
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
        /// Complete path formula paired with this state.
        pub path: crate::PathRef,
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
        /// Application-owned postprocessing allowance, separate from execution and export.
        pub postprocessing_max_work: u64,
        /// Charged postprocessing work; does not certify execution closure or source identity.
        pub postprocessing_work: u64,
        /// Frozen processing and queue durations; excluded from world identity.
        pub timings: crate::AnalysisTimings,
        /// Frozen projection policy, independent of execution limits.
        pub export_limits: crate::ExportLimits,
        /// Projection counters; work is informational, separate from the postprocessing ledger.
        pub export_usage: crate::ExportUsage,
    }
}
