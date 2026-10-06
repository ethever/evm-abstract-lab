//! 每一位独立记录必为零、必为一或未知；两个掩码不重叠。
//!
//! 这里只处理 EVM 的 256 bit 模运算，不引入 LLVM 的 poison/未定义行为。
//! 路径汇合保留共同位，约束相交合并已知位；其他域通过 facts 缩小未知位。

use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use std::collections::BTreeSet;

/// 一组 256 bit 整数的已知位；`zero & one == 0` 是构造不变量。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct KnownBits {
    zero: U256,
    one: U256,
}

impl KnownBits {
    /// 所有位未知，覆盖整个 EVM word。
    pub fn top() -> Self {
        Self {
            zero: U256::ZERO,
            one: U256::ZERO,
        }
    }

    /// 精确整数的所有位都已知。
    pub fn exact(value: U256) -> Self {
        Self {
            zero: !value,
            one: value,
        }
    }

    /// 保留有限集合中每个整数都具有的位；空输入保守返回 Top。
    pub fn from_values(values: &BTreeSet<U256>) -> Self {
        let mut values = values.iter();
        let Some(first) = values.next() else {
            return Self::top();
        };
        values.fold(Self::exact(*first), |bits, value| {
            bits.join(&Self::exact(*value))
        })
    }

    /// 同一位同时要求零和一说明约束矛盾，不能构造可达值。
    pub fn new(zero: U256, one: U256) -> Option<Self> {
        (zero & one == U256::ZERO).then_some(Self { zero, one })
    }

    /// 必为零的位。
    pub fn zero(&self) -> U256 {
        self.zero
    }

    /// 必为一的位。
    pub fn one(&self) -> U256 {
        self.one
    }

    /// 候选整数是否满足全部位约束。
    pub fn contains(&self, value: U256) -> bool {
        value & self.zero == U256::ZERO && value & self.one == self.one
    }

    /// 全部位都确定时返回唯一整数。
    pub fn singleton(&self) -> Option<U256> {
        (self.zero | self.one == U256::MAX).then_some(self.one)
    }

    /// 汇合只保留两条路径都能证明的位，因此满足交换、结合和幂等。
    pub fn join(&self, other: &Self) -> Self {
        Self {
            zero: self.zero & other.zero,
            one: self.one & other.one,
        }
    }

    /// 同时要求两个约束；不兼容时返回空交集。
    pub fn meet(&self, other: &Self) -> Option<Self> {
        Self::new(self.zero | other.zero, self.one | other.one)
    }

    /// 位约束的无符号最小值和最大值，两个端点本身都满足约束。
    pub fn unsigned_bounds(&self) -> (U256, U256) {
        (self.one, !self.zero)
    }

    /// 无符号闭区间只有共同的高位前缀必然固定。
    /// 无效端点保守返回 Top；空集由上层的可达性类型表示。
    pub fn from_unsigned_bounds(lo: U256, hi: U256) -> Self {
        if lo > hi {
            return Self::top();
        }
        let varying = 256 - (lo ^ hi).leading_zeros();
        let fixed = !low_mask(varying);
        Self {
            zero: !lo & fixed,
            one: lo & fixed,
        }
    }

