//! Internal fixed-slot adapter; callers already validate opcode and arity.

use alloy_primitives::U256;
pub use evm_bytecode::word::{arity, evaluation_work};

#[cfg(any(test, feature = "test-support"))]
pub use evm_bytecode::word::test_support::EVALUATIONS;

/// Evaluate validated fixed slots through the shared, checked Word256 evaluator.
/// Unused slots are omitted using the exact supported arity.
pub(crate) fn evaluate(op: u8, a: U256, b: U256, c: U256) -> U256 {
    let operands = [a, b, c];
    let count = arity(op).expect("numeric transfer validated a pure opcode");
    evm_bytecode::word::evaluate(op, &operands[..count])
        .expect("fixed slots provide the validated pure operation's operands")
}
