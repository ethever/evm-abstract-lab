//! Content hashes retain a conservative value when equality support is limited.

use super::{Execution, TransferContext, boundary, errors};
use crate::{
    analysis::DiagnosticKind,
    domain::{AbstractValue, relational::QueryReason, symbolic::HashSupport},
    world::ByteArray,
};
use revm_bytecode::opcode;

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
    if value.singleton().is_none() {
        match bytes
            .view()
            .hash_support(context.config.analysis.relations.expressions())
        {
            Ok(HashSupport::Uncompared) => result.diagnostics.push((
                pc,
                DiagnosticKind::RelationalPrecisionLost(QueryReason::Unsupported(
                    opcode::KECCAK256,
                )),
            )),
            Ok(_) => {}
            Err(error) => {
                boundary(result, pc, errors::expression(error));
                return None;
            }
        }
    }
    Some(value)
}
