use crate::{
    domain::{
        AbstractValue, Domain, DomainSpec, EvmNumericAlgebra, FiniteConstantSet, NumericValue,
        Profile, ReductionStatus, ValueAlgebra, WordElement, WordObservation,
        congruence::Congruence,
        cooperation::{self, BoundsToMask, MaskToBounds, ProductAlgebra, RuleError},
        interval::Interval,
        known_bits::KnownBits,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use analysis_kernel::{LocalClosure, Reduction, Scope};
use revm_bytecode::opcode;
use std::{cell::Cell, num::NonZeroUsize, rc::Rc};

fn domain(capacity: usize, rounds: usize) -> Domain {
    Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(rounds).unwrap(),
        NonZeroUsize::new(4096).unwrap(),
    ))
}

fn cycle() -> NumericValue {
    NumericValue {
        bits: KnownBits::new(U256::ZERO, U256::ONE).unwrap(),
        interval: Interval::new_unsigned(U256::from(2), U256::from(8)).unwrap(),
        ..NumericValue::top()
    }
}

fn finite(size: usize) -> NumericValue {
    NumericValue {
        finite: FiniteConstantSet::try_from_values((0..size).map(U256::from).collect()).unwrap(),
        ..NumericValue::top()
    }
}

#[derive(Clone, Copy, Debug)]
struct CountMasks<'a>(&'a Cell<usize>);
impl<Cx> Reduction<Cx> for CountMasks<'_>
where
    BoundsToMask: Reduction<Cx, Task = Scope, Error = RuleError>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, task: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.0.set(self.0.get() + 1);
        BoundsToMask.reduce(task, cx)
    }
}

#[derive(Clone, Copy, Debug)]
struct CountBounds<'a>(&'a Cell<usize>);
impl<Cx> Reduction<Cx> for CountBounds<'_>
where
    MaskToBounds: Reduction<Cx, Task = Scope, Error = RuleError>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, task: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.0.set(self.0.get() + 1);
        MaskToBounds.reduce(task, cx)
    }
}

#[test]
fn closure_quote_funds_rule_and_graph_reservations_across_capacity_and_step_limits() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 32, 512] {
        for rounds in [1, 2, 4, 8] {
            let domain = domain(capacity, rounds);
            for input in [
                NumericValue::top(),
                NumericValue::constant(U256::from(7)),
                cycle(),
                NumericValue::unsigned_range(U256::from(2), U256::from(100)).unwrap(),
                finite(capacity),
                finite(capacity + 1),
                finite(1024),
            ] {
                let masks = Cell::new(0);
                let bounds = Cell::new(0);
                let algebra =
                    ProductAlgebra::new(domain.spec(), CountMasks(&masks), CountBounds(&bounds));
                let result = algebra.reduce_numeric(&input);
                let (used, dependencies, steps) = cooperation::PROTOCOL_WORK.get();
                let quote = domain.numeric_refinement_work(&input, 0);
                assert!(
                    used <= quote,
                    "capacity={capacity} rounds={rounds}: {used}>{quote}"
                );
                assert_eq!(dependencies, 10);
                assert!(steps <= 1 + 3 * rounds);
                assert!(result.rounds <= bounds.get());
                let semantic = masks.get() * 256
                    + bounds.get() * 4096
                    + result.rounds * (4096 + 256 * capacity + 32 * (capacity + 32));
                assert!(used > semantic, "graph/meet reservations must remain paid");
                if input.constants().is_some() && rounds >= 2 {
                    assert_eq!(result.status, ReductionStatus::Stable);
                    assert!(result.rounds <= 2 && masks.get() <= 2 && bounds.get() <= 2);
                }
            }
        }
    }
}

