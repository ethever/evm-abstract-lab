//! 无符号区间与有符号区间的交集；所有边界都属于同一个 EVM word。
//!
//! 有符号顺序用 `x ^ 2^255` 映射到无符号顺序，避免把 MIN/-1 或模回绕
//! 偷换为整数算术。两个线性区间的交集至多有两段，不使用不满足结合律的
//! “最短环形区间” join。每次构造都收紧到交集的两个 hull，保持表示唯一。

use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use std::collections::BTreeSet;

/// 同时限制无符号顺序和有符号顺序的非空 256 bit 区间。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Interval {
    unsigned_lo: U256,
    unsigned_hi: U256,
    signed_lo: U256,
    signed_hi: U256,
}

fn sign_bit() -> U256 {
    U256::from(1) << 255
}

impl Interval {
    /// 所有 256 bit word。
    pub fn top() -> Self {
        Self {
            unsigned_lo: U256::ZERO,
            unsigned_hi: U256::MAX,
            signed_lo: U256::ZERO,
            signed_hi: U256::MAX,
        }
    }

    /// 一个具体 word；有符号解释不改变它的位模式。
    pub fn exact(value: U256) -> Self {
        let biased = value ^ sign_bit();
        Self {
            unsigned_lo: value,
            unsigned_hi: value,
            signed_lo: biased,
            signed_hi: biased,
        }
    }

    /// 非空集合的两个顺序 hull；空集合由不可达状态表示。
    pub fn from_values(values: &BTreeSet<U256>) -> Self {
        let mut values = values.iter().copied();
        let Some(first) = values.next() else {
            return Self::top();
        };
        values.fold(Self::exact(first), |left, value| {
            left.join(&Self::exact(value))
        })
    }

    /// 由无符号闭区间构造；颠倒边界表示空集。
    pub fn new_unsigned(lo: U256, hi: U256) -> Option<Self> {
        Self {
            unsigned_lo: lo,
            unsigned_hi: hi,
            ..Self::top()
        }
        .normalize()
    }

    /// 由有符号顺序的闭区间构造；参数已用 XOR `2^255` 偏置。
    pub fn new_signed(biased_lo: U256, biased_hi: U256) -> Option<Self> {
        Self {
            signed_lo: biased_lo,
            signed_hi: biased_hi,
            ..Self::top()
        }
        .normalize()
    }

    /// 无符号闭区间的端点。
    pub fn unsigned_bounds(&self) -> (U256, U256) {
        (self.unsigned_lo, self.unsigned_hi)
    }

    /// 有符号闭区间的端点，使用 XOR `2^255` 后的无符号坐标。
    pub fn signed_bounds(&self) -> (U256, U256) {
        (self.signed_lo, self.signed_hi)
    }

    /// 是否没有数值限制。
    pub fn is_top(&self) -> bool {
        *self == Self::top()
    }

    /// 在两个顺序中都满足限制。
    pub fn contains(&self, value: U256) -> bool {
        let biased = value ^ sign_bit();
        self.unsigned_lo <= value
            && value <= self.unsigned_hi
            && self.signed_lo <= biased
            && biased <= self.signed_hi
    }

    /// 只有一个 word 时返回它。
    pub fn singleton(&self) -> Option<U256> {
        (self.unsigned_lo == self.unsigned_hi).then_some(self.unsigned_lo)
    }

    /// 真正的分量 hull join；覆盖并集且满足交换、结合、幂等律。
    pub fn join(&self, other: &Self) -> Self {
        Self {
            unsigned_lo: self.unsigned_lo.min(other.unsigned_lo),
            unsigned_hi: self.unsigned_hi.max(other.unsigned_hi),
            signed_lo: self.signed_lo.min(other.signed_lo),
            signed_hi: self.signed_hi.max(other.signed_hi),
        }
    }

    /// 循环状态的保守外推：向外移动的界直接扩大到该顺序的极值。
    /// 这不是 join；调用方必须用冻结的循环策略决定何时启用它。
    /// 两种顺序分别扩大后再规范交集，结果始终覆盖旧值与下一值。
    pub fn widen(&self, next: &Self) -> Self {
        Self {
            unsigned_lo: if next.unsigned_lo < self.unsigned_lo {
                U256::ZERO
            } else {
                self.unsigned_lo
            },
            unsigned_hi: if next.unsigned_hi > self.unsigned_hi {
                U256::MAX
            } else {
                self.unsigned_hi
            },
            signed_lo: if next.signed_lo < self.signed_lo {
                U256::ZERO
            } else {
                self.signed_lo
            },
            signed_hi: if next.signed_hi > self.signed_hi {
                U256::MAX
            } else {
                self.signed_hi
            },
        }
        .normalize()
        .expect("widened coordinate bounds cover both nonempty operands")
    }

