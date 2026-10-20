//! Local precision and global closure are independent results.

/// Completion of one local job, not a feasibility or global closure proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalClosure {
    /// All registered rules required for this job have stabilized.
    Stable,
    /// A sound coarse answer was accepted after optional precision was limited.
    /// Required unrepresented behavior must instead remain an obligation.
    Limited,
}

/// Counts explaining why a joint fixed point remains open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Open {
    /// Jobs queued for evaluation, including dirty in-flight jobs.
    pub pending: usize,
    /// Jobs currently evaluating.
    pub in_flight: usize,
    /// Evaluations abandoned before successful completion.
    pub interrupted: usize,
    /// Read answer cells for which no value or explicit absence was published.
    pub missing: usize,
    /// Generations whose job/provider admission has not been sealed.
    pub unsealed: usize,
    /// Adapter-reported uncovered semantic obligations.
    pub obligations: usize,
}

impl Open {
    pub(crate) fn is_empty(self) -> bool {
        self == Self::default()
    }
}

/// Closure of all registered propagation, provider and semantic obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closure {
    /// No registered mandatory work or uncovered semantic obligation remains.
    Closed,
    /// Pending work or a semantic boundary prevents claiming closure.
    Open(Open),
}

/// Result of a bounded joint scheduling run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SolveReport {
    /// Closure after this run, including work outside the step budget.
    pub closure: Closure,
    /// Whether any completed active job accepted an optional precision limit.
    pub local_limits: bool,
    /// Jobs evaluated during this call; no budget or counter is reset.
    pub steps: usize,
}
