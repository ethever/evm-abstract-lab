//! 临时规约只传播当前 product 蕴含的事实。每轮事务式导出/导入，
//! FactLattice 对同一语义槽取 meet；重复回馈不会生成新的日志条目。
use super::{
    AbstractValue, Domain, EvmValue, NumericReduction, NumericValue, WordElement, WordObservation,
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
    /// 事实原子容量不足，保留此前完整导入的安全结果。
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
pub struct EvmReduction<N: WordElement> {
    /// 覆盖全部具体执行结果的约束。
    pub value: EvmValue<N>,
    /// 终止边界。
    pub status: ReductionStatus,
    /// 策略报告的已完成数值规约次数。默认协作 product 计数 scalar
    /// 事实导出/导入的完整执行；其他依赖任务是否稳定由 `status` 表示。
    pub rounds: usize,
    /// 已接纳的严格增强事实次数。
    pub strengthened: usize,
    /// Additional typed expression-retention failure, independent of numeric exchange status.
    pub symbolic_error: Option<super::symbolic::ExprError>,
}
/// Concrete default-domain compatibility reduction.
pub type Reduction = EvmReduction<super::NumericValue>;

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
pub(super) fn import<N: WordElement>(
    value: &EvmValue<N>,
    lattice: &FactLattice,
) -> Result<EvmValue<N>, FactError> {
    let Some(facts) = lattice.scalar(Symbol::THIS) else {
        return Ok(value.clone());
    };
    let mut out = value.clone();
    out.numeric = out
        .numeric
        .constrain(&WordObservation::from_scalar(facts)?)?;
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
    Ok(out)
}

pub(super) fn numeric(domain: Domain, value: NumericValue) -> NumericReduction<NumericValue> {
    super::cooperation::close(
        domain,
        value,
        super::cooperation::BoundsToMask,
        super::cooperation::MaskToBounds,
    )
}

pub(super) fn error_status(error: &FactError) -> ReductionStatus {
    match error {
        FactError::Capacity { .. } => ReductionStatus::FactLimit,
        FactError::OriginContradiction { .. } => ReductionStatus::OriginConflict,
        FactError::Contradiction { .. }
        | FactError::EmptyFiniteSet
        | FactError::ConflictingBits
        | FactError::InvalidBounds => ReductionStatus::Empty,
        // Optional/unsupported rule imports are not proofs about this universe.
        FactError::InvalidBitIndex(_)
        | FactError::InvalidModulus
        | FactError::UnsupportedOperation(_)
        | FactError::InvalidOperation { .. } => ReductionStatus::RoundLimit,
    }
}
