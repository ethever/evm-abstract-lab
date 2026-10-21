use super::*;
use std::{collections::BTreeSet, fmt, num::NonZeroUsize};

pub(super) static NUMERIC_EXECUTIONS: crate::test_instrumentation::Counter<(usize, usize)> =
    crate::test_instrumentation::Counter::new((0, 0));

fn algebra(capacity: usize) -> FiniteSetAlgebra {
    FiniteSetAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    ))
}

fn finite(values: impl IntoIterator<Item = U256>) -> FiniteConstantSet {
    FiniteConstantSet::try_from_values(values.into_iter().collect()).unwrap()
}

fn words(values: &[u64]) -> FiniteConstantSet {
    finite(values.iter().copied().map(U256::from))
}

#[test]
fn borrowed_native_transfer_keeps_exact_finite_results_and_evm_pop_order() {
    let domain = algebra(8);
    let operands = [words(&[0, 3]), words(&[1, 2])];
    let sub = domain.apply_numeric(opcode::SUB, operands.iter());
    assert_eq!(
        sub.value,
        finite([U256::MAX, U256::MAX - U256::ONE, U256::ONE, U256::from(2)])
    );
    assert_eq!(sub.status, super::super::ReductionStatus::Stable);
    assert_eq!((sub.rounds, sub.strengthened), (0, 0));

    let operands = [words(&[0, 9]), words(&[0, 3])];
    assert_eq!(
        domain.apply_numeric(opcode::DIV, operands.iter()).value,
        words(&[0, 3])
    );
    let operands = [words(&[2, 3]), words(&[5]), words(&[3])];
    assert_eq!(
        domain.apply_numeric(opcode::MULMOD, operands.iter()).value,
        words(&[0, 1])
    );
    let operands = [finite([U256::ZERO, U256::MAX])];
    assert_eq!(
        domain.apply_numeric(opcode::CLZ, operands.iter()).value,
        words(&[0, 256])
    );
}

#[test]
fn finite_transfer_covers_every_tuple_for_representative_opcodes() {
    let domain = algebra(16);
    let left = finite([U256::ZERO, U256::ONE, U256::MAX]);
    let right = words(&[0, 2]);
    for op in [
        opcode::ADD,
        opcode::MUL,
        opcode::SDIV,
        opcode::SMOD,
        opcode::EXP,
        opcode::SIGNEXTEND,
        opcode::SLT,
        opcode::SGT,
        opcode::AND,
        opcode::OR,
        opcode::XOR,
        opcode::BYTE,
        opcode::SHL,
        opcode::SHR,
        opcode::SAR,
    ] {
        let result = domain.apply_numeric(op, [&left, &right].into_iter());
        let expected = finite(left.as_values().unwrap().iter().flat_map(|a| {
            right
                .as_values()
                .unwrap()
                .iter()
                .map(move |b| concrete::evaluate(op, *a, *b, U256::ZERO))
        }));
        assert_eq!(result.value, expected, "opcode {op:#x}");
    }
}

#[test]
fn unsupported_or_incomplete_operands_remain_reachable_top() {
    let domain = algebra(8);
    let operand = words(&[7]);
    for (op, args) in [
        (opcode::STOP, vec![]),
        (opcode::ADD, vec![&operand]),
        (opcode::ADDMOD, vec![&operand, &operand]),
        (0xfe, vec![&operand]),
    ] {
        let result = domain.apply_numeric(op, args.into_iter());
        assert!(result.value.is_top());
        assert_ne!(result.status, super::super::ReductionStatus::Empty);
        assert!(result.value.contains(U256::ZERO));
        assert!(result.value.contains(U256::MAX));
    }
    let top = FiniteConstantSet::top();
    assert!(
        domain
            .apply_numeric(opcode::ADD, [&top, &operand].into_iter())
            .value
            .is_top()
    );
}

#[test]
fn small_instruction_images_are_complete_and_capacity_checked() {
    let top = FiniteConstantSet::top();
    assert_eq!(
        algebra(2)
            .apply_numeric(opcode::ISZERO, [&top].into_iter())
            .value,
        words(&[0, 1])
    );
    assert!(
        algebra(1)
            .apply_numeric(opcode::ISZERO, [&top].into_iter())
            .value
            .is_top()
    );
    let index = words(&[0]);
    let byte = algebra(256).observe_byte_numeric(opcode::BYTE, [&index, &top].into_iter());
    assert_eq!(byte.cardinality(), Some(256));
    assert!(byte.contains(U256::ZERO));
    assert!(byte.contains(U256::from(255)));
    assert!(!byte.contains(U256::from(256)));
    let clz = algebra(257)
        .apply_numeric(opcode::CLZ, [&top].into_iter())
        .value;
    assert_eq!(clz.cardinality(), Some(257));
}

