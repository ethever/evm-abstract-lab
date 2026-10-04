//! Native calls use the pinned revm implementation for the selected fork. Work
//! is reserved before crypto or variable-size modular exponentiation begins.

use super::{Execution, TransferContext, boundary, calls::finish};
use crate::{
    Fork,
    analysis::{FrontierReason, OutcomeKind},
    world::ByteArray,
};
use alloy_primitives::{Address, U256};
use revm_precompile::{PrecompileSpecId, PrecompileStatus, Precompiles};

fn table(fork: Fork) -> &'static Precompiles {
    Precompiles::new(PrecompileSpecId::from_spec_id(fork.spec_id()))
}

pub(super) fn contains(fork: Fork, address: Address) -> bool {
    table(fork).contains(&address)
}

fn word(input: &[u8], start: usize) -> U256 {
    let mut bytes = [0u8; 32];
    let available = input.len().saturating_sub(start).min(32);
    if available == 0 {
        return U256::ZERO;
    }
    bytes[..available].copy_from_slice(&input[start..start + available]);
    U256::from_be_bytes(bytes)
}

// A pessimistic bound intentionally rejects work whose configured ledger
// cannot admit the native operation, without first allocating declared lengths.
fn cost(address: Address, input: &[u8], max_bytes: usize) -> Option<usize> {
    let number = U256::from_be_slice(address.as_slice());
    let bytes = input.len().saturating_add(1);
    if number == U256::from(5) {
        let lengths = [word(input, 0), word(input, 32), word(input, 64)];
        if lengths.iter().any(|length| *length > U256::from(max_bytes)) {
            return None;
        }
        let base = lengths[0].to::<usize>();
        let exponent = lengths[1].to::<usize>();
        let modulus = lengths[2].to::<usize>();
        return Some(
            bytes.saturating_add(
                base.max(modulus)
                    .max(1)
                    .saturating_pow(2)
                    .saturating_mul(exponent.max(1))
                    .saturating_mul(8),
            ),
        );
    }
    if number == U256::from(9) && input.len() >= 4 {
        let rounds = u32::from_be_bytes(input[..4].try_into().expect("four bytes")) as usize;
        return Some(bytes.saturating_add(rounds.saturating_mul(16)));
    }
    let multiplier = match number.to::<u64>() {
        1 | 6 | 7 | 0x100 => 1024,
        8 | 0x0a..=0x11 => 4096,
        _ => 4,
    };
    Some(bytes.saturating_mul(multiplier))
}

pub(super) fn execute(
    result: &mut Execution,
    fork: Fork,
    address: Address,
    context: &mut TransferContext<'_>,
) {
    let input_cost =
        result
            .payload
            .active()
            .calldata
            .work_size()
            .saturating_add(super::budget::maximum(
                result.payload.active().calldata.len(),
            ));
    if !context.budget.charge(input_cost) {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    let Some(input) = result
        .payload
        .active()
        .calldata
        .exact_bytes_bounded(context.config.max_memory_bytes)
    else {
        boundary(result, 0, FrontierReason::PrecompileInput(address));
        return;
    };
    let Some(cost) = cost(address, &input, context.config.max_memory_bytes) else {
        boundary(result, 0, FrontierReason::PrecompileInput(address));
        return;
    };
    if !context.budget.charge(cost) {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    let Some(native) = table(fork).get(&address) else {
        boundary(result, 0, FrontierReason::Precompile(address));
        return;
    };
    let (kind, data) = match native.execute(&input, u64::MAX, 0) {
        Ok(output) => match output.status {
            PrecompileStatus::Success => (OutcomeKind::Return, ByteArray::exact(&output.bytes)),
            PrecompileStatus::Revert => (OutcomeKind::Revert, ByteArray::exact(&output.bytes)),
            PrecompileStatus::Halt(_) => (OutcomeKind::Failure, ByteArray::empty()),
        },
        // Fatal backend errors are analysis boundaries, rather than invented
        // contract failures. Ordinary invalid input is returned as a Halt above.
        Err(_) => {
            boundary(result, 0, FrontierReason::Precompile(address));
            return;
        }
    };
    if super::budget::maximum(data.len()) > context.config.max_memory_bytes {
        boundary(result, 0, FrontierReason::Memory);
        return;
    }
    let payload = result.payload.clone();
    finish(result, payload, kind, data, context, 0);
}
