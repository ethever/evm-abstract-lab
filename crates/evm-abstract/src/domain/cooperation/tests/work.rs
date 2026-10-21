use super::*;
use crate::{domain::WorkExhausted, resource::WorkBudget};
use analysis_kernel::{Accepts, Availability, Charges, KernelError, PublishError, Read, Scoped};

use super::super::{
    BoundedWork, Bounds, BoundsRoute, MaskRoute, Masks, RuleWork, ScalarRoute, Scalars,
    test_support::CancelDuringRule,
};

#[derive(Clone, Copy, Debug)]
struct Paid<'a, R> {
    rule: R,
    calls: &'a Cell<usize>,
}
impl<R: RuleWork> RuleWork for Paid<'_, R> {
    fn evaluation_work(&self, spec: DomainSpec, finite: usize) -> usize {
        self.rule
            .evaluation_work(spec, finite)
            .saturating_add(100_000)
    }
}
impl<Cx, R> Reduction<Cx> for Paid<'_, R>
where
    Cx: Charges<Error = KernelError<WorkExhausted>>,
    R: Reduction<Cx, Task = Scope, Error = RuleError>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, task: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.calls.set(self.calls.get() + 1);
        cx.charge(100_000)?;
        self.rule.reduce(task, cx)
    }
}

#[test]
fn expensive_custom_rules_use_their_explicit_work_contract_at_every_round_cap() {
    let _counter = super::super::PROTOCOL_WORK.scope();
    for rounds in [1, 4, 64, 64_000] {
        let spec = DomainSpec::new(
            Profile::Product,
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(rounds).unwrap(),
            NonZeroUsize::new(4096).unwrap(),
        );
        let calls = Cell::new(0);
        let custom = ProductAlgebra::new(
            spec,
            Paid {
                rule: KeepMasks,
                calls: &calls,
            },
            KeepBounds,
        );
        for input in [NumericValue::top(), NumericValue::constant(U256::from(7))] {
            calls.set(0);
            let quote = custom.refinement_work(&input, 0);
            assert_eq!(calls.get(), 0, "quotation must not run custom rules");
            let result = custom.reduce_numeric(&input);
            assert_eq!(result.status, ReductionStatus::Stable);
            assert_eq!(result.value, input);
            assert_eq!(calls.get(), 1);
            let used = super::super::PROTOCOL_WORK.get().0;
            assert!(used > 100_000 && used <= quote);
            assert!(
                quote > EvmNumericAlgebra::refinement_work(&Domain::from_spec(spec), &input, 0)
            );
        }
    }
}

#[test]
fn exact_custom_operation_quote_is_atomic_and_stays_on_the_original_root_budget() {
    let calls = Cell::new(0);
    let algebra = ValueAlgebra::new(ProductAlgebra::new(
        spec(8, 4096),
        Paid {
            rule: KeepMasks,
            calls: &calls,
        },
        KeepBounds,
    ));
    let args = [AbstractValue::top(), AbstractValue::constant(U256::from(7))];
    let quote = algebra.operation_work(&args);
    assert!(quote >= 200_000);
    assert_eq!(algebra.operation_work_for(opcode::ADD, &args), quote);
    assert_eq!(calls.get(), 0);
    let mut denied = WorkBudget::new(quote - 1);
    assert!(
        algebra
            .apply_budgeted(opcode::ADD, &args, &mut denied)
            .is_err()
    );
    assert_eq!(denied.used(), 0);
    assert!(denied.exhausted());
    assert_eq!(calls.get(), 0);
    let mut paid = WorkBudget::new(quote + 17);
    assert!(paid.charge(17));
    let result = algebra
        .apply_budgeted(opcode::ADD, &args, &mut paid)
        .unwrap();
    assert_eq!(result.status, ReductionStatus::Stable);
    assert!(result.value.numeric().is_top());
    assert_eq!(paid.used(), quote + 17);
    assert_eq!(calls.get(), 1);
    assert!(
        algebra
            .apply_budgeted(opcode::ADD, &args, &mut paid)
            .is_err()
    );
    assert_eq!(paid.used(), quote + 17);
    assert_eq!(calls.get(), 1, "a second call cannot reset the root budget");
}

