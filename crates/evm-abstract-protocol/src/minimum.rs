//! A usable semantic publication before optional machine/history/SSA export.
//!
//! Every numerical row belongs to the adjacent immutable procedure relation.
//! No field refers to a private machine, symbolic pool or an invented execution.

mod numeric;
mod validation;
pub use numeric::*;

choice! {
    /// Structural minimum publication failure; validation never mints closure.
    pub enum MinimumValidationKind {
        /// Snapshot, namespace or execution qualification is inconsistent.
        Identity,
        /// The captured numerical policy has an invalid structural shape.
        DomainSpec,
        /// A closed observation has no retained producer generation/root.
        Closure,
        /// Numerical row and relation indices or output alternatives disagree.
        Bounds,
        /// A current return delivery refers to a different output generation.
        Revision,
        /// Numerical payload contains a private symbolic reference.
        PrivateState,
        /// A sparse numerical key occurs more than once.
        Duplicate,
        /// Source/owner/static qualification disagrees with the declared relation.
        Source,
        /// Actual usage exceeds the declared bounded export policy.
        Export,
    }
}

variant! {
    /// Rich presentation has no authority over a retained minimum model.
    pub enum RichExportOutcome {
        /// Full presentation has not been requested for this publication.
        NotRequested,
        /// All requested rich graphs were exported successfully.
        Complete,
        /// The minimum remains available with this separate export failure.
        Refused(crate::ApiError),
        /// Cancellation retained the already published minimum.
        Cancelled,
    }
}
record! {
    /// Existing immutable table identity, independent of World and job versions.
    #[derive(Copy)]
    pub struct MinimumEvidenceVersion {
        /// Relative procedure table namespace.
        pub namespace: usize,
        /// Actual immutable table generation; zero never certifies closure.
        pub generation: usize,
    }
}
record! {
    /// Actual producer identity for one numerical procedure row.
    pub struct MinimumProcedureVersion {
        /// Relative procedure namespace.
        pub namespace: usize,
        /// Covering entry revision.
        pub entry_revision: usize,
        /// Kernel output slot version, losslessly encoded.
        pub output_revision: crate::DecimalU64,
        /// Frozen source/owner vocabulary, when present.
        pub source_epoch: Option<String>,
    }
}
record! {
    /// Data and physical effects from the same termination alternative.
    pub struct MinimumOutput {
        /// Relative termination, whose caller owns rollback.
        pub kind: crate::OutcomeKind,
        /// Producer generation used by regional call-return receipts.
        pub generation: usize,
        /// Canonical return or revert byte/length bound.
        pub data: MinimumByteArray,
        /// Canonical physical owner/alias effects for this same outcome.
        pub store: MinimumStore,
    }
}
record! {
    /// Numerical bounds for the procedure at this index in relation.procedures.
    pub struct MinimumProcedureBounds {
        /// Immutable native producer qualification.
        pub version: MinimumProcedureVersion,
        /// Canonical whole-store entry invariant.
        pub input: MinimumStore,
        /// Joint data/store alternatives; never split into independent unions.
        pub outputs: Vec<MinimumOutput>,
    }
}
record! {
    /// One existing closure observation with its complete numerical bounds.
    pub struct MinimumSummary {
        /// Actual immutable library evidence identity.
        pub version: MinimumEvidenceVersion,
        /// Source/owner/static qualifications and real MAY control/call-return.
        pub relation: crate::AbstractSummary,
        /// Same order and length as relation.procedures.
        pub bounds: std::sync::Arc<Vec<MinimumProcedureBounds>>,
    }
}
record! {
    /// Effective original environment, without private symbolic roots.
    pub struct MinimumEnvironment {
        /// Original symbolic input namespace; absence grants no alias identity.
        pub input_scope: Option<crate::DecimalU64>,
        /// Logical root destination, including its symbolic identity.
        pub to: crate::AddressValue,
        /// Effective caller.
        pub caller: crate::AddressValue,
        /// Effective transaction origin.
        pub origin: crate::AddressValue,
        /// Root CALLVALUE numerical bound.
        pub call_value: crate::ValueInfo,
        /// Original input byte and length bounds.
        pub calldata: MinimumByteArray,
        /// Root static restriction.
        pub is_static: bool,
        /// Effective GAS upper bound; None is unknown.
        pub gas_upper_bound: Option<String>,
        /// Transaction gas price.
        pub gas_price: crate::ValueInfo,
        /// Block beneficiary.
        pub coinbase: crate::AddressValue,
        /// Effective block timestamp.
        pub timestamp: crate::ValueInfo,
        /// Effective block number.
        pub number: crate::ValueInfo,
        /// Effective block randomness.
        pub prevrandao: crate::ValueInfo,
        /// Effective block gas limit.
        pub gas_limit: crate::ValueInfo,
        /// Execution chain ID, distinct from snapshot identity.
        pub chain_id: crate::ValueInfo,
        /// Effective base fee.
        pub base_fee: crate::ValueInfo,
        /// Effective blob base fee.
        pub blob_base_fee: crate::ValueInfo,
        /// Fixed historical block observations.
        pub block_hashes: Vec<crate::IndexedHash>,
        /// Fixed transaction blob hashes.
        pub blob_hashes: Vec<crate::IndexedHash>,
        /// Effective blob count.
        pub blob_count: crate::ValueInfo,
    }
}
record! {
    /// Funded minimum, independently usable while rich graphs are unavailable.
    /// These read-only numerical bounds belong to the qualified library model;
    /// they do not express every symbolic correlation or form an executable seed
    /// or a separately solved fixed point. Outcome associations remain joint.
    pub struct MinimumModel {
        /// Graph namespace; MAY control is never Native SSA execution evidence.
        pub namespace: crate::AnalysisNamespace,
        /// Actual input/model scope.
        pub scope: crate::AnalysisScope,
        /// Execution rules fixed for this input.
        pub fork: crate::Fork,
        /// Exact original snapshot, including independent offline namespaces.
        pub source_snapshot: crate::RootTerminalSnapshot,
        /// Original immutable fact revision.
        pub revision: crate::DecimalU64,
        /// Original physical or namespace root binding.
        pub root_binding: crate::RootInputBinding,
        /// Original executable provenance; namespace bits are not an account.
        pub declared_root_source: crate::ProcessSource,
        /// Native entry coordinate qualified by root_binding.
        pub entry_address: String,
        /// Exact captured World fingerprint.
        pub fingerprint: String,
        /// Closure copied from the library, independent of worker/export state.
        pub status: crate::AnalysisStatus,
        /// Existing global/source currentness qualification.
        pub current: bool,
        /// Actual global execution configuration.
        pub config: crate::RootTerminalConfig,
        /// Selected numerical carrier.
        pub numeric_model: crate::NumericModel,
        /// Actual numerical policy captured by the immutable library model.
        pub domain_spec: crate::RootTerminalDomainSpec,
        /// Effective original root inputs, not a narrowed precision input.
        pub environment: MinimumEnvironment,
        /// Original World observations, distinct from joined procedure invariants.
        pub accounts: Vec<MinimumInputAccount>,
        /// Source-qualified relations and canonical numerical input/outputs.
        pub summaries: Vec<MinimumSummary>,
        /// Actual independent application allowance funding this publication.
        pub postprocessing_max_work: u64,
        /// Bounds applied to this mandatory projection.
        pub export_limits: crate::ExportLimits,
        /// Actual projection usage charged before publication.
        pub export_usage: crate::ExportUsage,
    }
}
