//! Trusted value identity, independently of numeric constraints and source labels.

use serde::Serialize;

/// Stable identity of an immutable input shared by every frame and block.
///
/// Equal identities assert equal concrete values within one environment. The
/// environment is part of summary qualification; arithmetic creates fresh values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Symbol {
    /// Logical root ADDRESS when analyzing bytecode without a destination.
    To,
    /// Unspecified root caller, also the default transaction origin.
    Caller,
    /// Independently specified symbolic transaction origin.
    Origin,
    /// Symbolic block beneficiary.
    Coinbase,
    /// Root frame's immutable call value.
    CallValue,
    /// Effective transaction gas price.
    GasPrice,
    /// Block timestamp.
    Timestamp,
    /// Block number.
    Number,
    /// Block randomness.
    Prevrandao,
    /// Block gas limit.
    GasLimit,
    /// Execution chain identifier when no anchored identity is available.
    ChainId,
    /// Block base fee.
    BaseFee,
    /// Blob base fee.
    BlobBaseFee,
    /// Root calldata length.
    CalldataLength,
    /// One root calldata word at an exact byte offset.
    CalldataWord(alloy_primitives::U256),
    /// A valid historical block hash at an exact height.
    BlockHash(alloy_primitives::U256),
    /// A blob versioned hash at an exact index.
    BlobHash(alloy_primitives::U256),
}

/// Immutable input identity scoped to one environment; equal names in separate
/// scopes never establish equal runtime values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct InputIdentity {
    #[serde(skip)]
    scope: u64,
    name: Symbol,
}
impl InputIdentity {
    pub(crate) fn new(scope: u64, name: Symbol) -> Self {
        Self { scope, name }
    }
    /// Input namespace allocated by the environment.
    pub fn scope(&self) -> u64 {
        self.scope
    }
    /// Input's name within its namespace.
    pub fn symbol(&self) -> Symbol {
        self.name
    }
}

/// Temporary definition identity valid within one basic-block execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeIdentity {
    scope: u64,
    definition: u32,
}
impl RuntimeIdentity {
    pub(crate) fn new(scope: u64, definition: u32) -> Self {
        Self { scope, definition }
    }
}

/// Must-alias identity accompanying an abstract value.
///
/// Persistent equality includes only immutable inputs. Temporary definitions
/// can prove copies equal while executing a block, but never alter stored state
/// equality, serialization, or joins.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ValueIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<InputIdentity>,
    #[serde(skip)]
    runtime: Option<RuntimeIdentity>,
}
impl PartialEq for ValueIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.input == other.input
    }
}
impl Eq for ValueIdentity {}

impl ValueIdentity {
    /// No trusted value identity.
    pub fn none() -> Self {
        Self::default()
    }
    /// Immutable input identity, if established by the environment.
    pub fn input(&self) -> Option<&InputIdentity> {
        self.input.as_ref()
    }
    /// Same immutable input or same temporary definition, never merely the same
    /// scalar constraints or source labels.
    pub fn same_identity(&self, other: &Self) -> bool {
        self.same_symbol(other) || (self.runtime.is_some() && self.runtime == other.runtime)
    }
    /// Same scoped immutable input; temporary definitions are excluded.
    pub fn same_symbol(&self, other: &Self) -> bool {
        self.input.is_some() && self.input == other.input
    }
    /// Keep only identities guaranteed on both incoming paths.
    pub fn join(&self, other: &Self) -> Self {
        Self {
            input: self.input.filter(|input| Some(*input) == other.input),
            runtime: None,
        }
    }
    pub(crate) fn with_input(mut self, input: InputIdentity) -> Self {
        self.input = Some(input);
        self
    }
    pub(crate) fn with_runtime(mut self, runtime: RuntimeIdentity) -> Self {
        self.runtime = Some(runtime);
        self
    }
    pub(crate) fn forget_runtime(&mut self) {
        self.runtime = None;
    }
    pub(crate) fn persistent_empty(&self) -> bool {
        self.input.is_none()
    }
}

#[cfg(test)]
mod tests;
