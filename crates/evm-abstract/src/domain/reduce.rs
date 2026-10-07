//! 临时规约只传播当前 product 蕴含的事实。每轮事务式导出/导入，
//! FactLattice 对同一语义槽取 meet；重复回馈不会生成新的日志条目。
use super::{
    AbstractValue, Domain,
    congruence::Congruence,
    facts::{
        BitConstraints, Fact, FactChange, FactError, FactLattice, FiniteSet, Symbol, UnaryFact,
        UnaryPredicate, WordBounds,
    },
    interval::Interval,
    known_bits::KnownBits,
    provenance::Provenance,
    query,
};
use alloy_primitives::U256;
use serde::Serialize;

/// 有界交换的结束原因；精度上限不等同于 EVM 失败或全局资源中断。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ReductionStatus {
    /// 支持的规则已经没有进一步变化。
    Stable,
    /// 已证明的信息保留，尚未声称闭包。
    RoundLimit,
    /// 事实原子容量不足，保留此前完整轮的安全结果。
    FactLimit,
    /// 数值约束已证明相互矛盾；不表示资源中断。
    Empty,
    /// 来源声明不相容，数值不可达性仍未知。
    OriginConflict,
    /// Numeric facts remain sound, but an operation expression exceeded its policy.
    SymbolicLimit,
}
/// 数值结果及可核对的交换计数。
#[derive(Clone, Debug)]
pub struct Reduction {
    /// 覆盖全部具体执行结果的约束。
    pub value: AbstractValue,
    /// 终止边界。
    pub status: ReductionStatus,
    /// 实际执行的完整轮数。
    pub rounds: usize,
    /// 已接纳的严格增强事实次数。
    pub strengthened: usize,
    /// Additional typed expression-retention failure, independent of numeric exchange status.
    pub symbolic_error: Option<super::symbolic::ExprError>,
}
impl Reduction {
    pub(super) fn unchanged(value: AbstractValue) -> Self {
        Self {
            value,
            status: ReductionStatus::Stable,
            rounds: 0,
            strengthened: 0,
            symbolic_error: None,
        }
    }
}
fn insert(
    lattice: &mut FactLattice,
    predicate: UnaryPredicate,
    changes: &mut usize,
) -> Result<(), FactError> {
    if lattice.insert(Fact::Unary(UnaryFact::new(Symbol::THIS, predicate)))?
        == FactChange::Strengthened
    {
        *changes += 1;
    }
    Ok(())
}
fn emit_bounds(
    lattice: &mut FactLattice,
    interval: Interval,
    changes: &mut usize,
) -> Result<(), FactError> {
    let (lo, hi) = interval.unsigned_bounds();
    insert(
        lattice,
        UnaryPredicate::UnsignedBounds(WordBounds::new(lo, hi)?),
        changes,
    )?;
    let (lo, hi) = interval.signed_bounds();
    insert(
        lattice,
        UnaryPredicate::SignedBounds(WordBounds::new(lo, hi)?),
        changes,
    )
}
fn emit_bits(
    lattice: &mut FactLattice,
    bits: KnownBits,
    changes: &mut usize,
) -> Result<(), FactError> {
    insert(
        lattice,
        UnaryPredicate::KnownBits(BitConstraints::new(bits.zero(), bits.one())?),
        changes,
    )
}
pub(super) fn export(
    domain: Domain,
    value: &AbstractValue,
    lattice: &mut FactLattice,
    changes: &mut usize,
) -> Result<(), FactError> {
    if let Some(values) = value.constants() {
        let values = values
            .iter()
            .copied()
            .filter(|v| value.contains(*v))
            .collect::<std::collections::BTreeSet<_>>();
        if values.is_empty() {
            return Err(FactError::Contradiction {
                subject: Symbol::THIS,
            });
        }
        insert(
            lattice,
            UnaryPredicate::MemberOf(FiniteSet::new(values)?),
            changes,
        )?;
    }
    emit_bits(lattice, value.numeric.bits, changes)?;
    emit_bounds(lattice, value.numeric.interval, changes)?;
    insert(
        lattice,
        UnaryPredicate::Congruent(value.numeric.congruence.clone()),
        changes,
    )?;
    insert(
        lattice,
        UnaryPredicate::PossibleOrigins(value.provenance.origins().clone()),
        changes,
    )?;
    if value.provenance.is_code_address() {
        insert(lattice, UnaryPredicate::IsCodeAddress, changes)?;
    }
    if !value.may_be_zero() {
        insert(lattice, UnaryPredicate::NonZero, changes)?;
    }
    if value.singleton() == Some(U256::ZERO) {
        insert(lattice, UnaryPredicate::IsZero, changes)?;
    }
    // 位摘要给 unsigned 界；界的共同前缀再反馈位摘要。
    let (lo, hi) = value.numeric.bits.unsigned_bounds();
    insert(
        lattice,
        UnaryPredicate::UnsignedBounds(WordBounds::new(lo, hi)?),
        changes,
    )?;
    let mut inferred_bits = None;
    let mut inferred_bounds = None;
    for (lo, hi) in value.numeric.interval.segments() {
        let bits = KnownBits::from_unsigned_bounds(lo, hi);
        inferred_bits = Some(inferred_bits.map_or(bits, |old: KnownBits| old.join(&bits)));
        // 同余先给安全端点，digit DP 在这些端点之间找 mask 的可行 min/max。
        if let Some((first, last)) = value
            .numeric
            .congruence
            .first_last(lo, hi)
            .and_then(|(a, b)| query::mask_bounds(a, b, value.numeric.bits))
        {
            let interval = Interval::new_unsigned(first, last).expect("ordered endpoints");
            inferred_bounds =
                Some(inferred_bounds.map_or(interval, |old: Interval| old.join(&interval)));
        }
    }
    if let Some(bits) = inferred_bits {
        emit_bits(lattice, bits, changes)?;
    }
    let bounds = inferred_bounds.ok_or(FactError::Contradiction {
        subject: Symbol::THIS,
    })?;
    emit_bounds(lattice, bounds, changes)?;
    // 连续低位事实等价于模 2^k；不能把任意不连续 mask 当作一个同余。
    let known = value.numeric.bits.zero() | value.numeric.bits.one();
    let k = (0..256).take_while(|bit| known.bit(*bit)).count();
    if k == 256 {
        insert(
            lattice,
            UnaryPredicate::Exact(value.numeric.bits.one()),
            changes,
        )?;
    } else if k > 0 {
        let modulus = U256::from(1) << k;
        let residue = value.numeric.bits.one() & (modulus - U256::from(1));
        insert(
            lattice,
            UnaryPredicate::Congruent(Congruence::new(modulus, residue).expect("positive modulus")),
            changes,
        )?;
    }
    // 完整候选覆盖才能重建常量集合；从不截取候选的前 K 项。
    if let Some(values) = query::candidates(
        value.numeric(),
        domain.capacity().min(domain.spec().fact_limit()),
    ) {
        if values.is_empty() {
            return Err(FactError::Contradiction {
                subject: Symbol::THIS,
            });
        }
        insert(
            lattice,
            UnaryPredicate::MemberOf(FiniteSet::new(values)?),
            changes,
        )?;
    }
    Ok(())
}
pub(super) fn import(
    value: &AbstractValue,
    lattice: &FactLattice,
) -> Result<AbstractValue, FactError> {
    let Some(facts) = lattice.scalar(Symbol::THIS) else {
        return Ok(value.clone());
    };
    let mut out = value.clone();
    if let Some(bits) = facts.known_bits() {
        out.numeric.bits = out
            .numeric
            .bits
            .meet(&KnownBits::new(bits.zero(), bits.one()).expect("validated fact masks"))
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
    }
    if let Some(bounds) = facts.unsigned_bounds() {
        out.numeric.interval = out
            .numeric
            .interval
            .meet(
                &Interval::new_unsigned(bounds.lower(), bounds.upper()).expect("validated bounds"),
            )
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
    }
    if let Some(bounds) = facts.signed_bounds() {
        out.numeric.interval = out
            .numeric
            .interval
            .meet(&Interval::new_signed(bounds.lower(), bounds.upper()).expect("validated bounds"))
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
    }
    if let Some(congruence) = facts.congruence() {
        out.numeric.congruence =
            out.numeric
                .congruence
                .meet(congruence)
                .ok_or(FactError::Contradiction {
                    subject: Symbol::THIS,
                })?;
    }
    out.numeric.nonzero |= facts.nonzero();
    if facts.is_address() {
        let high = U256::MAX << 160usize;
        out.numeric.bits = out
            .numeric
            .bits
            .meet(&KnownBits::new(high, U256::ZERO).unwrap())
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
    }
    if let Some(origins) = facts.possible_origins() {
        let origins =
            out.provenance
                .origins()
                .meet(origins)
                .ok_or(FactError::OriginContradiction {
                    subject: Symbol::THIS,
                })?;
        // 元数据传播不复制身份；可信身份只由执行引擎签发。
        if &origins != out.provenance.origins() {
            out.provenance = Provenance::from_origins(origins);
        }
    }
    if facts.is_code_address() {
        out.provenance = out.provenance.with_code_address_role();
    }
    if !facts.finite_constants().is_top() {
        out.numeric.finite = out
            .numeric
            .finite
            .meet(facts.finite_constants())
            .map_err(|_| FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
    }
    if !out.numeric.finite.is_top() {
        let filtered = out
            .numeric
            .finite
            .filter(|v| out.contains(v))
            .map_err(|_| FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
        let values = filtered
            .as_values()
            .expect("a filtered finite set remains finite");
        out.numeric.bits = out
            .numeric
            .bits
            .meet(&KnownBits::from_values(values))
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
        out.numeric.interval = out
            .numeric
            .interval
            .meet(&Interval::from_values(values))
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
        out.numeric.congruence = out
            .numeric
            .congruence
            .meet(&Congruence::from_values(values))
            .ok_or(FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
        out.numeric.finite = filtered;
    }
    Ok(out)
}
pub(super) fn reduce(domain: Domain, mut value: AbstractValue) -> Reduction {
    let mut lattice = FactLattice::new(domain.spec().fact_limit());
    let mut changes = 0;
    for round in 0..domain.spec().reduction_rounds() {
        if let Err(error) = export(domain, &value, &mut lattice, &mut changes) {
            return Reduction {
                value,
                status: error_status(&error),
                rounds: round,
                strengthened: changes,
                symbolic_error: None,
            };
        }
        let next = match import(&value, &lattice) {
            Ok(next) => next,
            Err(error) => {
                return Reduction {
                    value,
                    status: error_status(&error),
                    rounds: round,
                    strengthened: changes,
                    symbolic_error: None,
                };
            }
        };
        if next == value {
            return Reduction {
                value: next,
                status: ReductionStatus::Stable,
                rounds: round + 1,
                strengthened: changes,
                symbolic_error: None,
            };
        }
        value = next;
    }
    Reduction {
        value,
        status: ReductionStatus::RoundLimit,
        rounds: domain.spec().reduction_rounds(),
        strengthened: changes,
        symbolic_error: None,
    }
}

fn error_status(error: &FactError) -> ReductionStatus {
    match error {
        FactError::Capacity { .. } => ReductionStatus::FactLimit,
        FactError::OriginContradiction { .. } => ReductionStatus::OriginConflict,
        _ => ReductionStatus::Empty,
    }
}
