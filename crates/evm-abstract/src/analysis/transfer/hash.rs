//! Content hashes retain a conservative value when equality support is limited.

use super::{Execution, TransferContext, boundary, errors};
use crate::{domain::AbstractValue, world::ByteArray};

pub(super) fn evaluate(
    bytes: &ByteArray,
    result: &mut Execution,
    context: &TransferContext<'_>,
    pc: usize,
) -> Option<AbstractValue> {
    let value = match bytes.keccak(context.domain, context.config.max_memory_bytes) {
        Ok(value) => value,
        Err(error) => {
            boundary(result, pc, errors::bytes(error));
            return None;
        }
    };
    Some(value)
}
