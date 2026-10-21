//! Native-anchored terminal coverage. These display projections do not define a
//! broader input set when a scalar or byte-array projection loses precision.

/// Lossless process identity encoded only as a canonical decimal string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64(pub u64);
impl std::fmt::Display for DecimalU64 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}
impl serde::Serialize for DecimalU64 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> serde::Deserialize<'de> for DecimalU64 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(serde::de::Error::custom(
                "expected canonical decimal u64 string",
            ));
        }
        let value: u64 = text
            .parse()
            .map_err(|_| serde::de::Error::custom("decimal identity exceeds u64"))?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom(
                "expected canonical decimal u64 string",
            ));
        }
        Ok(Self(value))
    }
}
choice! {
    /// Scope of the native terminal relation, separate from CFG/SSA coverage.
    pub enum RootTerminalScope {
        /// All states satisfying the actual immutable original root input.
        AllInputStates,
    }
}
choice! {
    /// Raw mandatory retention boundary, before frontier flattening.
    pub enum RootTerminalRetention {
        /// Required scalar expression retention.
        Expression,
        /// Required complete state retention.
        State,
    }
}
record! {
    /// Native interrupted detailed instruction; this is no terminal state.
    pub struct RootTerminalRetentionTrigger {
        /// Actual detailed state.
        pub state: usize,
        /// Actual interrupted instruction offset.
        pub pc: usize,
        /// Original retention category.
        pub kind: RootTerminalRetention,
    }
}
variant! {
    /// Actual native boundary activating the prepared relation.
    pub enum RootTerminalTrigger {
        /// Mandatory detailed work was refused by the original ledger.
        Work,
        /// Mandatory summary work was refused by the original ledger.
        SummaryWork,
        /// State counter was exhausted.
        States,
        /// Transfer counter was exhausted.
        Transfers,
        /// Raw mandatory retention refusal.
        MandatoryRetention(RootTerminalRetentionTrigger),
    }
}
record! {
    /// Offline labels alone do not identify independent snapshots.
    pub struct RootTerminalOfflineSnapshot {
        /// Actual offline label.
        pub label: String,
        /// Actual private process namespace, retained losslessly.
        pub namespace: DecimalU64,
    }
}
record! {
    /// Full chain snapshot identity, independent of execution overrides.
    pub struct RootTerminalChainSnapshot {
        /// Complete 256-bit chain ID as hexadecimal text.
        pub chain_id: String,
        /// Exact pinned 32-byte hash.
        pub block_hash: String,
    }
}
variant! {
    /// Exact native snapshot identity rather than a hash-only authority claim.
    pub enum RootTerminalSnapshot {
        /// Independent offline namespace and label.
        Offline(RootTerminalOfflineSnapshot),
        /// Full chain ID and block hash.
        Chain(RootTerminalChainSnapshot),
    }
}
record! {
    /// Actual finite expression and relation policy.
    pub struct RootTerminalRelationLimits {
        /// Whether relational queries are enabled.
        pub enabled: bool,
        /// Maximum retained constraints.
        pub max_constraints: u64,
        /// Maximum traversal nodes.
        pub max_nodes: u64,
        /// Maximum nesting depth.
        pub max_depth: u64,
    }
}
record! {
    /// Complete actual execution policy, excluding RPC transport allowances.
    pub struct RootTerminalConfig {
        /// Actual jump history bound.
        pub context_depth: u64,
        /// Actual state bound.
        pub max_states: u64,
        /// Actual transfer bound.
        pub max_transfers: u64,
        /// Actual shared work bound.
        pub max_work: u64,
        /// Actual external frame bound.
        pub max_call_depth: u64,
        /// Actual byte extraction bound.
        pub max_memory_bytes: u64,
        /// Actual summary reuse setting.
        pub use_summaries: bool,
        /// Actual opt-in setting.
        pub root_terminal_coverage: bool,
    }
}
record! {
    /// Complete domain metadata captured by the native original entry lift.
    pub struct RootTerminalDomainSpec {
        /// Actual domain schema version.
        pub schema_version: u16,
        /// Actual word width.
        pub word_bits: u16,
        /// Actual constant capacity.
        pub capacity: u64,
        /// Actual reduction rounds.
        pub reduction_rounds: u64,
        /// Actual retained fact bound.
        pub fact_limit: u64,
        /// Actual logical cost version.
        pub cost_version: u16,
        /// Actual widening update threshold.
        pub widening_after_updates: u64,
        /// Actual provenance policy name.
        pub provenance_policy: String,
        /// Actual expression/relation policy.
        pub relations: RootTerminalRelationLimits,
    }
}
variant! {
    /// Exact native frame execution mode.
    pub enum RootTerminalFrameCode {
        /// Deployed runtime.
        Runtime,
        /// Creation initcode.
        InitCode,
        /// Native precompile address.
        Precompile(String),
        /// Known empty code.
        Empty,
        /// Invalid nested delegation.
        InvalidDelegation,
        /// Unknown implementation.
        Unknown,
    }
}
record! {
    /// Captured original FrameKey, independent of later account code effects.
    pub struct RootTerminalFrame {
        /// Physical executable owner.
        pub code_address: String,
        /// Captured code hash.
        pub code_hash: String,
        /// Exact executable interpretation.
        pub mode: RootTerminalFrameCode,
        /// Physical persistent/transient storage owner.
        pub address: String,
        /// Logical ADDRESS.
        pub address_value: crate::AddressValue,
        /// Logical CALLER.
        pub caller: crate::AddressValue,
        /// Original static mode.
        pub is_static: bool,
        /// Original program-local basic block.
        pub basic_block_index: usize,
        /// Original stack height.
        pub stack_height: usize,
        /// Actual retained jump history.
        pub jump_history: Vec<usize>,
    }
}
record! {
    /// Native assumptions shared jointly by all three terminal alternatives.
    /// The original input remains authoritative when its display is widened.
    pub struct RootTerminalInputBinding {
        /// Actual session snapshot, including an offline namespace.
        pub snapshot: RootTerminalSnapshot,
        /// Actual immutable fact revision, not a chain height.
        pub revision: DecimalU64,
        /// Actual immutable world storage-source token in the shared DAG.
        pub storage_source: crate::ExpressionRef,
        /// Actual original symbolic input namespace, when present.
        pub input_scope: Option<DecimalU64>,
        /// Actual execution fork.
        pub fork: crate::Fork,
        /// Original entry's physical owner.
        pub entry_address: String,
        /// Original effective environment before the entry lift.
        pub environment: crate::EnvironmentSnapshot,
        /// Complete actual native execution configuration.
        pub config: RootTerminalConfig,
        /// Descriptive application selection, not a native policy certificate.
        pub numeric_model: crate::NumericModel,
        /// Complete actual domain policy.
        pub domain_spec: RootTerminalDomainSpec,
        /// Captured original frame identity.
        pub frame: RootTerminalFrame,
        /// Original captured program in the shared directory.
        pub program: usize,
        /// Actual lifted original calldata in byte_arrays.
        pub calldata: usize,
        /// Actual lifted original CALLVALUE in the same symbolic DAG.
        pub call_value: crate::ValueInfo,
        /// Original pre-entry checkpoint in stores.
        pub saved_store: usize,
        /// Original entered Store before detailed execution, in stores.
        pub initial_store: usize,
    }
}
record! {
    /// One joint terminal tuple without a fabricated machine state or PC.
    pub struct RootTerminalOutcome {
        /// Return, Revert or Failure.
        pub kind: crate::OutcomeKind,
        /// Complete terminal bytes in byte_arrays; Failure is empty.
        pub data: usize,
        /// Joint finalized effects in stores.
        pub store: usize,
    }
}
record! {
    /// Separate native all-input terminal relation, not a convergence or
    /// feasibility witness and not a claim of total program termination.
    pub struct RootTerminalCoverage {
        /// Native semantic input scope.
        pub scope: RootTerminalScope,
        /// Actual immutable original input binding shared by all alternatives.
        pub input: RootTerminalInputBinding,
        /// Fixed native order: Return, Revert, Failure.
        pub outcomes: [RootTerminalOutcome; 3],
        /// Actual native activation cause.
        pub trigger: RootTerminalTrigger,
    }
}
choice! {
    /// Structural inconsistency in a projected native binding. Validation
    /// checks representation and references, not full EVM semantics.
    pub enum RootTerminalValidationFailure {
        /// Alternatives were reordered or relabeled.
        OutcomeOrder,
        /// Exceptional failure carries possibly nonempty data.
        FailureData,
        /// Captured snapshot disagrees with pinned report metadata.
        Snapshot,
        /// Captured fork disagrees with report decoding rules.
        Fork,
        /// Original physical/logical frame or source binding disagrees.
        Frame,
        /// Raw retention state/PC has no matching exported frontier.
        RetentionSite,
        /// Original effective environment disagrees with report metadata.
        Environment,
        /// Native execution settings disagree with effective report settings.
        Configuration,
        /// Captured domain metadata is inconsistent with execution settings.
        DomainSpec,
        /// A retained input identity contradicts its expression binding.
        InputIdentity,
    }
}
