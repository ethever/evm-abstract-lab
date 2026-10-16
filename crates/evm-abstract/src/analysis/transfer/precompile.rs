//! Materializable calls use the pinned fork's native implementation. Symbolic
//! inputs have a pure, coarse Return/Revert/Failure summary. Admission reserves
//! retained representation work, never the symbolic logical length or EVM gas.

use super::{Execution, TransferContext, boundary, calls::finish};
use crate::{
    Fork,
    analysis::{FrontierReason, OutcomeKind},
    domain::{AbstractValue, Domain, symbolic::ExprLimits},
    world::{ByteArray, RangeError},
};
use alloy_primitives::{Address, U256};
use revm_precompile::{
    PrecompileId, PrecompileResult, PrecompileSpecId, PrecompileStatus, Precompiles,
};

#[cfg(test)]
mod tests;

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
    let input = &result.payload.active().calldata;
    // Cached counts prepay the representation scan, including exact buffers
    // reachable through views. Logical length contributes only if extracted.
    if !context.budget.charge(
        input
            .metadata_len()
            .saturating_add(input.state_root().work_size()),
    ) || !context.budget.charge(input.work_size())
    {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    if let Err(error) = input.check_existing(context.domain.spec().relations().expressions()) {
        boundary(result, 0, super::errors::bytes(error));
        return;
    }
    let Some(native) = table(fork).get(&address) else {
        boundary(result, 0, FrontierReason::Precompile(address));
        return;
    };
    let input = match materialize(input, context) {
        Ok(Some(input)) => input,
        Ok(None) => {
            summarize(result, native.id(), context);
            return;
        }
        Err(reason) => {
            boundary(result, 0, reason);
            return;
        }
    };
    let Some(cost) = cost(address, &input, context.config.max_memory_bytes) else {
        // A refused concrete native operation is an analysis boundary. It is
        // not evidence of symbolic input and must not trigger a summary.
        boundary(result, 0, FrontierReason::PrecompileInput(address));
        return;
    };
    if !context.budget.charge(cost) {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    finish_native(
        result,
        address,
        native.execute(&input, u64::MAX, 0),
        context,
    );
}

/// None means semantic abstraction or no attempted materialization, never a
/// swallowed cancellation, expression error, allocation refusal or work limit.
fn materialize(
    input: &ByteArray,
    context: &mut TransferContext<'_>,
) -> Result<Option<Vec<u8>>, FrontierReason> {
    let Some(length) = input
        .len()
        .singleton()
        .and_then(|n| usize::try_from(n).ok())
    else {
        return Ok(None);
    };
    if length > context.config.max_memory_bytes {
        // This policy bounds extraction, not the semantics of a native buffer.
        return Ok(None);
    }
    let work = length.saturating_mul(
        input
            .work_size()
            .saturating_add(context.domain.capacity().saturating_pow(2))
            .saturating_add(8),
    );
    if !context.budget.charge(work) {
        return Err(FrontierReason::Work);
    }
    let view = input.view();
    let limits = ExprLimits {
        max_nodes: length
            .saturating_mul(input.state_root().nodes().saturating_add(2))
            .max(view.nodes()),
        max_depth: input.state_root().depth().saturating_add(2),
    };
    if let Some(bytes) = view
        .known_bytes(limits)
        .map_err(super::errors::expression)?
    {
        return Ok(Some(bytes));
    }
    // Numerical facts can make an otherwise symbolic source materializable.
    // Checked reads preserve the failure that exact_bytes_bounded erases.
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| FrontierReason::Memory)?;
    for index in 0..length {
        let value = input
            .read_byte_checked(&AbstractValue::constant(U256::from(index)), context.domain)
            .map_err(super::errors::bytes)?;
        let Some(byte) = value.singleton().and_then(|n| u8::try_from(n).ok()) else {
            return Ok(None);
        };
        bytes.push(byte);
    }
    Ok(Some(bytes))
}