#[test]
fn stable_inputs_pay_one_cycle_and_finite_inputs_pay_at_most_two() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 512] {
        let one = domain(capacity, 1);
        let two = domain(capacity, 2);
        let eight = domain(capacity, 8);
        for value in [NumericValue::top(), NumericValue::constant(U256::from(7))] {
            assert_eq!(
                one.numeric_refinement_work(&value, 0),
                eight.numeric_refinement_work(&value, 0)
            );
            let result = eight.reduce_numeric(&value);
            assert_eq!(result.status, ReductionStatus::Stable);
            assert_eq!(result.value, value);
            assert_eq!(result.rounds, 1);
            assert_eq!(cooperation::PROTOCOL_WORK.get().2, 4);
        }
        let value = finite(128);
        assert_eq!(
            two.numeric_refinement_work(&value, 1),
            eight.numeric_refinement_work(&value, 1)
        );
        assert!(
            eight.numeric_refinement_work(&NumericValue::top(), 1)
                > one.numeric_refinement_work(&NumericValue::top(), 1),
            "new facts can turn Top into a product requiring feedback"
        );
        let literal = NumericValue::constant(U256::from(7));
        assert_eq!(
            one.numeric_refinement_work(&literal, 1),
            eight.numeric_refinement_work(&literal, 1)
        );
        let algebra = ValueAlgebra::new(eight);
        let refined = algebra
            .constrain_numeric(&literal, &WordObservation::nonzero_word())
            .unwrap();
        assert_eq!(refined.value, literal);
        assert_eq!(refined.status, ReductionStatus::Stable);
        assert!(cooperation::PROTOCOL_WORK.get().0 <= eight.numeric_refinement_work(&literal, 1));
    }
}

#[test]
fn filtered_finite_covers_stabilize_without_capacity_truncation() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 32] {
        for size in [4, 9, 128] {
            let domain = domain(capacity, 8);
            let value = NumericValue {
                bits: KnownBits::new(U256::ZERO, U256::ONE).unwrap(),
                interval: Interval::new_unsigned(U256::from(2), U256::from(size - 1)).unwrap(),
                ..finite(size)
            };
            let result = domain.reduce_numeric(&value);
            assert_eq!(result.status, ReductionStatus::Stable);
            assert!(result.rounds <= 2);
            assert_eq!(
                result.value.constants().unwrap(),
                &(3..size).step_by(2).map(U256::from).collect()
            );
            assert!(
                cooperation::PROTOCOL_WORK.get().0 <= domain.numeric_refinement_work(&value, 0)
            );
        }
    }
}

#[test]
fn arithmetic_reservation_is_atomic_and_covers_the_actual_cooperative_schedule() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 8, 512] {
        for rounds in [1, 2, 4, 8] {
            let domain = ValueAlgebra::new(domain(capacity, rounds));
            for (op, arity) in (0x01..=0x0b).chain(0x10..=0x1e).map(|op| {
                let arity = match op {
                    opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
                    opcode::ADDMOD | opcode::MULMOD => 3,
                    _ => 2,
                };
                (op, arity)
            }) {
                for input in [NumericValue::top(), cycle(), finite(capacity + 1)] {
                    let mut args = vec![AbstractValue::from_numeric(input)];
                    args.resize(arity, AbstractValue::constant(U256::from(7)));
                    let quote = domain.operation_work(&args);
                    cooperation::PROTOCOL_WORK.set((0, 0, 0));
                    let mut denied = WorkBudget::new(quote - 1);
                    assert!(domain.apply_budgeted(op, &args, &mut denied).is_err());
                    assert_eq!(denied.used(), 0);
                    assert!(denied.exhausted());
                    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
                    let mut paid = WorkBudget::new(quote);
                    let actual = domain.apply_budgeted(op, &args, &mut paid).unwrap();
                    assert_eq!(paid.used(), quote);
                    assert!(!paid.exhausted());
                    assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
                    let expected = domain.apply_detailed(op, &args);
                    assert_eq!(actual.value, expected.value);
                    assert_eq!(actual.status, expected.status);
                    assert_eq!(actual.rounds, expected.rounds);
                }
            }
        }
    }
}