    /// 同时满足两个区间；只在交集确实为空时返回 `None`。
    pub fn meet(&self, other: &Self) -> Option<Self> {
        Self {
            unsigned_lo: self.unsigned_lo.max(other.unsigned_lo),
            unsigned_hi: self.unsigned_hi.min(other.unsigned_hi),
            signed_lo: self.signed_lo.max(other.signed_lo),
            signed_hi: self.signed_hi.min(other.signed_hi),
        }
        .normalize()
    }

    /// 交集的全部无符号连续段，升序且至多两段；不是代表值采样。
    pub fn segments(&self) -> Vec<(U256, U256)> {
        if self.unsigned_lo > self.unsigned_hi || self.signed_lo > self.signed_hi {
            return Vec::new();
        }
        let sign = sign_bit();
        let signed_segments = if self.signed_hi < sign || self.signed_lo >= sign {
            vec![(self.signed_lo ^ sign, self.signed_hi ^ sign)]
        } else {
            vec![
                (U256::ZERO, self.signed_hi ^ sign),
                (self.signed_lo ^ sign, U256::MAX),
            ]
        };
        signed_segments
            .into_iter()
            .filter_map(|(lo, hi)| {
                let lo = lo.max(self.unsigned_lo);
                let hi = hi.min(self.unsigned_hi);
                (lo <= hi).then_some((lo, hi))
            })
            .collect()
    }

    fn normalize(self) -> Option<Self> {
        let segments = self.segments();
        let (lo, _) = *segments.first()?;
        let (_, hi) = *segments.last()?;
        let sign = sign_bit();
        let mut signed_lo = U256::MAX;
        let mut signed_hi = U256::ZERO;
        for (start, end) in segments {
            // 每段均在同一个符号半区，XOR 因而保持段内顺序。
            signed_lo = signed_lo.min(start ^ sign);
            signed_hi = signed_hi.max(end ^ sign);
        }
        Some(Self {
            unsigned_lo: lo,
            unsigned_hi: hi,
            signed_lo,
            signed_hi,
        })
    }