fn success_data(
    input: &ByteArray,
    id: &PrecompileId,
    domain: Domain,
) -> Result<ByteArray, RangeError> {
    // Match semantic identifiers, so fork-specific native functions still use
    // the provider table. All pinned fixed-output variants are covered here.
    let length = match id {
        PrecompileId::Identity => return Ok(input.clone()),
        PrecompileId::EcRec | PrecompileId::P256Verify => {
            // Invalid signatures can successfully return no bytes. Intermediate
            // lengths are harmless extra possibilities in this coarse summary.
            AbstractValue::unsigned_range(U256::ZERO, U256::from(32)).expect("ordered bounds")
        }
        PrecompileId::Sha256
        | PrecompileId::Ripemd160
        | PrecompileId::Bn254Pairing
        | PrecompileId::Bls12Pairing => AbstractValue::constant(U256::from(32)),
        PrecompileId::Bn254Add
        | PrecompileId::Bn254Mul
        | PrecompileId::Blake2F
        | PrecompileId::KzgPointEvaluation => AbstractValue::constant(U256::from(64)),
        PrecompileId::Bls12G1Add | PrecompileId::Bls12G1Msm | PrecompileId::Bls12MapFpToGp1 => {
            AbstractValue::constant(U256::from(128))
        }
        PrecompileId::Bls12G2Add | PrecompileId::Bls12G2Msm | PrecompileId::Bls12MapFp2ToGp2 => {
            AbstractValue::constant(U256::from(256))
        }
        // In particular, pre-Osaka modexp's modulus length is not bounded by
        // max_memory_bytes. General/future variants get no invented byte cap.
        _ => return ByteArray::unknown_checked(),
    };
    ByteArray::unknown_checked()?.slice(&AbstractValue::constant(U256::ZERO), &length, 256, domain)
}

fn summarize(result: &mut Execution, id: &PrecompileId, context: &mut TransferContext<'_>) {
    // Each of three alternatives retains at most four payload-sized copies:
    // input, completion receipt, resumed payload and output store/data. The
    // fixed allowance pays for fresh roots, one bounded slice and empty data.
    // No logical bytes are enumerated, allocated or charged by this summary.
    let work = result
        .payload
        .work_size()
        .saturating_mul(4)
        .saturating_add(1024)
        .saturating_mul(3);
    if !context.budget.charge(work) {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    // Construct and validate all buffers before publishing any alternative.
    let data = (|| {
        let returned = success_data(&result.payload.active().calldata, id, context.domain)?;
        let reverted = ByteArray::unknown_checked()?;
        let failed = ByteArray::empty();
        let limits = context.domain.spec().relations().expressions();
        for data in [&returned, &reverted, &failed] {
            data.check_existing(limits)?;
        }
        Ok::<_, RangeError>([returned, reverted, failed])
    })();
    let data = match data {
        Ok(data) => data,
        Err(error) => {
            boundary(result, 0, super::errors::bytes(error));
            return;
        }
    };
    for (kind, data) in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ]
    .into_iter()
    .zip(data)
    {
        if !context.budget.charge(0) {
            boundary(result, 0, FrontierReason::Work);
            return;
        }
        // Native operations are pure; the call layer owns touch metadata and
        // checkpoints. Its common finish handles root and child rollback.
        finish(result, result.payload.clone(), kind, data, context, 0);
    }
}

fn finish_native(
    result: &mut Execution,
    address: Address,
    output: PrecompileResult,
    context: &mut TransferContext<'_>,
) {
    if !context.budget.charge(0) {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    let (kind, bytes) = match output {
        Ok(output) => match output.status {
            PrecompileStatus::Success => (OutcomeKind::Return, output.bytes),
            PrecompileStatus::Revert => (OutcomeKind::Revert, output.bytes),
            PrecompileStatus::Halt(_) => (OutcomeKind::Failure, Default::default()),
        },
        // Fatal backend errors are analysis boundaries, rather than invented
        // contract failures. Ordinary invalid input is returned as a Halt above.
        Err(_) => {
            boundary(result, 0, FrontierReason::Precompile(address));
            return;
        }
    };
    if bytes.len() > context.config.max_memory_bytes {
        boundary(result, 0, FrontierReason::Memory);
        return;
    }
    // Pay sparse byte-map construction and outcome/completion retention before
    // copying the provider's already computed output into an executable array.
    let work = result
        .payload
        .work_size()
        .saturating_mul(4)
        .saturating_add(bytes.len().saturating_mul(16).saturating_mul(3))
        .saturating_add(1024);
    if !context.budget.charge(work) {
        boundary(result, 0, FrontierReason::Work);
        return;
    }
    let data = ByteArray::exact(&bytes);
    if let Err(error) = data.check_existing(context.domain.spec().relations().expressions()) {
        boundary(result, 0, super::errors::bytes(error));
        return;
    }
    let payload = result.payload.clone();
    finish(result, payload, kind, data, context, 0);
}
