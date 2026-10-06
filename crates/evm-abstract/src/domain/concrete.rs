//! 所有有限枚举和组件单点计算共用的 EVM 具体语义。
use alloy_primitives::U256;
use revm_bytecode::opcode;

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

pub(super) fn evaluate(op: u8, a: U256, b: U256, c: U256) -> U256 {
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
        _ => unreachable!("apply checks the supported opcode range"),
    }
}
