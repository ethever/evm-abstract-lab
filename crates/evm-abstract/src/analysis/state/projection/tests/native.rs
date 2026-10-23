//! Native copies are charged independently of the optional relation query.

use super::*;
use crate::{
    domain::{DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction},
    resource::clone_payment::tests::{Policy as Inner, RECEIPT, Word, private_value, reset},
};

#[derive(Debug)]
struct Algebra(Inner);
impl EvmNumericAlgebra for Algebra {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        self.0.project_numeric(value)
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        self.0.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        self.0.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.0.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        self.0.reduce_numeric(value)
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        self.0.apply_numeric(op, args)
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        self.0.operation_work(op, args)
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        self.0.literal_quote_work(literal)
    }
    fn projection_work(&self, value: &Word) -> usize {
        self.0.projection_work(value)
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        // This backend retains its owned predicate through each internal query
        // phase; its whole-operation envelope pays those native copies.
        self.0
            .refinement_work(value, facts)
            .saturating_add(value.clone_work().saturating_mul(128))
    }
}

fn native_fixture() -> (
    ValueAlgebra<Algebra>,
    RelationLimits,
    RelationState,
    EvmValue<Word>,
) {
    let algebra = ValueAlgebra::new(Algebra(Inner(ProductAlgebra::default())));
    let limits = algebra.borrowed().spec().relations();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let value = private_value(100_000)
        .with_expression(expression)
        .with_symbolic_limit();
    (algebra, limits, relations, value)
}

#[test]
fn branch_moves_the_owned_native_answer_and_summary_pays_cache_copies() {
    let _scope = RECEIPT.scope();
    let (algebra, limits, relations, value) = native_fixture();
    let expression = value.expression().unwrap();
    let mut independent = WorkBudget::new(usize::MAX);
    reset();
    relations
        .refine_numeric_budgeted(
            expression,
            &value,
            algebra.borrowed(),
            &limits,
            &mut independent,
        )
        .unwrap();
    let query_clones = RECEIPT.get().clones;
    assert!(query_clones > 0);
    let query_fee = expression.work_size() + relations.work_size() + independent.used();
    let copy_fee = value.numeric().clone_work();
    for summary in [false, true] {
        for hit_short in [false, true] {
            let mut actual = value.clone();
            reset();
            let maximum = query_fee
                + if summary {
                    3 * copy_fee - usize::from(hit_short)
                } else {
                    0
                };
            let mut budget = WorkBudget::new(maximum);
            let mut diagnostics = Vec::new();
            let policy = if summary {
                Policy::Summary
            } else {
                Policy::Branch {
                    diagnostics: &mut diagnostics,
                    pc: 9,
                }
            };
            let mut projection =
                Projection::new(&relations, algebra.borrowed(), &limits, &mut budget, policy);
            projection.value(&mut actual);
            assert_eq!(actual, value);
            assert_eq!(
                RECEIPT.get().clones,
                query_clones + usize::from(summary) * 2
            );
            if summary {
                // Both the original and already-refined bound reuse the same
                // native cache; every unavoidable hit copy is priced.
                projection.value(&mut actual);
                assert_eq!(
                    RECEIPT.get().clones,
                    query_clones + 2 + usize::from(!hit_short)
                );
            }
            assert_eq!(
                projection.finish(),
                if summary && hit_short {
                    Err(MachineError::Work)
                } else {
                    Ok(true)
                }
            );
            assert_eq!(actual, value);
            assert!(!actual.contains(U256::from(7)));
            assert!(actual.contains(U256::from(8)));
            assert!(diagnostics.is_empty());
        }
    }
}