#[test]
fn literal_and_top_binary_transfers_need_at_most_two_complete_cycles() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for capacity in [1, 2, 8, 255, 256, 512] {
        for rounds in [1, 2, 4, 8] {
            let domain = domain(capacity, rounds);
            for (literal, reverse) in [
                U256::ZERO,
                U256::ONE,
                U256::from(2),
                U256::from(3),
                U256::from(7),
                U256::from(255),
                U256::from(256),
                (U256::ONE << 255) - U256::ONE,
                U256::ONE << 255,
                (U256::ONE << 255) + U256::ONE,
                U256::MAX - U256::ONE,
                U256::MAX,
            ]
            .into_iter()
            .flat_map(|literal| [false, true].map(|reverse| (literal, reverse)))
            {
                let mut args = [NumericValue::constant(literal), NumericValue::top()];
                if reverse {
                    args.reverse();
                }
                for op in (0x01..=0x0b).chain(0x10..=0x1e).filter(|op| {
                    !matches!(
                        *op,
                        opcode::ISZERO
                            | opcode::NOT
                            | opcode::CLZ
                            | opcode::ADDMOD
                            | opcode::MULMOD
                    )
                }) {
                    let masks = Cell::new(0);
                    let bounds = Cell::new(0);
                    let algebra = ProductAlgebra::new(
                        domain.spec(),
                        CountMasks(&masks),
                        CountBounds(&bounds),
                    );
                    cooperation::PROTOCOL_WORK.set((0, 0, 0));
                    let result = algebra.apply_numeric(op, &args);
                    let quote = domain.numeric_operation_work(args.iter());
                    assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
                    assert!(
                        result.rounds <= 2 && masks.get() <= 2 && bounds.get() <= 2,
                        "op={op:02x} literal={literal} reverse={reverse} capacity={capacity}"
                    );
                    if rounds >= 2 {
                        assert_eq!(result.status, ReductionStatus::Stable, "op={op:02x}");
                    }
                }
            }
        }
    }
    let args = [NumericValue::constant(U256::ZERO), NumericValue::top()];
    assert_eq!(
        domain(8, 2).numeric_operation_work(args.iter()),
        domain(8, 8).numeric_operation_work(args.iter())
    );
}

#[test]
fn dyadic_fact_normalization_can_require_a_third_scalar_pass() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    let input = NumericValue {
        bits: KnownBits::new(U256::from(2), U256::from(69)).unwrap(),
        interval: Interval::new_unsigned(U256::from(93), U256::from(180)).unwrap(),
        congruence: Congruence::new(U256::from(2), U256::ONE).unwrap(),
        ..NumericValue::top()
    };
    let limited = domain(8, 2).reduce_numeric(&input);
    assert_eq!(limited.status, ReductionStatus::RoundLimit);
    assert_eq!(limited.rounds, 2);
    let domain = domain(8, 3);
    let result = domain.reduce_numeric(&input);
    assert_eq!(result.status, ReductionStatus::Stable);
    assert_eq!(result.rounds, 3);
    assert_eq!(
        result.value.constants().unwrap(),
        &[93, 101, 109, 117, 125]
            .into_iter()
            .map(U256::from)
            .collect()
    );
    assert!(cooperation::PROTOCOL_WORK.get().0 <= domain.numeric_refinement_work(&input, 0));
    let args = [input, NumericValue::constant(U256::ZERO)];
    let quote = domain.numeric_operation_work(args.iter());
    let result = domain.apply_numeric(opcode::ADD, &args);
    assert_eq!(result.status, ReductionStatus::Stable);
    assert_eq!(result.rounds, 3);
    assert!(cooperation::PROTOCOL_WORK.get().0 <= quote);
}

#[test]
fn quotes_saturate_and_cancellation_precedes_all_cooperative_work() {
    let _counter = cooperation::PROTOCOL_WORK.scope();
    for (capacity, rounds) in [(usize::MAX, 1), (8, usize::MAX)] {
        let domain = domain(capacity, rounds);
        assert_eq!(
            domain.numeric_operation_work([&NumericValue::top()].into_iter()),
            usize::MAX
        );
        assert_eq!(
            domain.numeric_refinement_work(&NumericValue::top(), usize::MAX),
            usize::MAX
        );
    }
    let domain = Domain::default();
    let args = [AbstractValue::top(), AbstractValue::constant(U256::from(7))];
    let token = analysis_control::Cancellation::new();
    let mut paid = analysis_control::with_cancellation(&token, || {
        WorkBudget::new(domain.operation_work(&args))
    });
    token.cancel();
    assert!(
        domain
            .apply_budgeted(opcode::ADD, &args, &mut paid)
            .is_err()
    );
    assert_eq!(paid.used(), 0);
    assert!(paid.exhausted());
    assert_eq!(cooperation::PROTOCOL_WORK.get(), (0, 0, 0));
}

