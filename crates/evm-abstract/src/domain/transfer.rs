//! Pure numeric transfer over validated EVM operands and component guarantees.
//! Source labels and equal abstract summaries never establish value equality.
use super::{
    Domain, NumericReduction, NumericValue, congruence::Congruence, interval::Interval,
    known_bits::KnownBits,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

// The common entrypoint validates arity first. Query the complete product before raw
// finite enumeration: another component can narrow that finite cover further.
fn exact(op: u8, args: &[&NumericValue]) -> Option<NumericValue> {
    let mut words = [U256::ZERO; 3];
    for (i, value) in args.iter().enumerate() {
        if op == opcode::ISZERO && (!value.may_be_zero() || !value.may_be_nonzero()) {
            return Some(NumericValue::constant(U256::from(u8::from(
                !value.may_be_nonzero(),
            ))));
        }
        words[i] = value.singleton()?;
    }
    Some(NumericValue::constant(super::evaluate(
        op, words[0], words[1], words[2],
    )))
}

pub(super) fn apply(
    domain: Domain,
    op: u8,
    args: &[&NumericValue],
    reduce: impl FnOnce(NumericValue) -> NumericReduction<NumericValue>,
) -> NumericReduction<NumericValue> {
    if let Some(exact) = exact(op, args) {
        return NumericReduction::stable(exact);
    }
    let finite_value = domain.numeric_finite_apply(op, args);
    if !finite_value.finite.is_top() && args.iter().all(|v| v.constants().is_some()) {
        // 完整枚举已给精确 scalar 集合；重新传播其等价摘要不会增加信息。
        return NumericReduction::stable(finite_value);
    }
    let finite = finite_value.finite;
    let value = NumericValue {
        finite,
        bits: KnownBits::transfer(op, &args.iter().map(|v| v.bits).collect::<Vec<_>>()),
        interval: Interval::transfer(op, &args.iter().map(|v| v.interval).collect::<Vec<_>>()),
        congruence: Congruence::transfer(
            op,
            &args
                .iter()
                .map(|v| v.congruence.clone())
                .collect::<Vec<_>>(),
        ),
        nonzero: false,
    };
    let mut reduced = reduce(value);
    // A result and its operands need exactly one operation atom plus one per
    // operand. No other fact was retained or queried by this local admission.
    if domain.spec().fact_limit() < 1 + args.len()
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
