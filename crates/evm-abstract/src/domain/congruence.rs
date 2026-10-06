//! 一般同余类 `x ≡ r (mod m)`，在有限 word 宇宙中解释。
//!
//! `m = 1` 是 Top；单点独立表示。join 用 gcd，meet 用宽整数 CRT。
//! 运算结果还要取模 `2^256`，因此可能回绕的加减乘会把整数模数与
//! `2^256` 再做 gcd：模 3 等性质不能越过 word 回绕而原样保留。

use alloy_primitives::U256;
use num_bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{One, Zero};
use revm_bytecode::opcode;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
enum Kind {
    Top,
    Exact(U256),
    Modulo { modulus: U256, residue: U256 },
}

/// 规范的非空 256 bit 同余类；模数为零的非法类无法构造。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Congruence(Kind);

impl Congruence {
    /// 所有 word，即模 1 的同余类。
    pub fn top() -> Self {
        Self(Kind::Top)
    }

    /// 一个具体 word。
    pub fn exact(value: U256) -> Self {
        Self(Kind::Exact(value))
    }

    /// 构造一般同余类。余数会规范化；零模数返回 `None`。
    /// 在 word 宇宙中只有一个成员的类进一步规范为单点。
    pub fn new(modulus: U256, residue: U256) -> Option<Self> {
        if modulus == U256::ZERO {
            return None;
        }
        if modulus == U256::from(1) {
            return Some(Self::top());
        }
        let residue = residue % modulus;
        if residue.checked_add(modulus).is_none() {
            Some(Self::exact(residue))
        } else {
            Some(Self(Kind::Modulo { modulus, residue }))
        }
    }

    /// 非空集合的最小同余 hull；空集合由不可达状态表示。
    pub fn from_values(values: &BTreeSet<U256>) -> Self {
        let mut values = values.iter().copied();
        let Some(first) = values.next() else {
            return Self::top();
        };
        values.fold(Self::exact(first), |left, value| {
            left.join(&Self::exact(value))
        })
    }

    /// 是否为模 1 的无限制类。
    pub fn is_top(&self) -> bool {
        matches!(self.0, Kind::Top)
    }

    /// 模数与规范余数；Top 返回 `(1, 0)`，单点返回 `None`。
    pub fn modulus_residue(&self) -> Option<(U256, U256)> {
        match self.0 {
            Kind::Top => Some((U256::from(1), U256::ZERO)),
            Kind::Exact(_) => None,
            Kind::Modulo { modulus, residue } => Some((modulus, residue)),
        }
    }

    /// 检查一个完整 word，不能只比较低位。
    pub fn contains(&self, value: U256) -> bool {
        match self.0 {
            Kind::Top => true,
            Kind::Exact(exact) => value == exact,
            Kind::Modulo { modulus, residue } => value % modulus == residue,
        }
    }

    /// 只有一个 word 时返回它。
    pub fn singleton(&self) -> Option<U256> {
        match self.0 {
            Kind::Exact(value) => Some(value),
            _ => None,
        }
    }

    /// 覆盖并集的 gcd hull；单点的内部模数按零处理。
    pub fn join(&self, other: &Self) -> Self {
        if self == other {
            return self.clone();
        }
        if self.is_top() || other.is_top() {
            return Self::top();
        }
        let (left_modulus, left_residue) = self.integer_class();
        let (right_modulus, right_residue) = other.integer_class();
        let difference = if left_residue >= right_residue {
            left_residue - right_residue
        } else {
            right_residue - left_residue
        };
        let modulus = gcd(gcd(left_modulus, right_modulus), difference);
        if modulus == U256::ZERO {
            Self::exact(left_residue)
        } else {
            Self::new(modulus, left_residue).expect("gcd is nonzero")
        }
    }

    /// 精确 CRT 交集。中间 lcm 可以超过 256 bit，不能用回绕后的模数。
    /// 大于 word 宇宙的 lcm 只可能留下一个成员，也可能交集为空。
    pub fn meet(&self, other: &Self) -> Option<Self> {
        if self == other || other.is_top() {
            return Some(self.clone());
        }
        if self.is_top() {
            return Some(other.clone());
        }
        if let Some(value) = self.singleton() {
            return other.contains(value).then(|| Self::exact(value));
        }
        if let Some(value) = other.singleton() {
            return self.contains(value).then(|| Self::exact(value));
        }
        let (left_modulus, left_residue) = self.modulus_residue()?;
        let (right_modulus, right_residue) = other.modulus_residue()?;
        let m1 = BigInt::from(unsigned(left_modulus));
        let m2 = BigInt::from(unsigned(right_modulus));
        let r1 = BigInt::from(unsigned(left_residue));
        let r2 = BigInt::from(unsigned(right_residue));
        let common = m1.gcd(&m2);
        let difference = r2 - &r1;
        if !difference.is_multiple_of(&common) {
            return None;
        }
        let reduced_left = &m1 / &common;
        let reduced_right = &m2 / &common;
        let inverse = reduced_left.extended_gcd(&reduced_right).x;
        let factor = ((difference / common) * inverse).mod_floor(&reduced_right);
        let modulus = m1 * &reduced_right;
        let residue = (r1 + (&modulus / &reduced_right) * factor).mod_floor(&modulus);
        let (_, residue_bytes) = residue.to_bytes_be();
        if residue_bytes.len() > 32 {
            return None;
        }
        let residue = U256::from_be_slice(&residue_bytes);
        let (_, modulus_bytes) = modulus.to_bytes_be();
        if modulus_bytes.len() > 32 {
            Some(Self::exact(residue))
        } else {
            Self::new(U256::from_be_slice(&modulus_bytes), residue)
        }
    }

