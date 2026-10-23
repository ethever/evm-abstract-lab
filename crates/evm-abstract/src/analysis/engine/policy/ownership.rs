//! Ownership adapters retain the full obligation when preparation refuses.

use crate::{
    analysis::{EvmMachinePayload, FrameKey, MachineError, MachineKey},
    domain::{EvmNumericAlgebra, WordElement},
};

use super::{Partitioned, PartitionedRelative, PolicyContext, RelativeState};

/// Refused preparation with the complete original input still owned by caller.
/// A refusal is an unfinished obligation, never a bottom or unreachable state.
#[derive(Debug)]
pub struct PartitionRefusal<T> {
    /// The input that could not be prepared.
    pub input: T,
    /// Typed shape, capacity, cancellation or work refusal.
    pub error: MachineError,
}

/// Owned machine preparation, retaining the original obligation on refusal.
pub type PartitionResult<K, N> = Result<Partitioned<K, N>, PartitionRefusal<EvmMachinePayload<N>>>;

/// Owned relative preparation with the same ownership/refusal boundary.
pub type RelativePartitionResult<K, N> =
    Result<PartitionedRelative<K, N>, PartitionRefusal<RelativeState<N>>>;

fn frame_work(frame: &FrameKey) -> usize {
    std::mem::size_of::<FrameKey>()
        .div_ceil(std::mem::size_of::<usize>())
        .saturating_add(frame.jump_history.len())
        .saturating_mul(2)
        .saturating_add(frame.process_work())
}

/// The default key copies structural coordinates and hashes code/lifecycle
/// facts. It does not clone stacks, byte histories, stores or rollback states.
fn machine_key<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    input: &EvmMachinePayload<N>,
    cx: &mut PolicyContext<'_, D>,
) -> Result<MachineKey, MachineError> {
    let entries = input.store.code_identity_entries();
    cx.reserve(
        entries
            .saturating_add(input.call_stack.depth())
            .saturating_add(1),
    )?;
    let frame_work = input.call_stack.iter().fold(1usize, |work, frame| {
        work.saturating_add(frame_work(&frame.key))
    });
    // At most nine address-tree operations per entry, a bounded-width key
    // comparison at each tree level, temporary records and their disposal.
    let key_work = entries
        .saturating_mul((usize::BITS as usize).saturating_add(1))
        .saturating_mul(9)
        .saturating_add(
            entries
                .saturating_mul(20 + 1 + 32 + 3)
                .saturating_add(2)
                .saturating_mul(4),
        )
        .saturating_add(input.store.code_identity_bytes().saturating_mul(2))
        .saturating_add(frame_work);
    cx.reserve(key_work)?;
    let key = input.key();
    cx.reserve(0)?;
    Ok(key)
}

#[expect(
    clippy::result_large_err,
    reason = "refusal returns the original input without allocating after work or cancellation refusal"
)]
pub(super) fn structural<D: EvmNumericAlgebra, Tag>(
    input: EvmMachinePayload<D::Element>,
    class: Tag,
    cx: &mut PolicyContext<'_, D>,
) -> PartitionResult<(MachineKey, Tag), D::Element> {
    match machine_key(&input, cx) {
        Ok(key) => Ok(Partitioned {
            key: (key, class),
            payload: input,
        }),
        Err(error) => Err(PartitionRefusal { input, error }),
    }
}

#[expect(
    clippy::result_large_err,
    reason = "refusal returns the original input without allocating after work or cancellation refusal"
)]
pub(super) fn structural_relative<D: EvmNumericAlgebra, Tag>(
    input: RelativeState<D::Element>,
    class: Tag,
    cx: &mut PolicyContext<'_, D>,
) -> RelativePartitionResult<(MachineKey, Tag), D::Element> {
    let mut prepare = || {
        cx.reserve(input.key().frames.len().saturating_add(1))?;
        let work = input
            .key()
            .frames
            .iter()
            .fold(1usize, |work, frame| work.saturating_add(frame_work(frame)));
        cx.reserve(work)?;
        let key = input.key().clone();
        cx.reserve(0)?;
        Ok(key)
    };
    match prepare() {
        Ok(key) => Ok(PartitionedRelative {
            key: (key, class),
            payload: input,
        }),
        Err(error) => Err(PartitionRefusal { input, error }),
    }
}

#[cfg(test)]
mod tests;