    /// 纯数值 transfer，参数顺序为先弹出的栈顶在前。
    /// 未支持的操作返回 Top；所有移位、零除和溢出遵循 EVM 规则。
    ///
    /// 成本上界与输入数值无关：加减扫描 256 位，每位至多 8 种进位组合；
    /// 未知移位最多检查 256 个普通移位量和一个统一的超长移位分支。
    pub fn transfer(op: u8, args: &[Self]) -> Self {
        if !matches!(op, 0x01..=0x0b | 0x10..=0x1e) {
            return Self::top();
        }
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if args.len() != expected {
            return Self::top();
        }
        // 完全确定的输入复用统一的具体语义，避免多份 EVM 边界规则漂移。
        if let Some(values) = args.iter().map(Self::singleton).collect::<Option<Vec<_>>>() {
            return Self::exact(super::evaluate(
                op,
                values[0],
                values.get(1).copied().unwrap_or(U256::ZERO),
                values.get(2).copied().unwrap_or(U256::ZERO),
            ));
        }
        let a = args[0];
        let b = args.get(1).copied().unwrap_or_else(Self::top);
        match op {
            opcode::ADD => add_bits(a, b, false),
            opcode::SUB => add_bits(a, b.invert(), true),
            opcode::MUL => multiply(a, b),
            opcode::DIV => divide(a, b),
            opcode::MOD => modulo(a, b),
            opcode::SDIV => {
                if a.singleton() == Some(U256::ZERO) || b.singleton() == Some(U256::ZERO) {
                    Self::exact(U256::ZERO)
                } else if b.singleton() == Some(U256::from(1)) {
                    a
                } else if b.singleton() == Some(U256::MAX) {
                    add_bits(Self::exact(U256::ZERO), a.invert(), true)
                } else {
                    Self::top()
                }
            }
            opcode::SMOD => {
                if a.singleton() == Some(U256::ZERO)
                    || matches!(b.singleton(), Some(value) if value == U256::ZERO || value == U256::from(1) || value == U256::MAX)
                {
                    Self::exact(U256::ZERO)
                } else {
                    Self::top()
                }
            }
            opcode::ADDMOD | opcode::MULMOD => modular(op, a, b, args[2]),
            opcode::EXP => exponent(a, b),
            opcode::SIGNEXTEND => indexed(op, a, b, 32),
            opcode::LT | opcode::GT => compare(op, a.unsigned_bounds(), b.unsigned_bounds()),
            opcode::SLT | opcode::SGT => compare(op, a.signed_bounds(), b.signed_bounds()),
            opcode::EQ => {
                if (a.zero & b.one) | (a.one & b.zero) != U256::ZERO {
                    Self::exact(U256::ZERO)
                } else {
                    boolean()
                }
            }
            opcode::ISZERO => {
                if a.one != U256::ZERO {
                    Self::exact(U256::ZERO)
                } else {
                    boolean()
                }
            }
            opcode::AND => Self {
                zero: a.zero | b.zero,
                one: a.one & b.one,
            },
            opcode::OR => Self {
                zero: a.zero & b.zero,
                one: a.one | b.one,
            },
            opcode::XOR => Self {
                zero: (a.zero & b.zero) | (a.one & b.one),
                one: (a.zero & b.one) | (a.one & b.zero),
            },
            opcode::NOT => a.invert(),
            opcode::CLZ => {
                let (lo, hi) = a.unsigned_bounds();
                Self::from_unsigned_bounds(
                    U256::from(hi.leading_zeros()),
                    U256::from(lo.leading_zeros()),
                )
            }
            opcode::BYTE => indexed(op, a, b, 32),
            opcode::SHL | opcode::SHR | opcode::SAR => indexed(op, a, b, 256),
            _ => Self::top(),
        }
    }

    fn invert(self) -> Self {
        Self {
            zero: self.one,
            one: self.zero,
        }
    }

    // 翻转符号位后，无符号顺序就是二补码的有符号顺序。
    fn signed_bounds(self) -> (U256, U256) {
        let sign: U256 = U256::from(1) << 255;
        Self {
            zero: (self.zero & !sign) | (self.one & sign),
            one: (self.one & !sign) | (self.zero & sign),
        }
        .unsigned_bounds()
    }
}

fn low_mask(bits: usize) -> U256 {
    match bits {
        0 => U256::ZERO,
        256.. => U256::MAX,
        _ => U256::MAX >> (256 - bits),
    }
}

fn boolean() -> KnownBits {
    KnownBits {
        zero: !U256::from(1),
        one: U256::ZERO,
    }
}

fn bit_values(bits: KnownBits, index: usize) -> u8 {
    if bits.zero.bit(index) {
        1
    } else if bits.one.bit(index) {
        2
    } else {
        3
    }
}

fn add_bits(a: KnownBits, b: KnownBits, initial_carry: bool) -> KnownBits {
    let mut result = KnownBits::top();
    let mut carries = if initial_carry { 2_u8 } else { 1_u8 };
    for index in 0..256 {
        let a_values = bit_values(a, index);
        let b_values = bit_values(b, index);
        let mut sums = 0_u8;
        let mut next_carries = 0_u8;
        // 位之间的相关性会遗失，但枚举当前进位集合覆盖全部具体加法。
        for a_bit in 0..=1 {
            for b_bit in 0..=1 {
                for carry in 0..=1 {
                    if a_values & (1 << a_bit) == 0
                        || b_values & (1 << b_bit) == 0
                        || carries & (1 << carry) == 0
                    {
                        continue;
                    }
                    let sum = a_bit + b_bit + carry;
                    sums |= 1 << (sum & 1);
                    next_carries |= 1 << (sum >> 1);
                }
            }
        }
        let mask = U256::from(1) << index;
        if sums == 1 {
            result.zero |= mask;
        } else if sums == 2 {
            result.one |= mask;
        }
        carries = next_carries;
    }
    // 最高位的进位按 EVM 的模 2^256 语义舍弃。
    result
}

