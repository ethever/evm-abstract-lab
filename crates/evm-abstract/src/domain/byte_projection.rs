//! Conservative byte observations for the content reader's numeric policy.
//!
//! Each component transfer covers its input component, so their intersection
//! covers every concrete observed byte without exchanging facts. This private
//! boundary deliberately omits optional reduction; ordinary execution still
//! uses Domain::apply, including all finite and exact scalar inputs here.

use super::{
    AbstractValue, Domain, FiniteConstantSet, NumericValue, Profile, congruence::Congruence,
    interval::Interval, known_bits::KnownBits, provenance::Provenance,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

impl Domain {
    /// Observe a byte after the caller has funded the original operation_work.
    /// Only literal-index BYTE and trailing literal AND255 on open scalars use
    /// component transfers. Every other request retains ordinary evaluation.
    pub(crate) fn observe_byte(&self, op: u8, args: &[AbstractValue]) -> AbstractValue {
        if projection_scalar(*self, op, args).is_none() {
            return self.apply(op, args);
        }
        // The validated scalar has no finite candidates. finite_apply would
        // therefore return finite Top for either supported opcode as well.
        let mut value = AbstractValue::from_numeric(NumericValue {
            finite: FiniteConstantSet::top(),
            bits: KnownBits::transfer(op, &[args[0].numeric.bits, args[1].numeric.bits]),
            interval: Interval::transfer(op, &[args[0].numeric.interval, args[1].numeric.interval]),
            congruence: Congruence::transfer(
                op,
                &[
                    args[0].numeric.congruence.clone(),
                    args[1].numeric.congruence.clone(),
                ],
            ),
            nonzero: false,
        });
        // Known suffixes and out-of-range indices can already prove an exact
        // byte. Preserve its usual finite/nonzero summaries without reduction.
        if let Some(exact) = value.singleton() {
            value = value.with_numeric(NumericValue::constant(exact));
        }
        value.provenance =
            Provenance::transfer(&[args[0].provenance.clone(), args[1].provenance.clone()]);
        value.symbolic_limit =
            args.iter().any(AbstractValue::symbolic_limit_reached) && value.singleton().is_none();
        value
    }
}

fn projection_scalar(domain: Domain, op: u8, args: &[AbstractValue]) -> Option<&AbstractValue> {
    if domain.spec().profile() != Profile::Product || domain.spec().relations().enabled {
        return None;
    }
    let [left, right] = args else { return None };
    let literal = |value: &AbstractValue| {
        value
            .finite_constants()
            .singleton()
            .filter(|literal| value.contains(*literal))
    };
    let scalar = match op {
        opcode::BYTE if literal(left).is_some() => right,
        opcode::AND if literal(right) == Some(U256::from(255)) => left,
        _ => return None,
    };
    // Keep the original complete finite transfer even when its output exceeds
    // capacity, and keep exact scalars represented by any numeric component.
    (scalar.constants().is_none() && scalar.singleton().is_none()).then_some(scalar)
}

#[cfg(test)]
mod tests;