    /// 单条 EVM 数值指令；参数顺序为先弹出的栈顶在前。
    /// 无法证明不回绕时扩大范围，绝不把整数界限套到模 `2^256` 结果上。
    pub fn transfer(op: u8, args: &[Self]) -> Self {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if args.len() != expected || !matches!(op, 0x01..=0x0b | 0x10..=0x1e) {
            return Self::top();
        }
        let exact = args.iter().map(Self::singleton).collect::<Option<Vec<_>>>();
        if let Some(exact) = exact {
            return Self::exact(super::evaluate(
                op,
                exact[0],
                exact.get(1).copied().unwrap_or(U256::ZERO),
                exact.get(2).copied().unwrap_or(U256::ZERO),
            ));
        }
        let a = args[0];
        let b = args.get(1).copied().unwrap_or_else(Self::top);
        let range = |lo, hi| Self::new_unsigned(lo, hi).unwrap_or_else(Self::top);
        let zero = || Self::exact(U256::ZERO);
        let boolean = || range(U256::ZERO, U256::from(1));
        match op {
            opcode::ADD if a.singleton() == Some(U256::ZERO) => b,
            opcode::ADD | opcode::SUB if b.singleton() == Some(U256::ZERO) => a,
            opcode::ADD => match (
                a.unsigned_lo.checked_add(b.unsigned_lo),
                a.unsigned_hi.checked_add(b.unsigned_hi),
            ) {
                (Some(lo), Some(hi)) => range(lo, hi),
                _ => Self::top(),
            },
            opcode::SUB if a.unsigned_lo >= b.unsigned_hi => {
                range(a.unsigned_lo - b.unsigned_hi, a.unsigned_hi - b.unsigned_lo)
            }
            opcode::MUL
                if a.singleton() == Some(U256::ZERO) || b.singleton() == Some(U256::ZERO) =>
            {
                zero()
            }
            opcode::MUL if a.singleton() == Some(U256::from(1)) => b,
            opcode::MUL | opcode::DIV if b.singleton() == Some(U256::from(1)) => a,
            opcode::MUL => match (
                a.unsigned_lo.checked_mul(b.unsigned_lo),
                a.unsigned_hi.checked_mul(b.unsigned_hi),
            ) {
                (Some(lo), Some(hi)) => range(lo, hi),
                _ => Self::top(),
            },
            opcode::DIV | opcode::MOD | opcode::SDIV | opcode::SMOD
                if b.singleton() == Some(U256::ZERO) =>
            {
                zero()
            }
            opcode::DIV => {
                let lo = if b.unsigned_lo == U256::ZERO {
                    U256::ZERO
                } else {
                    a.unsigned_lo / b.unsigned_hi
                };
                range(lo, a.unsigned_hi / b.unsigned_lo.max(U256::from(1)))
            }
            opcode::MOD => range(U256::ZERO, a.unsigned_hi.min(b.unsigned_hi - U256::from(1))),
            opcode::SDIV if b.singleton() == Some(U256::from(1)) => a,
            opcode::ADDMOD | opcode::MULMOD => {
                let modulus = args[2];
                if modulus.unsigned_hi == U256::ZERO {
                    zero()
                } else {
                    range(U256::ZERO, modulus.unsigned_hi - U256::from(1))
                }
            }
            opcode::EXP
                if b.singleton() == Some(U256::ZERO) || a.singleton() == Some(U256::from(1)) =>
            {
                Self::exact(U256::from(1))
            }
            opcode::EXP if a.singleton() == Some(U256::ZERO) && !b.contains(U256::ZERO) => zero(),
            opcode::SIGNEXTEND if a.unsigned_lo >= U256::from(32) => b,
            opcode::LT if a.unsigned_hi < b.unsigned_lo => Self::exact(U256::from(1)),
            opcode::LT if a.unsigned_lo >= b.unsigned_hi => zero(),
            opcode::GT if a.unsigned_lo > b.unsigned_hi => Self::exact(U256::from(1)),
            opcode::GT if a.unsigned_hi <= b.unsigned_lo => zero(),
            opcode::SLT if a.signed_hi < b.signed_lo => Self::exact(U256::from(1)),
            opcode::SLT if a.signed_lo >= b.signed_hi => zero(),
            opcode::SGT if a.signed_lo > b.signed_hi => Self::exact(U256::from(1)),
            opcode::SGT if a.signed_hi <= b.signed_lo => zero(),
            opcode::EQ if a.meet(&b).is_none() => zero(),
            opcode::ISZERO if !a.contains(U256::ZERO) => zero(),
            opcode::LT | opcode::GT | opcode::SLT | opcode::SGT | opcode::EQ | opcode::ISZERO => {
                boolean()
            }
            opcode::NOT => Self {
                unsigned_lo: !a.unsigned_hi,
                unsigned_hi: !a.unsigned_lo,
                signed_lo: !a.signed_hi,
                signed_hi: !a.signed_lo,
            },
            opcode::AND => range(U256::ZERO, a.unsigned_hi.min(b.unsigned_hi)),
            opcode::OR => range(a.unsigned_lo.max(b.unsigned_lo), U256::MAX),
            opcode::BYTE if a.unsigned_lo >= U256::from(32) => zero(),
            opcode::BYTE => range(U256::ZERO, U256::from(255)),
            opcode::SHL if a.unsigned_lo >= U256::from(256) => zero(),
            opcode::SHL => {
                let Some(shift) = a.singleton() else {
                    return Self::top();
                };
                let shift = shift.to::<usize>();
                if b.unsigned_hi <= U256::MAX >> shift {
                    range(b.unsigned_lo << shift, b.unsigned_hi << shift)
                } else {
                    Self::top()
                }
            }
            opcode::SHR => range(
                shr(b.unsigned_lo, a.unsigned_hi),
                shr(b.unsigned_hi, a.unsigned_lo),
            ),
            opcode::SAR => {
                let sign = sign_bit();
                let mut lo = U256::MAX;
                let mut hi = U256::ZERO;
                for shift in [a.unsigned_lo, a.unsigned_hi] {
                    for value in [b.signed_lo ^ sign, b.signed_hi ^ sign] {
                        let result = super::evaluate(op, shift, value, U256::ZERO) ^ sign;
                        lo = lo.min(result);
                        hi = hi.max(result);
                    }
                }
                Self::new_signed(lo, hi).unwrap_or_else(Self::top)
            }
            opcode::CLZ => range(
                U256::from(a.unsigned_hi.leading_zeros()),
                U256::from(a.unsigned_lo.leading_zeros()),
            ),
            _ => Self::top(),
        }
    }
}

fn shr(value: U256, shift: U256) -> U256 {
    if shift >= U256::from(256) {
        U256::ZERO
    } else {
        value >> shift.to::<usize>()
    }
}

#[cfg(test)]
mod tests;
