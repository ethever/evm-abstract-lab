//! Explicit analysis input and bounded execution/acquisition policy.

variant! {
    /// Single submitted runtime program or an address at a pinned RPC snapshot.
    pub enum AnalysisInput {
        /// Runtime bytecode supplied by the user.
        Bytecode(BytecodeInput),
        /// Trusted ordinary RPC acquisition and native world execution.
        Rpc(RpcInput),
    }
}
record! {
    /// Standalone bytecode input; an omitted address retains a symbolic ADDRESS.
    pub struct BytecodeInput {
        /// Hexadecimal runtime bytecode.
        pub bytecode: String,
        /// Optional concrete root address.
        pub address: Option<String>,
    }
}
variant! {
    /// A block selector resolved exactly once before account reads.
    pub enum BlockSelector {
        /// Resolve the latest canonical block once.
        Latest,
        /// Resolve this block number once.
        Number(u64),
        /// Use this exact 32-byte block hash.
        Hash(String),
    }
}
record! {
    /// Explicit initial observations in addition to the root account.
    pub struct AccountQuery {
        /// Account address.
        pub address: String,
        /// Concrete initial storage keys, in hexadecimal.
        pub slots: Vec<String>,
    }
}
record! {
    /// Native RPC analysis through a server-configured provider.
    pub struct RpcInput {
        /// Stable provider ID selected from the backend catalogue.
        pub provider_id: String,
        /// Root execution/storage account.
        pub address: String,
        /// Block to pin.
        pub block: BlockSelector,
        /// Optional seed accounts and storage observations.
        pub accounts: Vec<AccountQuery>,
    }
}
variant! {
    /// A scalar transaction input observation.
    pub enum WordInput {
        /// Deliberately unobserved word.
        Unknown,
        /// A concrete 256-bit word as hexadecimal text.
        Concrete(String),
    }
}
variant! {
    /// Address inputs distinguish unknown identity from a concrete account.
    pub enum AddressSetting {
        /// Keep this environment field symbolic.
        Unknown,
        /// Concrete 20-byte address.
        Concrete(String),
    }
}
variant! {
    /// Calldata can remain symbolic or be an exact byte sequence.
    pub enum CalldataInput {
        /// Unknown bytes and length.
        Unknown,
        /// Exact hexadecimal bytes; empty text denotes no calldata.
        Exact(String),
    }
}
record! {
    /// A supplied historical block hash or transaction blob hash.
    pub struct IndexedHash {
        /// Block height or blob index, as a 256-bit hexadecimal word.
        pub index: String,
        /// Exact 32-byte hash.
        pub hash: String,
    }
}
record! {
    /// Transaction observations and explicit overrides of pinned block fields.
    /// Optional block fields inherit RPC header facts when absent; supplied
    /// values are explicit execution overrides, separate from snapshot identity.
    pub struct EnvironmentInput {
        /// Root caller.
        pub caller: AddressSetting,
        /// None preserves the caller's identity as the transaction origin.
        pub origin: Option<AddressSetting>,
        /// Root CALLVALUE.
        pub call_value: WordInput,
        /// Root input bytes.
        pub calldata: CalldataInput,
        /// Whether the root executes in static mode.
        pub is_static: bool,
        /// Optional initial upper bound for GAS; not exact gas accounting.
        pub gas_upper_bound: Option<String>,
        /// Effective transaction gas price.
        pub gas_price: WordInput,
        /// Block beneficiary override.
        pub coinbase: Option<String>,
        /// Block timestamp override.
        pub timestamp: Option<String>,
        /// Block number override.
        pub number: Option<String>,
        /// Block randomness override.
        pub prevrandao: Option<String>,
        /// Block gas limit override.
        pub gas_limit: Option<String>,
        /// Execution CHAINID override, distinct from the RPC snapshot chain identity.
        pub chain_id: Option<String>,
        /// Base fee override.
        pub base_fee: Option<String>,
        /// Blob base fee override.
        pub blob_base_fee: Option<String>,
        /// Explicit historical block hashes.
        pub block_hashes: Vec<IndexedHash>,
        /// Explicit transaction blob hashes.
        pub blob_hashes: Vec<IndexedHash>,
        /// Number of transaction blobs, independently known or unknown.
        pub blob_count: WordInput,
    }
}
impl Default for EnvironmentInput {
    fn default() -> Self {
        Self {
            caller: AddressSetting::Unknown,
            origin: None,
            call_value: WordInput::Unknown,
            calldata: CalldataInput::Unknown,
            is_static: false,
            gas_upper_bound: None,
            gas_price: WordInput::Unknown,
            coinbase: None,
            timestamp: None,
            number: None,
            prevrandao: None,
            gas_limit: None,
            chain_id: None,
            base_fee: None,
            blob_base_fee: None,
            block_hashes: Vec::new(),
            blob_hashes: Vec::new(),
            blob_count: WordInput::Unknown,
        }
    }
}
choice! {
    /// Numeric abstraction selected for this request.
    pub enum DomainProfile {
        /// Product of constants, bits, intervals and congruences.
        Product,
        /// Finite constants baseline.
        ConstantsOnly,
    }
}
choice! {
    /// Explicit native SMT provider; no silent provider fallback.
    pub enum SmtProvider {
        /// Z3.
        Z3,
        /// Bitwuzla.
        Bitwuzla,
        /// cvc5.
        Cvc5,
    }
}
record! {
    /// All execution, precision and acquisition controls.
    pub struct AnalysisLimits {
        /// Maximum graph states.
        pub max_states: usize,
        /// Maximum cumulative transfers.
        pub max_transfers: usize,
        /// Retained internal jump history.
        pub context_depth: usize,
        /// Constants per abstract value.
        pub max_constants: usize,
        /// Maximum cumulative logical execution work.
        pub max_work: usize,
        /// Maximum simultaneously retained frames.
        pub max_call_depth: usize,
        /// Maximum modeled byte-array span.
        pub max_memory_bytes: usize,
        /// Enable complete callee summary reuse.
        pub use_summaries: bool,
        /// Numeric abstraction.
        pub domain_profile: DomainProfile,
        /// Fact reduction rounds.
        pub reduction_rounds: usize,
        /// Maximum retained scalar facts.
        pub max_facts: usize,
        /// Enable relational SMT queries.
        pub relations_enabled: bool,
        /// Maximum retained relational constraints.
        pub max_constraints: usize,
        /// Expression and encoding node bound.
        pub max_expression_nodes: usize,
        /// Expression nesting bound.
        pub max_expression_depth: usize,
        /// Explicit solver provider.
        pub smt_provider: SmtProvider,
        /// Per-check provider resource allowance, not milliseconds.
        pub smt_rlimit: u32,
        /// Maximum acquired accounts.
        pub rpc_max_accounts: usize,
        /// Maximum cumulative HTTP requests.
        pub rpc_max_requests: usize,
        /// Maximum accepted bytes per RPC response.
        pub rpc_max_response_bytes: usize,
        /// Per-request RPC timeout in milliseconds.
        pub rpc_timeout_ms: u64,
    }
}
impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_states: 512,
            max_transfers: 10_000,
            context_depth: 0,
            max_constants: 8,
            max_work: 20_000_000,
            max_call_depth: 32,
            max_memory_bytes: 65_536,
            use_summaries: true,
            domain_profile: DomainProfile::Product,
            reduction_rounds: 4,
            max_facts: 256,
            relations_enabled: true,
            max_constraints: 128,
            max_expression_nodes: 1024,
            max_expression_depth: 64,
            smt_provider: SmtProvider::Z3,
            smt_rlimit: 100_000,
            rpc_max_accounts: 256,
            rpc_max_requests: 16_384,
            rpc_max_response_bytes: 4 * 1024 * 1024,
            rpc_timeout_ms: 15_000,
        }
    }
}
record! {
    /// Validated by the server before a bounded native job begins.
    pub struct AnalyzeRequest {
        /// Submitted program or server-configured snapshot provider.
        pub input: AnalysisInput,
        /// Fixed execution rules.
        pub fork: crate::Fork,
        /// Root transaction and block observations.
        pub environment: EnvironmentInput,
        /// Complete bounded policy.
        pub limits: AnalysisLimits,
    }
}
impl Default for AnalyzeRequest {
    fn default() -> Self {
        Self {
            input: AnalysisInput::Bytecode(BytecodeInput {
                bytecode: "600160020100".into(),
                address: None,
            }),
            fork: crate::Fork::Osaka,
            environment: EnvironmentInput::default(),
            limits: AnalysisLimits::default(),
        }
    }
}
