//! Domain-dependent copy quotation above the shared resource boundary.

use crate::{EvmValue, WordElement};
use evm_resource::{ResourceError, WorkBudget, checkpoint, finish_quote};

/// Quote only declared cached copy metadata, after the owner's payload census.
/// No clone, singleton discovery or observation is allowed during quotation.
pub fn value_work<N: WordElement>(
    value: &EvmValue<N>,
    budget: &WorkBudget,
) -> Result<usize, ResourceError> {
    checkpoint(budget)?;
    finish_quote(value.clone_work(), budget)
}