#[test]
fn projection_join_and_widening_cover_all_input_values() {
    let domain = algebra(3);
    let old = words(&[1, 2]);
    let next = words(&[2, 3]);
    assert_eq!(domain.join_numeric(&old, &next), words(&[1, 2, 3]));
    assert_eq!(domain.widen_numeric(&old, &next), words(&[1, 2, 3]));
    for result in [
        domain.project_numeric(&words(&[1, 2, 3, 4])),
        domain.reduce_numeric(&words(&[1, 2, 3, 4])).value,
        domain.join_numeric(&old, &words(&[3, 4])),
        domain.widen_numeric(&old, &words(&[3, 4])),
        domain.widen_procedure_numeric(&old, &words(&[3, 4])),
    ] {
        assert!(result.is_top());
        for word in 1..=4 {
            assert!(result.contains(U256::from(word)));
        }
    }
}

#[test]
fn procedure_widening_preserves_stability_and_covers_moving_sets() {
    let domain = algebra(8);
    let old = words(&[1, 2, 3, 4]);
    assert_eq!(domain.widen_procedure_numeric(&old, &old), old);
    let next = words(&[2, 3, 4, 5]);
    let widened = domain.widen_procedure_numeric(&old, &next);
    assert!(widened.is_top());
    assert_eq!(
        domain.reduce_numeric(&widened).value,
        FiniteConstantSet::top()
    );
}

#[test]
fn output_overflow_discards_the_whole_set() {
    let domain = algebra(2);
    let left = words(&[0, 1]);
    let right = words(&[1, 3]);
    let result = domain.apply_numeric(opcode::ADD, [&left, &right].into_iter());
    assert!(result.value.is_top());
    for concrete in [1, 2, 3, 4] {
        assert!(result.value.contains(U256::from(concrete)));
    }
}

#[test]
fn complete_cartesian_transfer_is_admitted_by_the_root_budget() {
    use crate::{
        domain::{EvmValue, ValueAlgebra},
        resource::WorkBudget,
    };

    let domain = ValueAlgebra::new(algebra(512));
    let args = [
        EvmValue::from_numeric(finite((0u64..=64).map(U256::from))),
        EvmValue::from_numeric(finite((0u64..=64).map(U256::from))),
    ];
    let _scope = NUMERIC_EXECUTIONS.scope();
    let quote = domain.operation_work(opcode::ADD, &args);
    assert!(quote < usize::MAX);
    assert_eq!(NUMERIC_EXECUTIONS.get(), (0, 0));

    let mut denied = WorkBudget::new(quote - 1);
    assert!(
        domain
            .apply_budgeted(opcode::ADD, &args, &mut denied)
            .is_err()
    );
    assert!(denied.exhausted());
    assert_eq!(denied.used(), 0);
    assert_eq!(NUMERIC_EXECUTIONS.get(), (0, 0));

    let mut paid = WorkBudget::new(quote);
    let result = domain
        .apply_budgeted(opcode::ADD, &args, &mut paid)
        .unwrap();
    assert_eq!(
        result.value.numeric(),
        &finite((0u64..=128).map(U256::from))
    );
    assert_eq!(result.value.constants().unwrap().len(), 129);
    assert_eq!(paid.used(), quote);
    assert_eq!(NUMERIC_EXECUTIONS.get(), (1, 65 * 65));

    // The unmetered entrypoint is caller-funded and has the same precision.
    assert_eq!(
        domain.apply(opcode::ADD, &args).numeric(),
        result.value.numeric()
    );
    assert_eq!(NUMERIC_EXECUTIONS.get(), (2, 2 * 65 * 65));
}