    /// 给定无符号范围中第一个和最后一个成员；返回完整的等差类边界。
    /// 返回 `None` 表示范围内确实没有成员，不表示算法资源不足。
    pub fn first_last(&self, lo: U256, hi: U256) -> Option<(U256, U256)> {
        if lo > hi {
            return None;
        }
        if self.is_top() {
            return Some((lo, hi));
        }
        if let Some(value) = self.singleton() {
            return (lo <= value && value <= hi).then_some((value, value));
        }
        let (modulus, residue) = self.modulus_residue()?;
        let lo_remainder = lo % modulus;
        let advance = if lo_remainder <= residue {
            residue - lo_remainder
        } else {
            modulus - (lo_remainder - residue)
        };
        let first = lo.checked_add(advance)?;
        if first > hi {
            return None;
        }
        let last = hi - ((hi - first) % modulus);
        Some((first, last))
    }

    fn integer_class(&self) -> (U256, U256) {
        match self.0 {
            Kind::Top => (U256::from(1), U256::ZERO),
            Kind::Exact(value) => (U256::ZERO, value),
            Kind::Modulo { modulus, residue } => (modulus, residue),
        }
    }

    /// EVM 栈顶在前的运算。一般模数与 word 模数分开处理，避免模回绕误证。
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
        let a = &args[0];
        let b = args.get(1);
        match op {
            opcode::ADD | opcode::SUB | opcode::MUL => {
                let b = b.expect("arity checked");
                if a.singleton() == Some(U256::ZERO) && op == opcode::ADD {
                    return b.clone();
                }
                if b.singleton() == Some(U256::ZERO) && matches!(op, opcode::ADD | opcode::SUB) {
                    return a.clone();
                }
                if op == opcode::MUL {
                    if a.singleton() == Some(U256::ZERO) || b.singleton() == Some(U256::ZERO) {
                        return Self::exact(U256::ZERO);
                    }
                    if a.singleton() == Some(U256::from(1)) {
                        return b.clone();
                    }
                    if b.singleton() == Some(U256::from(1)) {
                        return a.clone();
                    }
                }
                let (am, ar) = a.integer_class();
                let (bm, br) = b.integer_class();
                let am = unsigned(am);
                let bm = unsigned(bm);
                let ar_wide = unsigned(ar);
                let br_wide = unsigned(br);
                let modulus = if op == opcode::MUL {
                    // (ar+am*k)(br+bm*j) 的所有变动由这三个项的 gcd 覆盖。
                    (&am * &bm).gcd(&(&am * &br_wide)).gcd(&(&bm * &ar_wide))
                } else {
                    am.gcd(&bm)
                };
                let residue = match op {
                    opcode::ADD => ar.wrapping_add(br),
                    opcode::SUB => ar.wrapping_sub(br),
                    opcode::MUL => ar.wrapping_mul(br),
                    _ => unreachable!(),
                };
                wrapped(modulus, residue)
            }
            opcode::NOT => {
                let (modulus, residue) = a.integer_class();
                if modulus == U256::ZERO {
                    Self::exact(!residue)
                } else {
                    Self::new(modulus, U256::MAX - residue).expect("nonzero modulus")
                }
            }
            opcode::ADDMOD | opcode::MULMOD => {
                let Some(modulus) = args[2].singleton() else {
                    return Self::top();
                };
                if modulus == U256::ZERO {
                    return Self::exact(U256::ZERO);
                }
                let b = b.expect("arity checked");
                let (am, ar) = a.integer_class();
                let (bm, br) = b.integer_class();
                let am = unsigned(am);
                let bm = unsigned(bm);
                let common = if op == opcode::MULMOD {
                    (&am * &bm)
                        .gcd(&(&am * unsigned(br)))
                        .gcd(&(&bm * unsigned(ar)))
                } else {
                    am.gcd(&bm)
                }
                .gcd(&unsigned(modulus));
                let common = U256::from_be_slice(&common.to_bytes_be());
                let residue = if op == opcode::ADDMOD {
                    ar.add_mod(br, modulus)
                } else {
                    ar.mul_mod(br, modulus)
                };
                // ADDMOD/MULMOD 的中间值不先取模 word；只有显式 modulus 会折返。
                if common == modulus {
                    Self::exact(residue)
                } else {
                    Self::new(common, residue).expect("gcd with nonzero modulus is nonzero")
                }
            }
            opcode::EXP
                if b.and_then(Self::singleton) == Some(U256::ZERO)
                    || a.singleton() == Some(U256::from(1)) =>
            {
                Self::exact(U256::from(1))
            }
            opcode::EXP
                if a.singleton() == Some(U256::ZERO)
                    && b.is_some_and(|exponent| !exponent.contains(U256::ZERO)) =>
            {
                Self::exact(U256::ZERO)
            }
            opcode::SIGNEXTEND if a.singleton().is_some_and(|index| index >= U256::from(32)) => {
                b.expect("arity checked").clone()
            }
            opcode::ISZERO if !a.contains(U256::ZERO) => Self::exact(U256::ZERO),
            opcode::EQ if a.meet(b.expect("arity checked")).is_none() => Self::exact(U256::ZERO),
            opcode::DIV | opcode::MOD | opcode::SDIV | opcode::SMOD
                if b.and_then(Self::singleton) == Some(U256::ZERO) =>
            {
                Self::exact(U256::ZERO)
            }
            opcode::DIV | opcode::SDIV if b.and_then(Self::singleton) == Some(U256::from(1)) => {
                a.clone()
            }
            opcode::MOD | opcode::SMOD if b.and_then(Self::singleton) == Some(U256::from(1)) => {
                Self::exact(U256::ZERO)
            }
            opcode::MOD => {
                let divisor = b.and_then(Self::singleton);
                match (a.modulus_residue(), divisor) {
                    (Some((modulus, residue)), Some(divisor))
                        if modulus % divisor == U256::ZERO =>
                    {
                        Self::exact(residue % divisor)
                    }
                    _ => Self::top(),
                }
            }
            opcode::SHL => {
                let Some(shift) = a.singleton() else {
                    return Self::top();
                };
                if shift >= U256::from(256) {
                    return Self::exact(U256::ZERO);
                }
                if shift == U256::ZERO {
                    return b.expect("arity checked").clone();
                }
                let shift = shift.to::<usize>();
                let value = b.expect("arity checked");
                let (modulus, residue) = value.integer_class();
                wrapped(unsigned(modulus) << shift, residue << shift)
            }
            opcode::SHR => {
                let Some(shift) = a.singleton() else {
                    return Self::top();
                };
                if shift >= U256::from(256) {
                    return Self::exact(U256::ZERO);
                }
                if shift == U256::ZERO {
                    return b.expect("arity checked").clone();
                }
                let shift = shift.to::<usize>();
                let value = b.expect("arity checked");
                let (modulus, residue) = value.integer_class();
                let divisor = U256::from(1) << shift;
                if modulus % divisor == U256::ZERO {
                    Self::new(modulus / divisor, residue / divisor).unwrap_or_else(Self::top)
                } else {
                    Self::top()
                }
            }
            opcode::SAR if a.singleton() == Some(U256::ZERO) => b.expect("arity checked").clone(),
            opcode::BYTE if a.singleton().is_some_and(|index| index >= U256::from(32)) => {
                Self::exact(U256::ZERO)
            }
            opcode::AND
                if a.singleton() == Some(U256::ZERO)
                    || b.and_then(Self::singleton) == Some(U256::ZERO) =>
            {
                Self::exact(U256::ZERO)
            }
            opcode::AND if a.singleton() == Some(U256::MAX) => b.expect("arity checked").clone(),
            opcode::AND if b.and_then(Self::singleton) == Some(U256::MAX) => a.clone(),
            opcode::OR
                if a.singleton() == Some(U256::MAX)
                    || b.and_then(Self::singleton) == Some(U256::MAX) =>
            {
                Self::exact(U256::MAX)
            }
            opcode::OR | opcode::XOR if a.singleton() == Some(U256::ZERO) => {
                b.expect("arity checked").clone()
            }
            opcode::OR | opcode::XOR if b.and_then(Self::singleton) == Some(U256::ZERO) => {
                a.clone()
            }
            _ => Self::top(),
        }
    }
}

fn gcd(mut a: U256, mut b: U256) -> U256 {
    while b != U256::ZERO {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

fn unsigned(value: U256) -> BigUint {
    BigUint::from_bytes_be(&value.to_be_bytes::<32>())
}

fn wrapped(modulus: BigUint, residue: U256) -> Congruence {
    let word_modulus = BigUint::one() << 256;
    let modulus = modulus.gcd(&word_modulus);
    if modulus.is_zero() || modulus == word_modulus {
        Congruence::exact(residue)
    } else {
        let modulus = U256::from_be_slice(&modulus.to_bytes_be());
        Congruence::new(modulus, residue).expect("nonzero bounded gcd")
    }
}

#[cfg(test)]
mod tests;