#[test]
fn composed_refinement_pays_import_then_the_actual_custom_closure_once() {
    use crate::domain::{
        relational::{CheckResult, EvmScalarQuery, RelationLimits, RelationState},
        symbolic::ExprId,
    };

    let calls = Cell::new(0);
    let algebra = ValueAlgebra::new(ProductAlgebra::new(
        spec(8, 4096),
        Paid {
            rule: KeepMasks,
            calls: &calls,
        },
        KeepBounds,
    ));
    let domain = algebra.borrowed();
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let input = AbstractValue::top().with_expression(expression.clone());
    let mut relations = RelationState::default();
    assert_eq!(
        relations
            .assume_checked(&expression, true, &limits)
            .unwrap(),
        CheckResult::MayBeFeasible
    );
    let import = relations
        .refinement_import_work_with(&expression, &input, domain)
        .unwrap();
    let imported = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    let quote = import + domain.numeric_reduction_work(&imported);
    let mut denied = WorkBudget::new(import - 1);
    assert!(
        relations
            .refine_numeric_budgeted_with(&expression, &input, domain, &limits, &mut denied)
            .is_err()
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(denied.used(), 0);
    let mut denied_closure = WorkBudget::new(quote - 1);
    assert!(
        relations
            .refine_numeric_budgeted_with(&expression, &input, domain, &limits, &mut denied_closure)
            .is_err()
    );
    assert_eq!(
        denied_closure.used(),
        import,
        "failed closure admission retains the import charge"
    );
    assert_eq!(calls.get(), 0);
    let mut exact = WorkBudget::new(quote);
    let result = relations
        .refine_numeric_budgeted_with(&expression, &input, domain, &limits, &mut exact)
        .unwrap();
    assert!(
        matches!(result, EvmScalarQuery::Refined { numeric, .. } if !numeric.may_be_zero() && numeric.contains(U256::ONE))
    );
    assert!(calls.get() > 0);
    assert_eq!(
        exact.used(),
        quote,
        "import and closure must spend the same original root ledger"
    );
}

#[derive(Clone, Copy, Debug)]
struct PayloadFee<'a>(&'a Cell<usize>);
impl RuleWork for PayloadFee<'_> {
    fn evaluation_work(&self, _: DomainSpec, finite: usize) -> usize {
        finite
            .saturating_mul(100_000)
            .saturating_add(super::super::cost::query_work(finite.saturating_add(8)))
    }
}
impl<Cx> Reduction<Cx> for PayloadFee<'_>
where
    Cx: Read<Scalars, ScalarRoute, Error = KernelError<WorkExhausted>>
        + Charges<Error = KernelError<WorkExhausted>>,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        let answer = cx.query(Scoped::new(*scope, ()))?;
        if let Availability::Value(value) = answer.state {
            let fee = value.finite.work_size().saturating_mul(100_000);
            cx.charge(fee)?;
            self.0.set(self.0.get() + fee);
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn payload_dependent_rule_work_is_paid_after_import_and_before_rule_execution() {
    use crate::domain::{
        relational::{EvmScalarQuery, RelationLimits, RelationState},
        symbolic::ExprId,
    };
    let _counter = super::super::PROTOCOL_WORK.scope();
    for size in [8, 128] {
        let spent = Cell::new(0);
        let algebra = ValueAlgebra::new(ProductAlgebra::new(
            spec(4, 4096),
            PayloadFee(&spent),
            KeepBounds,
        ));
        let domain = algebra.borrowed();
        let expression = ExprId::fresh().unwrap();
        let input = AbstractValue::top().with_expression(expression.clone());
        let imported = NumericValue {
            finite: FiniteConstantSet::try_from_values((0..size).map(U256::from).collect())
                .unwrap(),
            ..NumericValue::top()
        };
        let limits = RelationLimits::default();
        let mut relations = RelationState::default();
        relations.observe(&expression, &imported, &limits).unwrap();
        let import = relations
            .refinement_import_work_with(&expression, &input, domain)
            .unwrap();
        let closure = domain.numeric_reduction_work(&imported);
        let total = import + closure;
        let mut denied = WorkBudget::new(total - 1);
        assert!(
            relations
                .refine_numeric_budgeted_with(&expression, &input, domain, &limits, &mut denied)
                .is_err()
        );
        assert_eq!(
            spent.get(),
            0,
            "the larger actual payload must be paid before the rule sees it"
        );
        assert_eq!(denied.used(), import);
        let mut exact = WorkBudget::new(total);
        let result = relations
            .refine_numeric_budgeted_with(&expression, &input, domain, &limits, &mut exact)
            .unwrap();
        assert!(
            matches!(result, EvmScalarQuery::Refined { ref numeric, .. } if (0..size).all(|word| numeric.contains(U256::from(word))))
        );
        assert!(spent.get() >= size * 100_000);
        assert!(
            spent.get() <= exact.used(),
            "a private meter cannot fund uncharged component work"
        );
        assert!(super::super::PROTOCOL_WORK.get().0 <= closure);
        assert_eq!(exact.used(), total);
    }
}

#[test]
fn exact_local_allowance_pays_reporting_and_one_less_reports_work_exhaustion() {
    let _counter = super::super::PROTOCOL_WORK.scope();
    let calls = Cell::new(0);
    let algebra = ProductAlgebra::new(
        spec(8, 4096),
        Paid {
            rule: KeepMasks,
            calls: &calls,
        },
        KeepBounds,
    );
    let input = NumericValue::top();
    assert_eq!(
        algebra
            .with_work(BoundedWork(200_000))
            .reduce_numeric(&input)
            .status,
        ReductionStatus::Stable
    );
    let needed = super::super::PROTOCOL_WORK.get().0;
    assert!(
        needed > 100_000,
        "setup, scalar and reporting work is also paid"
    );
    assert_eq!(
        algebra
            .with_work(BoundedWork(needed))
            .reduce_numeric(&input)
            .status,
        ReductionStatus::Stable
    );
    assert_eq!(super::super::PROTOCOL_WORK.get().0, needed);
    let result = algebra
        .with_work(BoundedWork(needed - 1))
        .reduce_numeric(&input);
    assert_eq!(result.status, ReductionStatus::WorkLimit);
    assert!(result.value.is_top());
    for work in [0, usize::MAX] {
        calls.set(0);
        assert_eq!(
            algebra
                .with_work(BoundedWork(work))
                .reduce_numeric(&input)
                .status,
            ReductionStatus::WorkLimit
        );
        assert_eq!(calls.get(), 0);
    }
    calls.set(0);
    let unquotable = ValueAlgebra::new(algebra.with_work(BoundedWork(usize::MAX)));
    let args = [AbstractValue::top(), AbstractValue::constant(U256::ONE)];
    assert_eq!(unquotable.operation_work(&args), usize::MAX);
    assert_eq!(unquotable.numeric_reduction_work(&input), usize::MAX);
    let mut root = WorkBudget::new(usize::MAX);
    assert!(
        unquotable
            .apply_budgeted(opcode::ADD, &args, &mut root)
            .is_err()
    );
    assert_eq!(root.used(), 0);
    assert_eq!(calls.get(), 0);
}

/// Independent bounded enumeration can infer non-prefix mask bits that the
/// built-in interval consumer misses when the complete cover exceeds capacity.
#[derive(Clone, Copy, Debug)]
struct CongruenceMasks;
impl RuleWork for CongruenceMasks {
    fn evaluation_work(&self, _: DomainSpec, finite: usize) -> usize {
        use super::super::cost::{publish_work, query_work};
        100_000usize
            .saturating_add(query_work(8))
            .saturating_add(query_work(finite.saturating_add(8)))
            .saturating_add(publish_work(16, 8))
    }
}
impl<Cx> Reduction<Cx> for CongruenceMasks
where
    Cx: Charges<Error = KernelError<WorkExhausted>>
        + Read<Bounds, BoundsRoute, Error = KernelError<WorkExhausted>>
        + Read<Scalars, ScalarRoute, Error = KernelError<WorkExhausted>>
        + Accepts<
            Masks,
            MaskRoute,
            Error = PublishError<crate::domain::facts::FactError, WorkExhausted>,
        >,
{
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, scope: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        cx.charge(100_000)?;
        let bounds = <Cx as Read<Bounds, BoundsRoute>>::query(cx, Scoped::new(*scope, ()))?;
        let scalars = <Cx as Read<Scalars, ScalarRoute>>::query(cx, Scoped::new(*scope, ()))?;
        if let (Availability::Value(bounds), Availability::Value(scalars)) =
            (bounds.state, scalars.state)
        {
            let (lo, hi) = bounds.unsigned_bounds();
            if let Ok(width @ 0..=32) = usize::try_from(hi - lo) {
                let values = (0..=width)
                    .map(|i| lo + U256::from(i))
                    .filter(|word| scalars.congruence.contains(*word))
                    .collect::<std::collections::BTreeSet<_>>();
                if !values.is_empty() {
                    cx.accept(Scoped::new(*scope, ()), KnownBits::from_values(&values))?;
                }
            }
        }
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn paid_custom_inference_adds_real_precision_with_an_independent_removal_control() {
    let custom = ProductAlgebra::new(spec(16, 4096), CongruenceMasks, KeepBounds);
    let removed = ProductAlgebra::new(spec(16, 4096), KeepMasks, KeepBounds);
    let input = NumericValue {
        interval: Interval::new_unsigned(U256::ZERO, U256::from(6)).unwrap(),
        congruence: Congruence::new(U256::from(5), U256::ZERO).unwrap(),
        ..NumericValue::top()
    };
    let full = custom.reduce_numeric(&input);
    let control = removed.reduce_numeric(&input);
    assert_eq!(full.status, ReductionStatus::Stable);
    assert_eq!(control.status, ReductionStatus::Stable);
    assert!(
        full.value.constants().is_none(),
        "capacity one must not truncate a two-word cover"
    );
    assert!(full.value.known_bits().zero().bit(1));
    assert!(!control.value.known_bits().zero().bit(1));
    for word in [U256::ZERO, U256::from(5)] {
        assert!(full.value.contains(word));
        assert!(control.value.contains(word));
    }
    let args = [
        AbstractValue::from_numeric(full.value),
        AbstractValue::constant(U256::from(2)),
    ];
    let algebra = ValueAlgebra::new(custom);
    let quote = algebra.operation_work(&args);
    let mut denied = WorkBudget::new(quote - 1);
    assert!(
        algebra
            .apply_budgeted(opcode::AND, &args, &mut denied)
            .is_err()
    );
    assert_eq!(denied.used(), 0);
    let mut exact = WorkBudget::new(quote);
    let output = algebra
        .apply_budgeted(opcode::AND, &args, &mut exact)
        .unwrap();
    assert_eq!(output.status, ReductionStatus::Stable);
    assert_eq!(output.value.singleton(), Some(U256::ZERO));
    assert_eq!(exact.used(), quote);
    let control = removed.apply_numeric(
        opcode::AND,
        &[control.value, NumericValue::constant(U256::from(2))],
    );
    assert!(control.value.contains(U256::ZERO));
    assert!(
        control.value.contains(U256::from(2)),
        "removing the custom rule must lose downstream precision"
    );
}

#[derive(Clone, Copy, Debug)]
struct SwallowRefusal;
impl<Cx: Charges<Error = KernelError<WorkExhausted>>> Reduction<Cx> for SwallowRefusal {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        let _ = cx.charge(100_000);
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn a_caught_component_refusal_cannot_turn_into_a_stable_closure() {
    let algebra = ProductAlgebra::new(spec(64, 4096), SwallowRefusal, KeepBounds)
        .with_work(BoundedWork(10_000));
    let result = algebra.reduce_numeric(&NumericValue::top());
    assert_eq!(result.status, ReductionStatus::WorkLimit);
    assert!(result.value.is_top());
}

#[test]
fn cancellation_is_captured_by_the_local_meter_and_keeps_the_root_identity() {
    for budgeted in [false, true] {
        let owner = analysis_control::Cancellation::new();
        let unrelated = analysis_control::Cancellation::new();
        let calls = Cell::new(0);
        let refused = Cell::new(false);
        let custom = ProductAlgebra::new(
            spec(8, 4096),
            CancelDuringRule {
                owner: &owner,
                unrelated: &unrelated,
                calls: &calls,
                refused: &refused,
            },
            KeepBounds,
        )
        .with_work(BoundedWork(200_000));
        if budgeted {
            let algebra = ValueAlgebra::new(custom);
            let args = [AbstractValue::top(), AbstractValue::constant(U256::from(7))];
            let quote = algebra.operation_work(&args);
            let mut root = analysis_control::with_cancellation(&owner, || WorkBudget::new(quote));
            // The call runs outside the root token's thread-local scope.
            assert!(
                algebra
                    .apply_budgeted(opcode::ADD, &args, &mut root)
                    .is_err()
            );
            assert_eq!(root.used(), quote, "admitted work is never refunded");
            assert!(root.exhausted());
        } else {
            let result = analysis_control::with_cancellation(&owner, || {
                custom.reduce_numeric(&NumericValue::top())
            });
            assert_eq!(result.status, ReductionStatus::Cancelled);
            assert!(result.value.is_top());
        }
        assert_eq!(calls.get(), 1);
        assert!(
            refused.get(),
            "a nested token cannot mask the original cancellation"
        );
        assert!(!unrelated.is_cancelled());
    }
}

#[derive(Clone, Copy, Debug)]
struct CancelThenFail<'a>(&'a analysis_control::Cancellation);
impl<Cx> Reduction<Cx> for CancelThenFail<'_> {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, _: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.0.cancel();
        Err(crate::domain::facts::FactError::UnsupportedOperation(0xfe).into())
    }
}