#[test]
fn observations_retain_finite_disjunctions_and_refinement_never_invents_bottom() {
    let source = words(&[0, 1, 2, 3]);
    let observation = source.observation();
    assert_eq!(observation.constants(), source.as_values());
    assert_eq!(FiniteConstantSet::from_observation(&observation), source);
    assert!(!source.requires_retained_binding());

    let nonzero = WordObservation::nonzero_word();
    assert_eq!(source.constrain(&nonzero).unwrap(), words(&[1, 2, 3]));
    let range = WordObservation::unsigned_range(U256::from(2), U256::from(4)).unwrap();
    assert_eq!(source.constrain(&range).unwrap(), words(&[2, 3]));
    assert!(FiniteConstantSet::top().constrain(&range).unwrap().is_top());
    assert!(FiniteConstantSet::from_observation(&range).is_top());
    assert!(
        source
            .constrain(&WordObservation::constant(U256::from(9)))
            .is_err()
    );
    assert_eq!(
        FiniteConstantSet::top().constrain(&observation).unwrap(),
        source
    );
}

#[test]
fn observation_import_filters_a_complete_cover_by_every_guarantee() {
    let observation = WordObservation {
        finite: words(&[0, 1, 2, 3]),
        nonzero: true,
        ..WordObservation::unsigned_range(U256::from(1), U256::from(2)).unwrap()
    };
    assert_eq!(
        FiniteConstantSet::from_observation(&observation),
        words(&[1, 2])
    );
    assert_eq!(
        words(&[0, 1, 3]).constrain(&observation).unwrap(),
        words(&[1])
    );
    assert_eq!(
        FiniteConstantSet::from_observation(&WordObservation::constant(U256::MAX)),
        FiniteConstantSet::constant(U256::MAX)
    );
}

#[test]
fn work_quotes_price_opcode_queries_shared_copies_and_overflow() {
    let domain = algebra(8);
    let one = words(&[1]);
    let many = words(&[1, 2, 3, 4]);
    assert_eq!(one.clone_work(), many.clone_work());
    assert_eq!(
        many.singleton_work(),
        FiniteConstantSet::top().singleton_work()
    );
    assert!(one.singleton_work() > many.singleton_work());
    let add = domain.operation_work(opcode::ADD, [&one, &one].into_iter());
    let exp = domain.operation_work(opcode::EXP, [&one, &one].into_iter());
    assert!(exp > add);
    assert!(domain.operation_work(opcode::ADD, [&many, &many].into_iter()) > add);
    assert!(domain.literal_quote_work(U256::MAX) < usize::MAX);
    assert_eq!(domain.projection_work(&many), domain.projection_work(&one));
    assert!(many.constrain_work(&many.observation()) >= many.clone_work());
    assert_eq!(
        domain.refinement_import_work(&many, usize::MAX, usize::MAX, usize::MAX),
        Some(usize::MAX)
    );
    let large_spec = DomainSpec::new(
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(usize::MAX).unwrap(),
    );
    let large_domain = FiniteSetAlgebra::from_spec(large_spec);
    assert_eq!(large_domain.refinement_work(&many, 1), usize::MAX);
    assert!(large_domain.operation_work(opcode::ADD, [&one, &one].into_iter()) < usize::MAX);
    // These cached sizes exercise true cardinality overflow without allocating
    // enormous fixtures. The same checked product serves transfer and quote;
    // a fully finite overflow is explicitly quoted MAX before root execution.
    assert_eq!(
        cartesian_visits([Some(usize::MAX / 2 + 1), Some(2)].into_iter()),
        None
    );
}