#[test]
fn borrowed_default_finite_operations_remove_actual_materialization_and_restore_legacy_fees() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    for profile in [Profile::ConstantsOnly, Profile::Product] {
        let policy = Domain::from_spec(DomainSpec::new(
            profile,
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        ));
        let algebra = ValueAlgebra::new(policy);
        let domain = algebra.borrowed().without_relations();
        let args = [
            AbstractValue::constant(U256::ONE),
            AbstractValue::constant(U256::from(2)),
        ];
        let quote = domain.operation_work(&args);
        if profile == Profile::ConstantsOnly {
            assert_eq!(quote, 1);
        }
        let mut denied = WorkBudget::new(quote - 1);
        assert!(
            domain
                .apply_budgeted(opcode::ADD, &args, &mut denied)
                .is_err()
        );
        assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
        let mut paid = WorkBudget::new(quote);
        let result = domain
            .apply_budgeted(opcode::ADD, &args, &mut paid)
            .unwrap();
        assert_eq!(result.value.singleton(), Some(U256::from(3)));
        assert_eq!(paid.used(), quote);
        assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
        assert_eq!(
            result.value.provenance(),
            &crate::domain::provenance::Provenance::transfer(
                &args
                    .iter()
                    .map(|value| value.provenance().clone())
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(
            domain.observe_byte(opcode::BYTE, &args).singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
    }
    let policy = Domain::default();
    let args = [
        AbstractValue::from_numeric(cycle()),
        AbstractValue::constant(U256::from(7)),
    ];
    let mut paid = WorkBudget::new(policy.operation_work(&args));
    policy
        .apply_budgeted(opcode::ADD, &args, &mut paid)
        .unwrap();
    assert_eq!(
        crate::domain::algebra::OPERAND_COPIES.get(),
        2,
        "remaining default materialization is still executed and prepaid"
    );
}

#[test]
fn borrowed_numeric_entry_preserves_owned_transfer_results_and_limits() {
    for profile in [Profile::ConstantsOnly, Profile::Product] {
        for capacity in [1, 8, 512] {
            let policy = Domain::from_spec(DomainSpec::new(
                profile,
                NonZeroUsize::new(capacity).unwrap(),
                NonZeroUsize::new(4).unwrap(),
                NonZeroUsize::new(256).unwrap(),
            ));
            for value in [
                NumericValue::constant(U256::from(7)),
                finite(2),
                NumericValue {
                    finite: FiniteConstantSet::try_from_values(
                        [U256::ONE, U256::from(2)].into_iter().collect(),
                    )
                    .unwrap(),
                    ..NumericValue::constant(U256::from(2))
                },
                cycle(),
                NumericValue::top(),
            ] {
                for op in (0x01..=0x0b).chain(0x10..=0x1e) {
                    let arity = match op {
                        opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
                        opcode::ADDMOD | opcode::MULMOD => 3,
                        _ => 2,
                    };
                    let mut args = vec![value.clone()];
                    args.resize(arity, NumericValue::constant(U256::from(3)));
                    let owned = policy.apply_numeric(op, &args);
                    let borrowed = policy.apply_numeric_borrowed(op, args.iter());
                    assert_eq!(
                        borrowed.value, owned.value,
                        "op={op:02x} profile={profile:?}"
                    );
                    assert_eq!(borrowed.status, owned.status);
                    assert_eq!(borrowed.rounds, owned.rounds);
                    assert_eq!(borrowed.strengthened, owned.strengthened);
                }
            }
        }
    }
}

#[test]
fn borrowed_finite_transfer_preserves_stricter_singletons_and_source_metadata() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    let input = NumericValue {
        finite: FiniteConstantSet::try_from_values(
            [U256::ONE, U256::from(2)].into_iter().collect(),
        )
        .unwrap(),
        ..NumericValue::constant(U256::from(2))
    };
    let domain = ValueAlgebra::new(Domain::default());
    let args = [
        AbstractValue::from_numeric(input),
        AbstractValue::constant(U256::from(3)),
    ];
    let result = domain.apply(opcode::ADD, &args);
    assert_eq!(result.singleton(), Some(U256::from(5)));
    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 0);
    assert!(result.provenance().is_top());
}

