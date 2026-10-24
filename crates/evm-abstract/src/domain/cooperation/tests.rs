use super::{
    BoundsToMask, KeepBounds, KeepMasks, MaskToBounds, ProductAlgebra, RuleError, RuleWork,
    close_order, test_support::Count,
};
use crate::domain::{
    DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, NumericValue, ReductionStatus,
    ValueAlgebra, WordElement, WordObservation, congruence::Congruence, interval::Interval,
    known_bits::KnownBits,
};
use alloy_primitives::U256;
use analysis_kernel::{LocalClosure, Reduction, Scope};
use revm_bytecode::opcode;
use std::{cell::Cell, num::NonZeroUsize};

mod integration;
mod work;

fn spec(rounds: usize, facts: usize) -> DomainSpec {
    DomainSpec::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(rounds).unwrap(),
        NonZeroUsize::new(facts).unwrap(),
    )
}
fn input() -> NumericValue {
    NumericValue {
        bits: KnownBits::new(U256::ZERO, U256::ONE).unwrap(),
        interval: Interval::new_unsigned(U256::from(2), U256::from(8)).unwrap(),
        ..NumericValue::top()
    }
}
#[test]
fn cyclic_typed_rules_close_in_both_orders_without_finite_enumeration() {
    let mut answers = Vec::new();
    for order in [true, false] {
        let masks = Cell::new(0);
        let bounds = Cell::new(0);
        let result = close_order(
            spec(8, 256),
            input(),
            Count {
                rule: BoundsToMask,
                calls: &masks,
            },
            Count {
                rule: MaskToBounds,
                calls: &bounds,
            },
            order,
        );
        assert_eq!(result.status, ReductionStatus::Stable);
        assert!(
            result.value.constants().is_none(),
            "capacity one cannot enumerate these three odd words"
        );
        assert_eq!(
            result.value.interval().unsigned_bounds(),
            (U256::from(3), U256::from(7))
        );
        assert_eq!(result.value.known_bits().zero(), !U256::from(7));
        // Starting with masks requires mask -> bounds -> mask -> bounds. When
        // bounds run first, the first mask evaluation can already see final
        // bounds; only the interval's tracked self-read must run again.
        assert!(bounds.get() > 1);
        assert!(masks.get() >= if order { 2 } else { 1 });
        assert!(
            masks.get() < 8 && bounds.get() < 8,
            "stable publications must stop rescheduling"
        );
        answers.push(result.value);
    }
    assert_eq!(answers[0], answers[1]);
}

#[test]
fn replaceable_components_change_production_numeric_reduction() {
    let complete = ValueAlgebra::new(ProductAlgebra::new(
        spec(8, 256),
        BoundsToMask,
        MaskToBounds,
    ));
    let no_masks = ValueAlgebra::new(ProductAlgebra::new(spec(8, 256), KeepMasks, MaskToBounds));
    let no_bounds = ValueAlgebra::new(ProductAlgebra::new(spec(8, 256), BoundsToMask, KeepBounds));
    let value = EvmValue::<NumericValue>::from_numeric(input());
    let full = complete.reduce(&value);
    let maskless = no_masks.reduce(&value);
    let boundless = no_bounds.reduce(&value);
    assert_eq!(full.status, ReductionStatus::Stable);
    assert_ne!(full.value.known_bits(), maskless.value.known_bits());
    assert_ne!(full.value.interval(), boundless.value.interval());
    for v in [3, 5, 7] {
        for output in [&full.value, &maskless.value, &boundless.value] {
            assert!(output.contains(U256::from(v)));
        }
    }
    // Both ordinary/default and explicit assembly use the same registered rules.
    assert_eq!(
        ValueAlgebra::new(ProductAlgebra::from_spec(spec(8, 256)))
            .reduce(&value)
            .value,
        full.value
    );
}

#[test]
fn bounded_statuses_and_fresh_inputs_preserve_safe_partial_values() {
    let domain = ValueAlgebra::new(ProductAlgebra::from_spec(spec(1, 1024)));
    let result = domain.apply_detailed(
        opcode::MUL,
        &[
            EvmValue::<NumericValue>::top(),
            EvmValue::<NumericValue>::constant(U256::ZERO),
        ],
    );
    assert_eq!(result.status, ReductionStatus::RoundLimit);
    assert_eq!(result.rounds, 1);
    assert!(result.strengthened > 0);
    assert_eq!(result.value.singleton(), Some(U256::ZERO));
    assert_eq!(domain.reduce(&result.value).status, ReductionStatus::Stable);
    let limited = ValueAlgebra::new(ProductAlgebra::from_spec(spec(8, 1))).apply_detailed(
        opcode::MUL,
        &[
            EvmValue::<NumericValue>::top(),
            EvmValue::<NumericValue>::constant(U256::ZERO),
        ],
    );
    assert_eq!(limited.status, ReductionStatus::FactLimit);
    assert_eq!(limited.value.singleton(), Some(U256::ZERO));
    let precise = ProductAlgebra::new(spec(8, 256), BoundsToMask, MaskToBounds);
    let first = precise.reduce_numeric(&NumericValue::constant(U256::ZERO));
    let second =
        precise.reduce_numeric(&NumericValue::unsigned_range(U256::ZERO, U256::from(100)).unwrap());
    assert_eq!(first.value.singleton(), Some(U256::ZERO));
    assert!(
        second.value.contains(U256::from(100)),
        "new input must not inherit an earlier generation's zero fact"
    );
}