#[test]
fn facade_exp_quote_prepays_retained_constant_folding() {
    use crate::{
        domain::{EvmValue, ValueAlgebra, symbolic::ExprId},
        resource::WorkBudget,
    };

    let domain = ValueAlgebra::new(algebra(8));
    // For odd 3, 3^(2^256 - 1) is its inverse modulo 2^256. This independent
    // closed form avoids deriving the expected value from the evaluated path.
    let expected = (U256::MAX / U256::from(3)) * U256::from(2) + U256::ONE;
    for [base, exponent] in [
        [EvmValue::<FiniteConstantSet>::top(), EvmValue::top()],
        [
            EvmValue::from_numeric(words(&[2, 3])),
            EvmValue::from_numeric(finite([U256::from(2), U256::MAX])),
        ],
    ] {
        let _scope = NUMERIC_EXECUTIONS.scope();
        let args = [
            base.with_expression(ExprId::constant(U256::from(3))),
            exponent.with_expression(ExprId::constant(U256::MAX)),
        ];
        let quote = domain.operation_work(opcode::EXP, &args);
        assert_eq!(NUMERIC_EXECUTIONS.get(), (0, 0));

        // Even the single possible facade EXP exceeds this reservation. The
        // previous Top quote admitted it and actually performed the operation.
        let mut too_small = WorkBudget::new(evaluation_work(opcode::EXP) - 1);
        assert!(
            domain
                .apply_budgeted(opcode::EXP, &args, &mut too_small)
                .is_err()
        );
        assert!(too_small.exhausted());
        assert_eq!(too_small.used(), 0);
        assert_eq!(NUMERIC_EXECUTIONS.get(), (0, 0));

        let mut almost = WorkBudget::new(quote - 1);
        assert!(
            domain
                .apply_budgeted(opcode::EXP, &args, &mut almost)
                .is_err()
        );
        assert_eq!(almost.used(), 0);
        assert_eq!(NUMERIC_EXECUTIONS.get(), (0, 0));

        let mut paid = WorkBudget::new(quote);
        let result = domain
            .apply_budgeted(opcode::EXP, &args, &mut paid)
            .unwrap();
        assert_eq!(result.value.singleton(), Some(expected));
        assert_eq!(result.status, super::super::ReductionStatus::Stable);
        assert_eq!(paid.used(), quote);
        let (transfers, evaluations) = NUMERIC_EXECUTIONS.get();
        assert_eq!(transfers, 1);
        // The actual numeric evaluations alone do not produce a singleton for
        // either input shape. The extra evaluation is the facade's exact fold.
        assert_eq!(evaluations, if args[0].numeric().is_top() { 0 } else { 4 });
        assert!(quote >= (evaluations + 1) * evaluation_work(opcode::EXP));
    }
}

#[test]
fn finite_capabilities_decline_product_specific_semantic_certificates() {
    let domain = algebra(8);
    let top = FiniteConstantSet::top();
    assert!(domain.byte_quotes_uniform());
    assert!(!domain.uniform_word_bytes(&top));
    assert!(!domain.byte_image_candidate(&top));
    assert!(!domain.byte_image_complete(&top));
    assert!(!domain.uniform_byte_top(&top));
    assert!(!domain.supports_product_scalar_shortcuts());
    assert_eq!(domain.pack_word_work(&[&top; 32]), None);
    assert_eq!(domain.pack_word_numeric(&[&top; 32]), None);

    let word = words(&[3, 9]);
    let quotes: BTreeSet<_> = (0u64..32)
        .map(|index| {
            let index = FiniteConstantSet::constant(U256::from(index));
            domain.operation_work(opcode::BYTE, [&index, &word].into_iter())
        })
        .collect();
    assert_eq!(quotes.len(), 1);
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct CustomWord {
    finite: FiniteConstantSet,
    private_guarantee: bool,
}

impl fmt::Display for CustomWord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.finite.fmt(formatter)
    }
}

impl WordElement for CustomWord {
    fn top() -> Self {
        Self {
            finite: FiniteConstantSet::top(),
            private_guarantee: false,
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            finite: FiniteConstantSet::constant(word),
            private_guarantee: false,
        }
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self {
            finite: FiniteConstantSet::from_observation(observation),
            private_guarantee: false,
        }
    }
    fn observation(&self) -> WordObservation {
        self.finite.observation()
    }
    fn contains(&self, word: U256) -> bool {
        self.finite.contains(word) && (!self.private_guarantee || !word.bit(0))
    }
    fn is_top(&self) -> bool {
        self.finite.is_top() && !self.private_guarantee
    }
    fn work_size(&self) -> usize {
        WordElement::work_size(&self.finite).saturating_add(1)
    }
}

#[test]
fn standard_observations_do_not_claim_to_retain_custom_constraints() {
    let custom = CustomWord {
        finite: words(&[1, 2]),
        private_guarantee: true,
    };
    assert!(custom.requires_retained_binding());
    assert!(!custom.contains(U256::ONE));
    assert!(custom.observation().contains(U256::ONE));
    assert_eq!(custom.constrain(&WordObservation::top()).unwrap(), custom);
    let imported = CustomWord::from_observation(&custom.observation());
    assert_eq!(imported.finite, custom.finite);
    assert!(!imported.private_guarantee);
    assert!(imported.contains(U256::ONE));
    assert_ne!(imported, custom);
}