#[test]
fn a_component_error_cannot_mask_owner_cancellation() {
    let owner = analysis_control::Cancellation::new();
    let custom = ProductAlgebra::new(spec(8, 4096), CancelThenFail(&owner), KeepBounds)
        .with_work(BoundedWork(200_000));
    let result = analysis_control::with_cancellation(&owner, || custom.reduce_numeric(&input()));
    assert_eq!(result.status, ReductionStatus::Cancelled);
    for word in [3, 5, 7] {
        assert!(result.value.contains(U256::from(word)));
    }
}

#[derive(Clone, Copy, Debug)]
struct ProtocolFailure;
impl<Cx> Reduction<Cx> for ProtocolFailure {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, _: &mut Cx) -> Result<LocalClosure, RuleError> {
        Err(KernelError::WrongEpoch.into())
    }
}

#[test]
fn infrastructure_failure_is_typed_and_an_unavailable_relation_does_not_hide_work_refusal() {
    let failed = ProductAlgebra::new(spec(8, 4096), ProtocolFailure, KeepBounds)
        .with_work(BoundedWork(200_000))
        .reduce_numeric(&input());
    assert_eq!(failed.status, ReductionStatus::KernelFailure);
    for word in [3, 5, 7] {
        assert!(failed.value.contains(U256::from(word)));
    }
    let limited = ProductAlgebra::new(spec(8, 1), KeepMasks, KeepBounds)
        .with_work(BoundedWork(0))
        .apply_numeric(
            opcode::MUL,
            &[NumericValue::top(), NumericValue::constant(U256::ZERO)],
        );
    assert_eq!(limited.status, ReductionStatus::WorkLimit);
    assert!(limited.value.contains(U256::ZERO));
}

