//! 有限常量集合域：一个槽位保存“可能值的摘要”，而非单次执行的数。
//!
//! `{3} ⊔ {7} = {3,7}`；超过容量则升到 `⊤`，表示全部 2²⁵⁶ 个值。
//! 这丢失精度，但绝不能丢失可能值。`⊥`（没有执行）由工作表中缺少状态表示。
//! 集合域高度有限，所以循环不需要另造区间 widening 算法；容量截断本身
//! 就保证了每个槽位只会有限次上升。多槽位独立存储会丢失槽位之间的相关性。

use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use std::{collections::BTreeSet, fmt, num::NonZeroUsize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
enum Kind {
    Constants(BTreeSet<U256>),
    Top,
}

/// 非空常量集合或 Top；私有构造避免出现“空但可达”的非法槽位。
///
/// 外部代码只能用 [`Value::constant`] 或 [`Value::top`] 等受控入口构造。
/// ```compile_fail
/// use evm_abstract::domain::Value;
/// let impossible = Value(()); // 元组字段私有，不能绕过非空不变量。
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Value(Kind);

impl Value {
    /// 精确知道一个常量。
    pub fn constant(value: U256) -> Self {
        Self(Kind::Constants(BTreeSet::from([value])))
    }
    /// 任何 256 bit 值都可能。
    pub fn top() -> Self {
        Self(Kind::Top)
    }
    /// 返回有限集合；`None` 表示 Top，不表示空集合。
    pub fn constants(&self) -> Option<&BTreeSet<U256>> {
        match &self.0 {
            Kind::Constants(values) => Some(values),
            Kind::Top => None,
        }
    }
    /// 是否覆盖一个具体值，供具体执行轨迹核对使用。
    pub fn contains(&self, value: U256) -> bool {
        self.constants()
            .is_none_or(|values| values.contains(&value))
    }
    /// 是否包含零，即 JUMPI 是否可能不跳。
    pub fn may_be_zero(&self) -> bool {
        self.contains(U256::ZERO)
    }
    /// 是否可能非零，即 JUMPI 是否可能跳转。
    pub fn may_be_nonzero(&self) -> bool {
        self.constants()
            .is_none_or(|values| values.iter().any(|v| *v != U256::ZERO))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Top => f.write_str("⊤"),
            Kind::Constants(values) => {
                f.write_str("{")?;
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "0x{value:x}")?;
                }
                f.write_str("}")
            }
        }
    }
}

/// 所有 join 和 transfer 共享同一个容量，保证整个分析使用同一抽象域。
#[derive(Clone, Copy, Debug)]
pub struct Domain {
    capacity: NonZeroUsize,
}

impl Default for Domain {
    fn default() -> Self {
        Self {
            capacity: NonZeroUsize::new(8).expect("8 is nonzero"),
        }
    }
}

impl Domain {
    /// 零容量没有学习价值，类型上禁止它。
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self { capacity }
    }
    /// 集合容量。
    pub fn capacity(&self) -> usize {
        self.capacity.get()
    }
    /// 最小的、覆盖两个输入的抽象值；不能用相交来合并路径。
    pub fn join(&self, left: &Value, right: &Value) -> Value {
        match (left.constants(), right.constants()) {
            (Some(a), Some(b)) => self.collect(a.union(b).copied()),
            _ => Value::top(),
        }
    }
    fn collect(&self, values: impl IntoIterator<Item = U256>) -> Value {
        let mut constants = BTreeSet::new();
        for value in values {
            constants.insert(value);
            if constants.len() > self.capacity() {
                return Value::top();
            }
        }
        debug_assert!(!constants.is_empty());
        Value(Kind::Constants(constants))
    }

    /// 纯栈运算。参数顺序是 **先弹出的栈顶在前**，例如 SUB(a,b) = a - b。
    /// 不建模的指令/环境输入产生 Top；未知操作数上的比较仍能限制为 {0,1}。
    /// 指令是否在所选 fork 启用由 Program/transfer 检查，这里只计算数值语义。
    pub fn apply(&self, op: u8, args: &[Value]) -> Value {
        if !matches!(op, 0x01..=0x0b | 0x10..=0x1e) {
            return Value::top();
        }
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        if args.len() != expected {
            return Value::top();
        }
        let Some(sets) = args
            .iter()
            .map(Value::constants)
            .collect::<Option<Vec<_>>>()
        else {
            return if op == opcode::CLZ {
                // 未知 U256 的前导零数只可能是 0..=256。容量不够仍必须升到 Top。
                self.collect((0_u64..=256).map(U256::from))
            } else if matches!(op, 0x10..=0x15) {
                self.collect([U256::ZERO, U256::from(1)])
            } else {
                Value::top()
            };
        };
        // 至多三输入。中间结果一旦超过容量就立刻停止，避免构造全部积。
        let mut results = BTreeSet::new();
        let singleton = BTreeSet::from([U256::ZERO]);
        for a in sets[0] {
            for b in sets.get(1).copied().unwrap_or(&singleton) {
                for c in sets.get(2).copied().unwrap_or(&singleton) {
                    results.insert(evaluate(op, *a, *b, *c));
                    if results.len() > self.capacity() {
                        return Value::top();
                    }
                }
            }
        }
        Value(Kind::Constants(results))
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

fn evaluate(op: u8, a: U256, b: U256, c: U256) -> U256 {
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