#[test]
fn observation_roundtrips_full_product_and_native_json_keeps_actual_components() {
    let sign = U256::ONE << 255;
    let values = [U256::from(3), U256::from(9), sign + U256::from(3)];
    let numeric = NumericValue {
        finite: FiniteConstantSet::try_from_values(values.into_iter().collect()).unwrap(),
        bits: KnownBits::new(U256::from(2), U256::ONE).unwrap(),
        interval: Interval::top(),
        congruence: Congruence::new(U256::from(3), U256::ZERO).unwrap(),
        nonzero: true,
    };
    let observation = numeric.observation();
    assert_eq!(NumericValue::from_observation(&observation), numeric);
    // An observation is a semantic projection, not the native carrier's JSON
    // representation. The typed roundtrip above retains every component;
    // native sparse JSON independently retains every nondefault component.
    assert_eq!(
        serde_json::to_value(&numeric).unwrap(),
        serde_json::json!({
            "constants": values,
            "known_bits": numeric.known_bits(),
            "congruence": numeric.congruence(),
            "nonzero": true,
        })
    );
    let signed = NumericValue {
        interval: Interval::new_signed(U256::ONE, U256::from(19)).unwrap(),
        ..NumericValue::top()
    };
    assert_eq!(
        NumericValue::from_observation(&signed.observation()),
        signed
    );
    let constrained = numeric.constrain(&WordObservation::top()).unwrap();
    for word in values {
        assert_eq!(constrained.contains(word), numeric.contains(word));
    }
}

#[test]
fn large_default_operand_quotation_is_stable_and_does_not_run_reducers() {
    let _counter = super::PROTOCOL_WORK.scope();
    let values = (0u64..1024).map(U256::from).collect();
    let value = EvmValue::<NumericValue>::from_numeric(NumericValue {
        finite: FiniteConstantSet::try_from_values(values).unwrap(),
        ..NumericValue::top()
    });
    let before = value.clone();
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let first = domain.operation_work(opcode::NOT, std::slice::from_ref(&value));
    assert_eq!(super::PROTOCOL_WORK.get(), (0, 0, 0));
    for _ in 0..8 {
        assert_eq!(
            domain.operation_work(opcode::NOT, std::slice::from_ref(&value)),
            first
        );
        assert_eq!(super::PROTOCOL_WORK.get(), (0, 0, 0));
    }
    assert_eq!(value, before);
    assert!(first >= 1024);
}

#[test]
fn default_copy_and_singleton_hooks_price_the_native_literal_certificate() {
    let value = EvmValue::<NumericValue>::constant(U256::from(7));
    assert_eq!(value.numeric().clone_work(), 12);
    // Quote and query validate the fixed-width complete component certificate.
    assert_eq!(value.singleton_work(), 64 * 4);
    assert_eq!(
        value.clone_work(),
        20 + value.provenance().origins().work_size() * 4
    );
    let numeric = NumericValue {
        finite: FiniteConstantSet::top(),
        ..NumericValue::constant(U256::from(7))
    };
    assert_eq!(
        numeric.canonical_for_summary(),
        NumericValue::constant(U256::from(7))
    );
}

#[derive(Clone, Copy, Debug)]
struct UnsupportedBounds;
impl RuleWork for UnsupportedBounds {
    fn evaluation_work(&self, _: DomainSpec, _: usize) -> usize {
        0
    }
}
impl<Cx> Reduction<Cx> for UnsupportedBounds {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, _: &mut Cx) -> Result<LocalClosure, RuleError> {
        Err(crate::domain::facts::FactError::UnsupportedOperation(0xfe).into())
    }
}

#[test]
fn an_unavailable_component_is_not_a_numeric_contradiction() {
    let domain = ProductAlgebra::new(spec(8, 256), BoundsToMask, UnsupportedBounds);
    let before = input();
    let result = domain.reduce_numeric(&before);
    assert_eq!(result.status, ReductionStatus::RoundLimit);
    for member in [3, 5, 7] {
        assert!(result.value.contains(U256::from(member)));
    }
}

#[test]
fn rule_fact_conversions_preserve_payloads_and_display() {
    use crate::domain::facts::{FactError, Symbol};
    use analysis_kernel::PublishError;

    for fact in [
        FactError::Contradiction {
            subject: Symbol::new(17),
        },
        FactError::Capacity {
            max_atoms: 3,
            required: 5,
        },
        FactError::UnsupportedOperation(0xfe),
    ] {
        let direct = RuleError::from(fact.clone());
        let published = RuleError::from(PublishError::Policy(fact.clone()));
        for error in [direct, published] {
            assert_eq!(error.to_string(), fact.to_string());
            let RuleError::Fact(retained) = error else {
                panic!("a semantic-policy error became a kernel failure");
            };
            assert_eq!(retained, fact);
        }
    }
}

#[test]
fn rule_kernel_conversions_keep_interruptions_distinct_from_numeric_bottom() {
    use crate::domain::WorkExhausted;
    use analysis_kernel::{KernelError, PublishError};

    for kernel in [
        KernelError::Stopped(WorkExhausted),
        KernelError::CostOverflow,
        KernelError::WrongEpoch,
    ] {
        let direct = RuleError::from(kernel.clone());
        let published = RuleError::from(PublishError::Kernel(kernel.clone()));
        for error in [direct, published] {
            assert_eq!(error.to_string(), kernel.to_string());
            let RuleError::Kernel(retained) = error else {
                panic!("a kernel failure became a numerical contradiction");
            };
            assert_eq!(retained, kernel);
        }
    }
}