fn multiply(a: KnownBits, b: KnownBits) -> KnownBits {
    for (constant, other) in [(a.singleton(), b), (b.singleton(), a)] {
        if let Some(constant) = constant {
            if constant == U256::ZERO {
                return KnownBits::exact(U256::ZERO);
            }
            if constant == U256::from(1) {
                return other;
            }
            if constant.is_power_of_two() {
                return shifted(opcode::SHL, constant.trailing_zeros(), other);
            }
        }
    }
    // 无论乘积是否环绕，低位的整除性仍然成立。
    let trailing = (a.zero.trailing_ones() + b.zero.trailing_ones()).min(256);
    let trailing_bits = KnownBits {
        zero: low_mask(trailing),
        one: U256::ZERO,
    };
    let (_, a_max) = a.unsigned_bounds();
    let (_, b_max) = b.unsigned_bounds();
    let (maximum, overflow) = a_max.overflowing_mul(b_max);
    if overflow {
        trailing_bits
    } else {
        trailing_bits
            .meet(&KnownBits::from_unsigned_bounds(U256::ZERO, maximum))
            .expect("multiplication bounds and divisibility agree")
    }
}

fn divide(a: KnownBits, b: KnownBits) -> KnownBits {
    if a.singleton() == Some(U256::ZERO) || b.singleton() == Some(U256::ZERO) {
        return KnownBits::exact(U256::ZERO);
    }
    if let Some(divisor) = b.singleton()
        && divisor.is_power_of_two()
    {
        return shifted(opcode::SHR, divisor.trailing_zeros(), a);
    }
    let (a_min, a_max) = a.unsigned_bounds();
    let (b_min, b_max) = b.unsigned_bounds();
    if b_min == U256::ZERO {
        KnownBits::from_unsigned_bounds(U256::ZERO, a_max)
    } else {
        KnownBits::from_unsigned_bounds(a_min / b_max, a_max / b_min)
    }
}

fn modulo(a: KnownBits, b: KnownBits) -> KnownBits {
    if a.singleton() == Some(U256::ZERO) || b.singleton() == Some(U256::ZERO) {
        return KnownBits::exact(U256::ZERO);
    }
    if let Some(divisor) = b.singleton()
        && divisor.is_power_of_two()
    {
        let mask = divisor - U256::from(1);
        return KnownBits {
            zero: a.zero | !mask,
            one: a.one & mask,
        };
    }
    let (_, a_max) = a.unsigned_bounds();
    let (_, b_max) = b.unsigned_bounds();
    KnownBits::from_unsigned_bounds(U256::ZERO, a_max.min(b_max - U256::from(1)))
}

fn modular(op: u8, a: KnownBits, b: KnownBits, modulus: KnownBits) -> KnownBits {
    let a_zero = a.singleton() == Some(U256::ZERO);
    let b_zero = b.singleton() == Some(U256::ZERO);
    if (a_zero && b_zero) || (op == opcode::MULMOD && (a_zero || b_zero)) {
        return KnownBits::exact(U256::ZERO);
    }
    if let Some(modulus) = modulus.singleton() {
        if modulus == U256::ZERO {
            return KnownBits::exact(U256::ZERO);
        }
        if modulus.is_power_of_two() {
            let raw = if op == opcode::ADDMOD {
                add_bits(a, b, false)
            } else {
                multiply(a, b)
            };
            let mask = modulus - U256::from(1);
            return KnownBits {
                zero: raw.zero | !mask,
                one: raw.one & mask,
            };
        }
    }
    let (_, maximum) = modulus.unsigned_bounds();
    KnownBits::from_unsigned_bounds(U256::ZERO, maximum - U256::from(1))
}