#[test]
fn cache_copy_refusal_preserves_the_input_and_cancellation_identity() {
    let _scope = RECEIPT.scope();
    let (algebra, limits, relations, value) = native_fixture();
    let mut independent = WorkBudget::new(usize::MAX);
    relations
        .refine_numeric_budgeted(
            value.expression().unwrap(),
            &value,
            algebra.borrowed(),
            &limits,
            &mut independent,
        )
        .unwrap();
    let query_fee =
        value.expression().unwrap().work_size() + relations.work_size() + independent.used();
    let mut actual = value.clone();
    let mut budget = WorkBudget::new(query_fee);
    let mut projection = Projection::new(
        &relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    projection.value(&mut actual);
    assert!(projection.cache.is_empty());
    assert_eq!(projection.finish(), Err(MachineError::Work));
    assert_eq!(actual, value);

    let payer = analysis_control::Cancellation::new();
    let mut actual = value;
    let mut numeric = actual.numeric().clone();
    numeric.cancel_price = Some(payer.clone());
    actual.replace_numeric_after_singleton(numeric, None);
    let mut budget = analysis_control::with_cancellation(&payer, || WorkBudget::new(usize::MAX));
    let mut projection = Projection::new(
        &relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    reset();
    projection.value(&mut actual);
    assert_eq!(projection.outcome, Ok(true));
    reset();
    RECEIPT.update(|receipt| receipt.armed = true);
    projection.value(&mut actual);
    assert_eq!(
        projection.finish(),
        Err(MachineError::Relations(
            crate::domain::relational::QueryReason::Cancelled
        ))
    );
    assert!(payer.is_cancelled());
    assert_eq!(RECEIPT.get().clones, 0);
}

#[cfg(feature = "imbl")]
#[test]
fn shared_store_cow_is_paid_before_any_native_clone() {
    let _scope = RECEIPT.scope();
    let (world, entry, _, algebra) = crate::resource::clone_payment::tests::fixture("00");
    let domain = algebra.borrowed();
    let mut store = crate::world::EvmStore::new(&world, domain);
    let value = private_value(100_000).with_expression(ExprId::fresh().unwrap());
    store
        .write_checked(
            entry.address,
            &EvmValue::constant(U256::ZERO),
            &value,
            domain,
        )
        .unwrap();
    let saved = store.snapshot();
    let limits = domain.spec().relations();
    let relations = bounded_relations(&[ExprId::fresh().unwrap()], &limits);
    let mut quoted = WorkBudget::new(usize::MAX);
    store.pay_clone_with(&mut quoted).unwrap();
    let fee = quoted.used();
    for maximum in [fee - 1, usize::MAX] {
        let mut actual = store.clone();
        reset();
        let mut budget = WorkBudget::new(maximum);
        let mut diagnostics = Vec::new();
        let mut projection = Projection::new(
            &relations,
            domain,
            &limits,
            &mut budget,
            Policy::Branch {
                diagnostics: &mut diagnostics,
                pc: 0,
            },
        );
        projection.store(&mut actual);
        assert_eq!(
            projection.finish(),
            if maximum < fee {
                Err(MachineError::Work)
            } else {
                Ok(true)
            }
        );
        assert_eq!(RECEIPT.get().clones > 0, maximum >= fee);
        assert_eq!(&actual, saved.state());
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn empty_summary_preserves_native_values_and_checkpoints_without_materializing_copies() {
    let _scope = RECEIPT.scope();
    let (world, entry, _, algebra) = crate::resource::clone_payment::tests::fixture("00");
    let domain = algebra.borrowed();
    let mut input = crate::analysis::transfer::initial(&world, &entry, domain).unwrap();
    input
        .store
        .write_balance(entry.address, private_value(usize::MAX));
    input.active_mut().saved_store = input.store.snapshot();
    let saved = input.active().saved_store.clone();
    let original = input.clone();
    let relations = RelationState::default();
    let limits = domain.spec().relations();
    let mut budget = WorkBudget::new(0);
    reset();
    let mut projection = Projection::new(&relations, domain, &limits, &mut budget, Policy::Summary);
    projection.payload(&mut input).unwrap();
    assert_eq!(projection.finish(), Ok(true));
    assert_eq!(input, original);
    assert!(std::ptr::eq(
        input.active().saved_store.state(),
        saved.state()
    ));
    assert_eq!(RECEIPT.get().clones, 0);
    assert_eq!(RECEIPT.get().quotes, 0);
    assert_eq!(budget.used(), 0);
}
