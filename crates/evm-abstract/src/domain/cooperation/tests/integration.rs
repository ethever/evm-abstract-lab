//! Boundaries shared by selected rule work, root payment and scalar queries.

use super::*;
use crate::{
    domain::{
        WorkExhausted,
        cooperation::BoundedWork,
        relational::{CheckResult, EvmScalarQuery, QueryReason, RelationLimits, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use analysis_kernel::{Charges, KernelError};

#[derive(Clone, Copy, Debug)]
struct CancelRule<'a> {
    owner: &'a analysis_control::Cancellation,
    nested: &'a analysis_control::Cancellation,
    calls: &'a Cell<usize>,
    refused: &'a Cell<bool>,
}
impl<Cx: Charges<Error = KernelError<WorkExhausted>>> Reduction<Cx> for CancelRule<'_> {
    type Task = Scope;
    type Error = RuleError;
    fn reduce(&mut self, _: &Scope, cx: &mut Cx) -> Result<LocalClosure, RuleError> {
        self.calls.set(self.calls.get() + 1);
        cx.charge(100_000)?;
        self.owner.cancel();
        self.refused
            .set(analysis_control::with_cancellation(self.nested, || cx.charge(1)).is_err());
        Ok(LocalClosure::Stable)
    }
}

#[test]
fn refused_or_cancelled_root_cannot_publish_a_later_numeric_operation() {
    let algebra = ValueAlgebra::new(ProductAlgebra::new(spec(8, 4096), KeepMasks, KeepBounds));
    let args = [AbstractValue::top(), AbstractValue::constant(U256::ONE)];
    let quote = algebra.operation_work(&args);
    let mut refused = WorkBudget::new(quote);
    assert!(!refused.charge(quote + 1));
    assert!(
        algebra
            .apply_budgeted(opcode::ADD, &args, &mut refused)
            .is_err()
    );
    assert_eq!(refused.used(), 0);

    let current = analysis_control::Cancellation::new();
    let mut paid = WorkBudget::new(quote);
    current.cancel();
    analysis_control::with_cancellation(&current, || {
        assert!(
            algebra
                .apply_budgeted(opcode::ADD, &args, &mut paid)
                .is_err()
        );
    });
    assert_eq!(paid.used(), 0);
}

#[test]
fn cancellation_precedes_an_unquotable_local_closure() {
    let owner = analysis_control::Cancellation::new();
    let custom = ProductAlgebra::new(spec(8, 4096), KeepMasks, KeepBounds)
        .with_work(BoundedWork(usize::MAX));
    owner.cancel();
    let result = analysis_control::with_cancellation(&owner, || custom.reduce_numeric(&input()));
    assert_eq!(result.status, ReductionStatus::Cancelled);
    for word in [3, 5, 7] {
        assert!(result.value.contains(U256::from(word)));
    }
}

#[test]
fn scalar_cancellation_remains_a_stop_for_pure_and_captured_budget_callers() {
    for budgeted in [false, true] {
        let owner = analysis_control::Cancellation::new();
        let unrelated = analysis_control::Cancellation::new();
        let calls = Cell::new(0);
        let refused = Cell::new(false);
        let algebra = ValueAlgebra::new(
            ProductAlgebra::new(
                spec(8, 4096),
                CancelRule {
                    owner: &owner,
                    nested: &unrelated,
                    calls: &calls,
                    refused: &refused,
                },
                KeepBounds,
            )
            .with_work(BoundedWork(200_000)),
        );
        let limits = RelationLimits::default();
        let expression = ExprId::fresh().unwrap();
        let value = AbstractValue::top().with_expression(expression.clone());
        let mut relations = RelationState::default();
        assert_eq!(
            relations
                .assume_checked(&expression, true, &limits)
                .unwrap(),
            CheckResult::MayBeFeasible
        );
        if budgeted {
            let mut root =
                analysis_control::with_cancellation(&owner, || WorkBudget::new(usize::MAX));
            // The caller is now in a different scope from the ledger's owner.
            analysis_control::with_cancellation(&unrelated, || {
                assert!(
                    relations
                        .refine_numeric_budgeted_with(
                            &expression,
                            &value,
                            algebra.borrowed(),
                            &limits,
                            &mut root,
                        )
                        .is_err()
                );
            });
            assert!(root.exhausted());
            assert!(root.used() > 100_000, "admitted work stays paid");
        } else {
            let answer = analysis_control::with_cancellation(&owner, || {
                relations.refine_numeric_with(
                    &expression,
                    value.numeric(),
                    algebra.borrowed(),
                    &limits,
                )
            });
            assert_eq!(answer, EvmScalarQuery::Unknown(QueryReason::Cancelled));
        }
        assert_eq!(calls.get(), 1);
        assert!(refused.get(), "the local meter keeps the original owner");
        assert!(!unrelated.is_cancelled());
        assert!(value.numeric().is_top());
    }
}