#[derive(Debug, serde::Serialize)]
struct DeepWord {
    numeric: NumericValue,
    payload: Vec<u8>,
    #[serde(skip)]
    copies: Rc<Cell<usize>>,
}
impl Clone for DeepWord {
    fn clone(&self) -> Self {
        self.copies.set(self.copies.get() + 1);
        Self {
            numeric: self.numeric.clone(),
            payload: self.payload.clone(),
            copies: Rc::clone(&self.copies),
        }
    }
}
impl PartialEq for DeepWord {
    fn eq(&self, other: &Self) -> bool {
        self.numeric == other.numeric
    }
}
impl Eq for DeepWord {}
impl std::fmt::Display for DeepWord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.numeric.fmt(f)
    }
}
impl WordElement for DeepWord {
    fn top() -> Self {
        Self {
            numeric: NumericValue::top(),
            payload: vec![],
            copies: Rc::new(Cell::new(0)),
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            numeric: NumericValue::constant(value),
            ..Self::top()
        }
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self {
            numeric: NumericValue::from_observation(value),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        self.numeric.observation()
    }
    fn singleton(&self) -> Option<U256> {
        self.numeric.singleton()
    }
    fn is_top(&self) -> bool {
        self.numeric.is_top()
    }
    fn work_size(&self) -> usize {
        64 + self.payload.len() + self.numeric.work_size()
    }
}

#[derive(Debug)]
struct DeepPolicy {
    calls: Cell<usize>,
}
impl EvmNumericAlgebra for DeepPolicy {
    type Element = DeepWord;
    fn spec(&self) -> DomainSpec {
        DomainSpec::new(
            Profile::ConstantsOnly,
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(256).unwrap(),
        )
    }
    fn project_numeric(&self, value: &DeepWord) -> DeepWord {
        value.clone()
    }
    fn join_numeric(&self, _: &DeepWord, _: &DeepWord) -> DeepWord {
        DeepWord::top()
    }
    fn widen_numeric(&self, left: &DeepWord, right: &DeepWord) -> DeepWord {
        self.join_numeric(left, right)
    }
    fn widen_procedure_numeric(&self, left: &DeepWord, right: &DeepWord) -> DeepWord {
        self.join_numeric(left, right)
    }
    fn reduce_numeric(&self, value: &DeepWord) -> crate::domain::NumericReduction<DeepWord> {
        crate::domain::NumericReduction::stable(value.clone())
    }
    fn apply_numeric(
        &self,
        op: u8,
        args: &[DeepWord],
    ) -> crate::domain::NumericReduction<DeepWord> {
        self.calls.set(self.calls.get() + 1);
        let value = if op == opcode::ADD && args.len() == 2 {
            args[0]
                .singleton()
                .zip(args[1].singleton())
                .map(|(a, b)| a.wrapping_add(b))
                .map_or_else(DeepWord::top, DeepWord::constant)
        } else {
            DeepWord::top()
        };
        crate::domain::NumericReduction::stable(value)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a DeepWord>) -> usize {
        args.fold(1024usize, |work, value| {
            work.saturating_add(value.work_size())
        })
    }
    fn projection_work(&self, _: &DeepWord) -> usize {
        0
    }
    fn refinement_work(&self, value: &DeepWord, _: usize) -> usize {
        value.work_size() + 1024
    }
}

#[test]
fn backward_compatible_custom_materialization_remains_paid_before_deep_copies() {
    let _copies = crate::domain::algebra::OPERAND_COPIES.scope();
    let copies = Rc::new(Cell::new(0));
    let args = [4096, 2048].map(|size| {
        crate::domain::EvmValue::from_numeric(DeepWord {
            numeric: NumericValue::constant(U256::from(7)),
            payload: vec![9; size],
            copies: Rc::clone(&copies),
        })
    });
    let domain = ValueAlgebra::new(DeepPolicy {
        calls: Cell::new(0),
    });
    let view = domain.borrowed().without_relations();
    let quote = view.operation_work(&args);
    assert!(quote >= 2 * (4096 + 2048));
    assert_eq!(copies.get(), 0);
    let mut denied = WorkBudget::new(quote - 1);
    assert!(
        view.apply_budgeted(opcode::ADD, &args, &mut denied)
            .is_err()
    );
    assert_eq!(denied.used(), 0);
    assert_eq!(copies.get(), 0);
    assert_eq!(domain.numeric_algebra().calls.get(), 0);
    let mut paid = WorkBudget::new(quote);
    let result = view.apply_budgeted(opcode::ADD, &args, &mut paid).unwrap();
    assert_eq!(result.value.singleton(), Some(U256::from(14)));
    assert_eq!(paid.used(), quote);
    assert_eq!(copies.get(), 2);
    assert_eq!(crate::domain::algebra::OPERAND_COPIES.get(), 2);
    assert_eq!(domain.numeric_algebra().calls.get(), 1);
}