fn exponent(base: KnownBits, exponent: KnownBits) -> KnownBits {
    if exponent.singleton() == Some(U256::ZERO) || base.singleton() == Some(U256::from(1)) {
        return KnownBits::exact(U256::from(1));
    }
    if exponent.singleton() == Some(U256::from(1)) {
        return base;
    }
    let (minimum_exponent, _) = exponent.unsigned_bounds();
    if base.singleton() == Some(U256::ZERO) {
        return if minimum_exponent == U256::ZERO {
            boolean()
        } else {
            KnownBits::exact(U256::ZERO)
        };
    }
    let trailing = base.zero.trailing_ones();
    if trailing == 0 || minimum_exponent == U256::ZERO {
        return KnownBits::top();
    }
    let zero_threshold = 256_usize.div_ceil(trailing);
    if minimum_exponent >= U256::from(zero_threshold) {
        KnownBits::exact(U256::ZERO)
    } else {
        KnownBits {
            zero: low_mask(trailing * minimum_exponent.to::<usize>()),
            one: U256::ZERO,
        }
    }
}

fn compare(op: u8, a: (U256, U256), b: (U256, U256)) -> KnownBits {
    let (a_min, a_max) = a;
    let (b_min, b_max) = b;
    let less = matches!(op, opcode::LT | opcode::SLT);
    if (less && a_max < b_min) || (!less && a_min > b_max) {
        KnownBits::exact(U256::from(1))
    } else if (less && a_min >= b_max) || (!less && a_max <= b_min) {
        KnownBits::exact(U256::ZERO)
    } else {
        boolean()
    }
}

fn indexed(op: u8, index: KnownBits, value: KnownBits, limit: usize) -> KnownBits {
    if let Some(index) = index.singleton() {
        let index = if index >= U256::from(limit) {
            limit
        } else {
            index.to::<usize>()
        };
        return indexed_exact(op, index, value);
    }
    let mut result = None;
    for candidate in 0..limit {
        if index.contains(U256::from(candidate)) {
            let candidate_result = indexed_exact(op, candidate, value);
            result = Some(match result {
                None => candidate_result,
                Some(previous) => KnownBits::join(&previous, &candidate_result),
            });
        }
    }
    // 所有超长移位/越界字节下标具有同一种行为，不枚举其 256 bit 数值。
    if index.unsigned_bounds().1 >= U256::from(limit) {
        let overflow_result = indexed_exact(op, limit, value);
        result = Some(match result {
            None => overflow_result,
            Some(previous) => previous.join(&overflow_result),
        });
    }
    result.expect("nonempty known bits cover an index")
}

fn indexed_exact(op: u8, index: usize, value: KnownBits) -> KnownBits {
    match op {
        opcode::BYTE => {
            if index >= 32 {
                KnownBits::exact(U256::ZERO)
            } else {
                let shifted = shifted(opcode::SHR, (31 - index) * 8, value);
                KnownBits {
                    zero: shifted.zero | !low_mask(8),
                    one: shifted.one & low_mask(8),
                }
            }
        }
        opcode::SIGNEXTEND => {
            if index >= 32 {
                return value;
            }
            let sign_index = index * 8 + 7;
            let extended = !low_mask(sign_index + 1);
            KnownBits {
                zero: (value.zero & !extended)
                    | if value.zero.bit(sign_index) {
                        extended
                    } else {
                        U256::ZERO
                    },
                one: (value.one & !extended)
                    | if value.one.bit(sign_index) {
                        extended
                    } else {
                        U256::ZERO
                    },
            }
        }
        _ => shifted(op, index, value),
    }
}

fn shifted(op: u8, shift: usize, value: KnownBits) -> KnownBits {
    if shift >= 256 {
        return if op != opcode::SAR || value.zero.bit(255) {
            KnownBits::exact(U256::ZERO)
        } else if value.one.bit(255) {
            KnownBits::exact(U256::MAX)
        } else {
            KnownBits::top()
        };
    }
    if op == opcode::SHL {
        return KnownBits {
            zero: (value.zero << shift) | low_mask(shift),
            one: value.one << shift,
        };
    }
    let extended = !low_mask(256 - shift);
    KnownBits {
        zero: (value.zero >> shift)
            | if op == opcode::SHR || value.zero.bit(255) {
                extended
            } else {
                U256::ZERO
            },
        one: (value.one >> shift)
            | if op == opcode::SAR && value.one.bit(255) {
                extended
            } else {
                U256::ZERO
            },
    }
}

#[cfg(test)]
mod tests;
