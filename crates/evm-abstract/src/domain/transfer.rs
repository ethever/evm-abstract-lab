//! 运算关系和可信复制事实通过同一语义语言进入 transfer；
//! 同名来源和相同抽象摘要从不产生 Eq。
use super::{
    Domain, NumericReduction, NumericValue,
    congruence::Congruence,
    facts::{Fact, FactLattice, OperationRelation, RelationalFact, Symbol, Term},
    interval::Interval,
    known_bits::KnownBits,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) fn apply(
    domain: Domain,
    op: u8,
    args: &[NumericValue],
    reduce: impl FnOnce(NumericValue) -> NumericReduction<NumericValue>,
) -> NumericReduction<NumericValue> {
    let operands = (0..args.len())
        .map(|i| Term::Symbol(Symbol::new(i as u32 + 1)))
        .collect::<Vec<_>>();
    let relation = OperationRelation::new(op, Symbol::THIS, operands)
        .expect("facade validates pure operation and arity");
    let mut facts = FactLattice::new(domain.spec().fact_limit());
    let relation_available = facts
        .insert(Fact::Relation(RelationalFact::Operation(relation.clone())))
        .is_ok();
    if relation.opcode() == opcode::ISZERO && (!args[0].may_be_zero() || !args[0].may_be_nonzero())
    {
        return NumericReduction::stable(NumericValue::constant(U256::from(u8::from(
            !args[0].may_be_nonzero(),
        ))));
    }
    if let Some(exact) = args
        .iter()
        .map(NumericValue::singleton)
        .collect::<Option<Vec<_>>>()
    {
        let value = NumericValue::constant(super::evaluate(
            relation.opcode(),
            exact[0],
            exact.get(1).copied().unwrap_or(U256::ZERO),
            exact.get(2).copied().unwrap_or(U256::ZERO),
        ));
        return NumericReduction::stable(value);
    }
    let finite_value = domain.numeric_finite_apply(relation.opcode(), args);
    if !finite_value.finite.is_top() && args.iter().all(|v| v.constants().is_some()) {
        // 完整枚举已给精确 scalar 集合；重新传播其等价摘要不会增加信息。
        return NumericReduction::stable(finite_value);
    }
    let finite = finite_value.finite;
    let value = NumericValue {
        finite,
        bits: KnownBits::transfer(
            relation.opcode(),
            &args.iter().map(|v| v.bits).collect::<Vec<_>>(),
        ),
        interval: Interval::transfer(
            relation.opcode(),
            &args.iter().map(|v| v.interval).collect::<Vec<_>>(),
        ),
        congruence: Congruence::transfer(
            relation.opcode(),
            &args
                .iter()
                .map(|v| v.congruence.clone())
                .collect::<Vec<_>>(),
        ),
        nonzero: false,
    };
    let mut reduced = reduce(value);
    if !relation_available
        && !matches!(
            reduced.status,
            super::ReductionStatus::WorkLimit
                | super::ReductionStatus::Cancelled
                | super::ReductionStatus::KernelFailure
        )
    {
        reduced.status = super::ReductionStatus::FactLimit;
    }
    reduced
}