#[test]
fn work_and_round_caps_are_independent_for_built_in_rule_composition() {
    let calls = Cell::new(0);
    let custom = ProductAlgebra::new(
        spec(1, 4096),
        Paid {
            rule: BoundsToMask,
            calls: &calls,
        },
        MaskToBounds,
    )
    .with_work(BoundedWork(1_000_000));
    let result = custom.reduce_numeric(&input());
    assert_eq!(result.status, ReductionStatus::RoundLimit);
    for word in [3, 5, 7] {
        assert!(result.value.contains(U256::from(word)));
    }
    let custom = ProductAlgebra::new(
        spec(8, 4096),
        Paid {
            rule: BoundsToMask,
            calls: &calls,
        },
        MaskToBounds,
    )
    .with_work(BoundedWork(1_000_000));
    let result = custom.reduce_numeric(&input());
    assert_eq!(result.status, ReductionStatus::Stable);
    assert_eq!(
        result.value.interval().unsigned_bounds(),
        (U256::from(3), U256::from(7))
    );
    for word in [3, 5, 7] {
        assert!(result.value.contains(U256::from(word)));
    }
    let bounds = ProductAlgebra::new(
        spec(8, 4096),
        BoundsToMask,
        Paid {
            rule: MaskToBounds,
            calls: &calls,
        },
    )
    .with_work(BoundedWork(1_000_000));
    let bounds = bounds.reduce_numeric(&input());
    assert_eq!(bounds.status, ReductionStatus::Stable);
    assert_eq!(bounds.value, result.value);
}
