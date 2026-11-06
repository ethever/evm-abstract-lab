//! Wide model counts cross into native execution only at real native consumers.

use evm_resource::WorkBudget;
use std::num::NonZeroUsize;

#[cfg(test)]
mod tests;

/// Collection sizes and paid native work fit the model on supported 32/64-bit targets.
pub(crate) fn count(value: usize) -> u64 {
    u64::try_from(value).expect("native decompiler counts fit u64")
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NativeLimitError {
    #[error("{field}={value} cannot be represented by the native {bits}-bit execution boundary")]
    Unrepresentable {
        field: &'static str,
        value: u64,
        bits: u32,
    },
    #[error("max_constant_set must be nonzero")]
    ZeroConstantSet,
}

fn native<T: TryFrom<u64>>(value: u64, field: &'static str) -> Result<T, NativeLimitError> {
    T::try_from(value).map_err(|_| NativeLimitError::Unrepresentable {
        field,
        value,
        bits: u32::try_from(std::mem::size_of::<T>() * 8).expect("native width fits u32"),
    })
}

pub(crate) fn work_budget(max_work: u64) -> Result<WorkBudget, NativeLimitError> {
    native::<usize>(max_work, "max_work").map(WorkBudget::new)
}

pub(crate) fn constant_capacity(value: u64) -> Result<NonZeroUsize, NativeLimitError> {
    if value == 0 {
        return Err(NativeLimitError::ZeroConstantSet);
    }
    // Validate the domain policy before narrowing; the capacity never allocates.
    Ok(NonZeroUsize::new(native::<usize>(value, "max_constant_set")?).expect("checked nonzero"))
}
