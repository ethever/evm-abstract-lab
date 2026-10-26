//! Allocation-free protocol tariffs shared by reservations and adapter bounds.
//!
//! Inputs are collection sizes/capacities or caller-owned key/value work, never
//! semantic operations. Adapters prove their topology and step bounds separately.
//! Saturation returns `usize::MAX`; the kernel refuses that sentinel before any
//! operation or cancellation checkpoint. Opaque carrier preparation and temporary
//! destruction remain the adapter/policy's responsibility.

/// Tree lookup/insertion bound, including an empty index.
pub const fn tree_work(len: usize) -> usize {
    len.saturating_add(1).ilog2() as usize + 2
}

/// Create a missing scope after its separately paid directory lookup.
pub const fn scope_insert_work(scopes: usize) -> usize {
    7usize.saturating_add(tree_work(scopes))
}

/// Allocate a typed channel and its global directory fact.
pub const CHANNEL_WORK: usize = 4;
/// Seal a scope's initial registration inventory.
pub const SEAL_SCOPE_WORK: usize = 1;
/// Retain an opaque task's shared handle without cloning its payload.
pub const TASK_HANDLE_WORK: usize = 1;
/// Begin an evaluation already removed from the pending queue by admission.
pub const ADMITTED_BEGIN_WORK: usize = 1;
/// Complete an evaluation, retaining any independently queued self-wake.
pub const FINISH_WORK: usize = 1;

/// Queue insertion bound, including every geometric reallocation in a batch.
///
/// `pending <= capacity` describes the live queue. Use `capacity = pending` for
/// a capacity-independent upper bound. Duplicate identities are over-reserved.
pub const fn enqueue_work(pending: usize, capacity: usize, additional: usize) -> usize {
    let Some(spare) = capacity.checked_sub(pending) else {
        return usize::MAX;
    };
    let maximum = pending.saturating_add(additional);
    let insertions = additional.saturating_mul(tree_work(maximum).saturating_add(6));
    if additional <= spare {
        return insertions;
    }
    // Explicit doubling copies less than twice the maximum live length across
    // a batch. Each node owns three fields, all prepaid before the first wake.
    insertions.saturating_add(maximum.saturating_mul(6))
}

/// Queue removal, moved-node lookup and constant link repairs.
pub const fn removal_work(pending: usize) -> usize {
    tree_work(pending).saturating_mul(2).saturating_add(12)
}

/// One eligibility visit per live queue node and removal of the chosen node.
pub const fn selection_work(pending: usize) -> usize {
    pending.saturating_add(removal_work(pending))
}

/// Priority and eligibility visits per live queue node, then removal.
pub const fn priority_selection_work(pending: usize) -> usize {
    pending
        .saturating_mul(2)
        .saturating_add(removal_work(pending))
}

/// Register a job and queue its first obligation; opaque task work is separate.
pub const fn registration_work(scope_jobs: usize, enqueue: usize) -> usize {
    7usize
        .saturating_add(tree_work(scope_jobs))
        .saturating_add(enqueue)
}

/// One admitted solver step outside the analyzer's own work.
pub const fn dispatch_work(pending: usize) -> usize {
    selection_work(pending)
        .saturating_add(TASK_HANDLE_WORK)
        .saturating_add(ADMITTED_BEGIN_WORK)
        .saturating_add(FINISH_WORK)
}

/// Create a missing channel-local scope directory after its paid lookup.
pub const fn channel_scope_insert_work(scopes: usize, scope_facts: usize) -> usize {
    5usize
        .saturating_add(tree_work(scopes))
        .saturating_add(tree_work(scope_facts))
}

/// Locate a capability key, paying opaque key comparisons without cloning it.
pub const fn key_lookup_work(keys: usize, key: usize) -> usize {
    tree_work(keys)
        .saturating_mul(if key == 0 { 1 } else { key })
        .saturating_mul(3)
}

/// Create a missing typed cell after its paid lookup, including its owned key.
pub const fn key_insert_work(keys: usize, key: usize, scope_facts: usize) -> usize {
    5usize
        .saturating_add(tree_work(keys))
        .saturating_add(key)
        .saturating_add(tree_work(scope_facts))
}

/// Record a wake observation, including a repeat observation of an existing edge.
pub const fn observation_work(reads: usize, readers: usize) -> usize {
    tree_work(reads)
        .saturating_mul(2)
        .saturating_add(tree_work(readers))
        .saturating_add(2)
}

/// Additional required/demand indexes for a required observation.
pub const fn requirement_work(required: usize, demands: usize) -> usize {
    tree_work(required)
        .saturating_add(tree_work(demands))
        .saturating_add(2)
}

/// Classify one semantic seed and traverse its reverse assumption edges.
pub const fn semantic_seed_work(assumptions: usize, scopes: usize) -> usize {
    assumptions
        .saturating_mul(tree_work(scopes).saturating_mul(4).saturating_add(9))
        .saturating_add(4)
}

/// Prepare changed facts, including at most two facts per invalidated scope.
pub const fn change_plan_work(maximum: usize) -> usize {
    maximum
        .saturating_mul(tree_work(maximum).saturating_add(3))
        .saturating_add(1)
}

/// Install a value, prepaying destruction of the retained old carrier too.
pub const fn publication_value_work(value: usize, old_drop: usize) -> usize {
    value.saturating_add(old_drop).saturating_add(1)
}

/// Install changed facts and visit their readers after the separately paid plan.
///
/// `enqueue` must cover every wake, including duplicate and stale readers.
/// `extra` includes value/drop, coverage, and invalidated-job removal work.
pub const fn change_install_work(
    facts: usize,
    wakes: usize,
    enqueue: usize,
    extra: usize,
) -> usize {
    extra
        .saturating_add(facts.saturating_mul(2))
        .saturating_add(enqueue)
        .saturating_add(wakes.saturating_mul(3))
}

/// Constant-time report metadata quote, paid before scanning nested adjacency.
pub const fn report_metadata_work(facts: usize) -> usize {
    facts.saturating_add(1)
}

/// Counts needed by the solver report; read-only collection remains caller-paid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReportCounts {
    /// Live pending queue nodes.
    pub pending: usize,
    /// All retained job records, including tombstones.
    pub jobs: usize,
    /// All retained fact records, including tombstones.
    pub facts: usize,
    /// All retained scope records, including tombstones.
    pub scopes: usize,
    /// Sum of fact reader counts.
    pub readers: usize,
    /// Sum of producer-family coverage counts across facts.
    pub coverage: usize,
}

/// Solver report traversal, including nested readers and producer guards.
pub const fn report_work(counts: ReportCounts) -> usize {
    16usize
        .saturating_add(counts.pending.saturating_mul(4))
        .saturating_add(counts.jobs.saturating_mul(8))
        .saturating_add(counts.facts.saturating_mul(12))
        .saturating_add(counts.scopes.saturating_mul(4))
        .saturating_add(counts.readers.saturating_mul(6))
        .saturating_add(counts.coverage.saturating_mul(8))
}
