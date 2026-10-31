//! Pure EVM Word256 operations in stack-pop order.
//!
//! Unsupported opcodes and incorrect operand counts return typed errors. Numeric
//! evaluation is fork-independent; decoded instructions own fork availability.
use alloy_primitives::U256;
use revm_bytecode::opcode;

/// One fixed-width concrete evaluation, excluding carrier and facade work.
pub fn evaluation_work(op: u8) -> usize {
    match op {
        // 256 squares plus up to 256 products of four-limb words, including
        // exponent/loop bookkeeping in ruint's square-and-multiply evaluator.
        opcode::EXP => 16_384,
        // Include signed normalization and widened modular intermediates.
        opcode::ADDMOD | opcode::MULMOD => 4096,
        opcode::DIV | opcode::SDIV | opcode::MOD | opcode::SMOD => 1024,
        _ => 64,
    }
}

/// Supported pure word operations and their EVM pop-order operand count.
pub fn arity(op: u8) -> Option<usize> {
    match op {
        opcode::ISZERO | opcode::NOT | opcode::CLZ => Some(1),
        opcode::ADDMOD | opcode::MULMOD => Some(3),
        0x01..=0x0b | 0x10..=0x1e => Some(2),
        _ => None,
    }
}

fn signed_lt(a: U256, b: U256) -> bool {
    if a.bit(255) != b.bit(255) {
        a.bit(255)
    } else {
        a < b
    }
}

fn abs(value: U256) -> U256 {
    if value.bit(255) {
        U256::ZERO.wrapping_sub(value)
    } else {
        value
    }
}

/// Invalid input to the pure evaluator; no execution engine is involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WordError {
    /// This byte has no supported pure Word256 operation.
    #[error("unsupported pure Word256 opcode {opcode:#04x}")]
    UnsupportedOpcode {
        /// Rejected opcode byte.
        opcode: u8,
    },
    /// The operation requires exactly the declared number of pop-order inputs.
    #[error("opcode {opcode:#04x} expects {expected} operands, received {actual}")]
    OperandCount {
        /// Supported opcode byte.
        opcode: u8,
        /// Exact supported arity.
        expected: usize,
        /// Number of inputs provided by the caller.
        actual: usize,
    },
}

/// Evaluate a supported operation after validating its exact pop-order arity.
///
/// All arithmetic wraps at 256 bits. Division/modulo by zero, signed overflow,
/// large shifts, BYTE and SIGNEXTEND follow the EVM word rules. The first operand
/// is the first stack item popped, including the base for EXP and shift for SHL.
pub fn evaluate(op: u8, operands: &[U256]) -> Result<U256, WordError> {
    let expected = arity(op).ok_or(WordError::UnsupportedOpcode { opcode: op })?;
    if operands.len() != expected {
        return Err(WordError::OperandCount {
            opcode: op,
            expected,
            actual: operands.len(),
        });
    }
    #[cfg(any(test, feature = "test-support"))]
    test_support::EVALUATIONS.update(|(calls, work)| {
        *calls = calls.saturating_add(1);
        *work = work.saturating_add(evaluation_work(op));
    });
    // Only these unused slots are padded; a public call never invents operands.
    let a = operands[0];
    let b = operands.get(1).copied().unwrap_or(U256::ZERO);
    let c = operands.get(2).copied().unwrap_or(U256::ZERO);
    Ok(evaluate_validated(op, a, b, c))
}

fn evaluate_validated(op: u8, a: U256, b: U256, c: U256) -> U256 {
    let boolean = |v: bool| U256::from(u8::from(v));
    // 256 bit 大整数、模乘和快速幂由 alloy/ruint 提供；这里只表达 EVM 规则。
    match op {
        opcode::ADD => a.wrapping_add(b),
        opcode::MUL => a.wrapping_mul(b),
        opcode::SUB => a.wrapping_sub(b),
        opcode::DIV => {
            if b == U256::ZERO {
                U256::ZERO
            } else {
                a / b
            }
        }
        opcode::MOD => {
            if b == U256::ZERO {
                U256::ZERO
            } else {
                a % b
            }
        }
        opcode::SDIV | opcode::SMOD => {
            if b == U256::ZERO {
                return U256::ZERO;
            }
            let unsigned = if op == opcode::SDIV {
                abs(a) / abs(b)
            } else {
                abs(a) % abs(b)
            };
            let negative = if op == opcode::SDIV {
                a.bit(255) ^ b.bit(255)
            } else {
                a.bit(255)
            };
            if negative {
                U256::ZERO.wrapping_sub(unsigned)
            } else {
                unsigned
            }
        }
        opcode::ADDMOD => a.add_mod(b, c),
        opcode::MULMOD => a.mul_mod(b, c),
        opcode::EXP => a.wrapping_pow(b),
        opcode::SIGNEXTEND => {
            if a >= U256::from(32) {
                return b;
            }
            let bit = a.to::<usize>() * 8 + 7;
            let mask = U256::MAX >> (255 - bit);
            if b.bit(bit) { b | !mask } else { b & mask }
        }
        opcode::LT => boolean(a < b),
        opcode::GT => boolean(a > b),
        opcode::SLT => boolean(signed_lt(a, b)),
        opcode::SGT => boolean(signed_lt(b, a)),
        opcode::EQ => boolean(a == b),
        opcode::ISZERO => boolean(a == U256::ZERO),
        opcode::AND => a & b,
        opcode::OR => a | b,
        opcode::XOR => a ^ b,
        opcode::NOT => !a,
        // EIP-7939：零的前导零数为 256；直接复用 alloy/ruint 的位运算。
        opcode::CLZ => U256::from(a.leading_zeros()),
        opcode::BYTE => {
            if a >= U256::from(32) {
                U256::ZERO
            } else {
                U256::from(b.to_be_bytes::<32>()[a.to::<usize>()])
            }
        }
        opcode::SHL | opcode::SHR => {
            if a >= U256::from(256) {
                return U256::ZERO;
            }
            if op == opcode::SHL {
                b << a.to::<usize>()
            } else {
                b >> a.to::<usize>()
            }
        }
        opcode::SAR => {
            if a >= U256::from(256) {
                return if b.bit(255) { U256::MAX } else { U256::ZERO };
            }
            if b.bit(255) {
                !(!b >> a.to::<usize>())
            } else {
                b >> a.to::<usize>()
            }
        }
        _ => unreachable!("evaluate validated the supported opcode"),
    }
}

/// Scoped observations for tests spanning shared semantic crate boundaries.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    /// Successful pure evaluations and their fixed-width quoted work.
    pub static EVALUATIONS: evm_resource::test_support::Counter<(usize, usize)> =
        evm_resource::test_support::Counter::new((0, 0));
}

#[cfg(test)]
mod tests;
