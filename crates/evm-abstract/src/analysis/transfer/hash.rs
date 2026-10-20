//! Content hashes retain a conservative value when equality support is limited.

use crate::domain::EvmNumericAlgebra;

use super::{EvmExecution, TransferContext, boundary, errors};

use crate::{domain::EvmValue, world::EvmByteArray};

pub(super) fn evaluate<D: EvmNumericAlgebra>(
    bytes: &EvmByteArray<D::Element>,
    result: &mut EvmExecution<D::Element>,
    context: &TransferContext<'_, D>,
    pc: usize,
) -> Option<EvmValue<D::Element>> {
    let value = match bytes.keccak_with(context.domain, context.config.max_memory_bytes) {
        Ok(value) => value,
        Err(error) => {
            boundary(result, pc, errors::bytes(error));
            return None;
        }
    };
    Some(value)
}
