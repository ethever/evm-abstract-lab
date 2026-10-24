//! Original upper-bound currency is not relabelled at later RPC revisions.
choice! {
    /// How the original baseline covers the currently installed initial facts.
    pub enum RpcBaselineRelation {
        /// Original baseline has no closure certificate.
        Uncertified,
        /// Original and current initial-fact revisions are identical.
        SameRevision,
        /// Accepted exact facts restrict the original initial-state set.
        InitialFactRestriction,
    }
}
choice! {
    /// Actual cumulative native limit, without a transport-frontier substitution.
    pub enum HybridBudget {
        /// State allocation cap.
        States,
        /// Transfer cap.
        Transfers,
        /// Shared work cap.
        Work,
        /// Call depth cap.
        CallDepth,
        /// Memory extraction cap.
        Memory,
        /// Model boundary.
        Model,
    }
}
variant! {
    /// Why optional fixed-snapshot precision ended.
    pub enum RpcPrecisionOutcome {
        /// Optional precision was disabled.
        Disabled,
        /// The original baseline remains uncertified.
        BaselinePending,
        /// Native detail is current at the installed revision.
        Current,
        /// Actual precision obligations remain pending.
        Incomplete,
        /// The retained work ledger refused the next operation.
        Work,
        /// A cumulative native limit prevented further precision.
        Budget(HybridBudget),
        /// Actual fixed-snapshot acquisition failed.
        AcquisitionFailed,
        /// Projection, refinement or analyzer preparation failed.
        PreparationFailed,
    }
}
record! {
    /// One atomically accepted restriction at the same snapshot.
    pub struct RpcFactRestriction {
        /// Previous initial-fact revision.
        pub previous: crate::DecimalU64,
        /// Installed restricted initial-fact revision.
        pub current: crate::DecimalU64,
        /// Additional exact observations, including known zeros.
        pub added_observations: u64,
    }
}
record! {
    /// Original baseline, current facts and native detail retain separate currency.
    pub struct RpcHybridCoverage {
        /// Identical fixed snapshot across original input and restrictions.
        pub snapshot: crate::RootTerminalSnapshot,
        /// Original baseline's actual source-guard revision.
        pub baseline_revision: crate::DecimalU64,
        /// Last atomically installed initial-fact revision.
        pub current_revision: crate::DecimalU64,
        /// Last retained native graph's actual revision.
        pub detail_revision: Option<crate::DecimalU64>,
        /// Binding equality only; does not certify execution/source closure.
        pub detail_at_current_revision: bool,
        /// Actual upper-bound qualification from the Session adapter.
        pub baseline_relation: RpcBaselineRelation,
        /// Accepted restriction chain; failed refinements cannot add a receipt.
        pub restrictions: Vec<RpcFactRestriction>,
        /// Independent optional precision outcome.
        pub precision: RpcPrecisionOutcome,
        /// Actual cumulative RPC acquisition evidence.
        pub acquisition: crate::AcquisitionReport,
        /// Total work across all phases, acquisitions and discarded rounds.
        pub work: u64,
        /// Accepted optional work across every native/refinement round.
        pub precision_work: u64,
        /// Declared optional cap, losslessly encoded.
        pub max_precision_work: crate::DecimalU64,
        /// Cumulative transfers, including discarded source revisions.
        pub transfers: u64,
        /// Actual sanitized errors, never hidden by a retained baseline.
        pub failures: Vec<crate::RpcFailure>,
        /// Original typed late preparation failure, if any.
        pub preparation_failure: Option<Box<crate::ApiError>>,
    }
}
