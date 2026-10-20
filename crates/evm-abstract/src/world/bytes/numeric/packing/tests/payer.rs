//! Pack receipts precede opaque work on both original world ledgers.

use super::*;
use crate::{
    domain::{NumericReduction, ValueAlgebra, symbolic::ExprError},
    resource::WorkBudget,
    world::bytes::{
        conditional::{self, BindingBudget, ConditionalFacts, Context},
        numeric::{Facts, Reader},
        word_plan,
    },
};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

const PRIOR: usize = 17;
const FEE: usize = 50_000;

mod query;

#[derive(Clone, Copy, Debug)]
enum Path {
    Prepared,
    Prepaid,
}
const PATHS: [Path; 2] = [Path::Prepared, Path::Prepaid];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hook {
    Delegate,
    Decline,
    CancelQuote,
    CancelPacking,
    CancelDecline,
}

// Neither Clone nor Copy. Every view must keep this exact policy and counters.
#[derive(Debug)]
struct Paid<D> {
    domain: D,
    fee: Option<usize>,
    hook: Hook,
    quotes: Cell<usize>,
    packs: Cell<usize>,
    actions: RefCell<Vec<u8>>,
}
impl<D> Paid<D> {
    fn new(domain: D, fee: Option<usize>, hook: Hook) -> Self {
        Self {
            domain,
            fee,
            hook,
            quotes: Cell::new(0),
            packs: Cell::new(0),
            actions: RefCell::new(Vec::new()),
        }
    }
}
impl<D: EvmNumericAlgebra> EvmNumericAlgebra for Paid<D> {
    type Element = D::Element;
    fn spec(&self) -> DomainSpec {
        self.domain.spec()
    }
    fn project_numeric(&self, value: &Self::Element) -> Self::Element {
        self.domain.project_numeric(value)
    }
    fn join_numeric(&self, left: &Self::Element, right: &Self::Element) -> Self::Element {
        self.domain.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        self.domain.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        self.domain.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Self::Element) -> NumericReduction<Self::Element> {
        self.domain.reduce_numeric(value)
    }
    fn apply_numeric(&self, op: u8, args: &[Self::Element]) -> NumericReduction<Self::Element> {
        self.actions.borrow_mut().push(op);
        self.domain.apply_numeric(op, args)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        self.domain.operation_work(args)
    }
    fn projection_work(&self, value: &Self::Element) -> usize {
        self.domain.projection_work(value)
    }
    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize {
        self.domain.refinement_work(value, facts)
    }
    fn pack_word_work(&self, _bytes: &[&Self::Element; 32]) -> Option<usize> {
        self.quotes.set(self.quotes.get() + 1);
        if self.hook == Hook::CancelQuote {
            analysis_control::current_cancellation().cancel();
        }
        self.fee
    }
    fn pack_word_numeric(&self, bytes: &[&Self::Element; 32]) -> Option<Self::Element> {
        self.packs.set(self.packs.get() + 1);
        if matches!(self.hook, Hook::CancelPacking | Hook::CancelDecline) {
            analysis_control::current_cancellation().cancel();
        }
        if matches!(self.hook, Hook::Decline | Hook::CancelDecline) {
            None
        } else {
            self.domain.pack_word_numeric(bytes)
        }
    }
}

fn default_domain() -> Domain {
    crate::world::bytes::numeric::numeric_domain(Domain::default())
}

fn context<D: EvmNumericAlgebra, T>(
    path: Path,
    maximum: usize,
    policy: &Paid<D>,
    action: impl FnOnce(&Context<'_, D::Element, Paid<D>>) -> T,
) -> (T, usize, bool) {
    let domain = ValueAlgebra::new(policy);
    let facts = Facts::new();
    let conditional = ConditionalFacts::default();
    let mut budget = WorkBudget::new(maximum);
    assert!(budget.charge(PRIOR));
    let value = match path {
        Path::Prepared => {
            let preparation = conditional::preparation(
                32,
                &EvmValue::<D::Element>::constant(U256::ZERO),
                0,
                0,
                &conditional,
                domain,
            );
            let context =
                Context::budgeted(&facts, &conditional, domain, &preparation, &mut budget)
                    .unwrap()
                    .unwrap();
            action(&context)
        }
        Path::Prepaid => {
            let payer = BindingBudget::new(&mut budget);
            let context = Context::prepaid(&facts, Some(&conditional), domain, &payer);
            action(&context)
        }
    };
    (value, budget.used(), budget.exhausted())
}

fn numeric_receipts<D: EvmNumericAlgebra>(
    path: Path,
    context: &Context<'_, D::Element, Paid<D>>,
) -> Vec<usize> {
    match path {
        Path::Prepared => context
            .demand_payments()
            .into_iter()
            .filter_map(|(numeric, fee)| numeric.then_some(fee))
            .collect(),
        Path::Prepaid => Vec::new(),
    }
}

#[test]
fn exact_pack_fee_and_receipt_minus_one_precede_the_hook_on_both_ledgers() {
    let bytes = vec![AbstractValue::constant(U256::from(7)); 32];
    let expected = original(&bytes, default_domain());
    let query_fee = expected.singleton_work();
    for path in PATHS {
        let policy = Paid::new(default_domain(), Some(FEE), Hook::Delegate);
        let (_, full, exhausted) = context(path, usize::MAX, &policy, |context| {
            assert_eq!(
                super::super::pack(&bytes, context.algebra(), context),
                Some(expected.clone())
            );
            if matches!(path, Path::Prepared) {
                assert_eq!(context.reader_calls(), (0, 0));
            }
        });
        assert!(!exhausted);
        let prefix = full - FEE - query_fee;
        for (available, hooks, receipts, complete) in [
            (prefix + FEE - 1, 0, vec![], false),
            (prefix + FEE, 1, vec![FEE], false),
            (full - 1, 1, vec![FEE], false),
            (full, 1, vec![FEE, query_fee], true),
        ] {
            let policy = Paid::new(default_domain(), Some(FEE), Hook::Delegate);
            let (_, used, exhausted) = context(path, available, &policy, |context| {
                let clone = context.clone();
                let before = context.demand_usage();
                let result = super::super::pack(&bytes, context.algebra(), context);
                assert_eq!(result, complete.then(|| expected.clone()));
                assert_eq!(
                    context.demand_usage(),
                    before,
                    "pack retains no context memo entry"
                );
                assert_eq!(policy.quotes.get(), 1);
                assert_eq!(policy.packs.get(), hooks);
                assert!(policy.actions.borrow().is_empty());
                if matches!(path, Path::Prepared) {
                    assert_eq!(numeric_receipts(path, context), receipts);
                }
                if complete {
                    assert!(!context.stopped());
                } else {
                    assert!(context.work_exhausted());
                    assert!(!context.cache_refused());
                    assert!(super::super::pack(&bytes, clone.algebra(), &clone).is_none());
                    assert_eq!(policy.quotes.get(), 1);
                }
            });
            assert_eq!(used, prefix + receipts.iter().sum::<usize>());
            assert_eq!(exhausted, !complete);
        }
    }
}

#[test]
fn no_quote_declines_and_paid_failed_predicate_keeps_its_fee_before_balanced_fallback() {
    let state = ByteStateId::exact(Arc::from([7u8; 32]));
    let offset = ExprId::constant(U256::ZERO);
    let bytes = vec![AbstractValue::constant(U256::from(7)); 32];
    let expected = original(&bytes, default_domain());
    for path in PATHS {
        let mut totals = Vec::new();
        for fee in [None, Some(FEE)] {
            let policy = Paid::new(default_domain(), fee, Hook::Decline);
            let (_, used, exhausted) = context(path, usize::MAX, &policy, |context| {
                let mut reader = Reader::with_context(context.clone(), context.algebra());
                assert_eq!(reader.word(&state, &offset), expected);
                assert!(!context.stopped());
                assert_eq!(policy.quotes.get(), 1);
                assert_eq!(policy.packs.get(), usize::from(fee.is_some()));
                assert_eq!(
                    &*policy.actions.borrow(),
                    &[vec![opcode::SHL; 32], vec![opcode::OR; 31]].concat()
                );
                if matches!(path, Path::Prepared) {
                    let receipts = numeric_receipts(path, context);
                    assert_eq!(receipts.len(), 63 + usize::from(fee.is_some()));
                    if fee.is_some() {
                        assert_eq!(receipts[0], FEE);
                    }
                    assert_eq!(context.reader_calls(), (63, 0));
                }
            });
            assert!(!exhausted);
            totals.push(used);
        }
        assert_eq!(
            totals[1] - totals[0],
            FEE,
            "failed certificate is never refunded"
        );
    }
}

#[test]
fn max_and_overflow_quotes_refuse_without_hooks_fallback_or_publication() {
    let state = ByteStateId::exact(Arc::from([7u8; 32]));
    for path in PATHS {
        for fee in [usize::MAX, (usize::MAX / 2 + 1).saturating_mul(2)] {
            let policy = Paid::new(default_domain(), Some(fee), Hook::Delegate);
            let (_, used, exhausted) = context(path, usize::MAX, &policy, |context| {
                let domain = context.algebra();
                assert_eq!(
                    word_plan::evaluate(
                        &state,
                        None,
                        &ExprId::constant(U256::ZERO),
                        context,
                        domain
                    ),
                    Ok(None)
                );
                assert!(context.work_exhausted());
                assert!(!context.cache_refused());
                assert_eq!(policy.quotes.get(), 1);
                assert_eq!(policy.packs.get(), 0);
                assert!(policy.actions.borrow().is_empty());
                assert!(numeric_receipts(path, context).is_empty());
                let before = context.demand_usage();
                let mut reader = Reader::with_context(context.clone(), domain);
                let caches = reader.cache_stats();
                assert_eq!(
                    reader.word(&state, &ExprId::constant(U256::ZERO)),
                    AbstractValue::top()
                );
                assert_eq!(reader.cache_stats(), caches);
                assert_eq!(context.demand_usage(), before);
                assert_eq!(policy.quotes.get(), 1);
            });
            assert!(used < FEE);
            assert!(exhausted);
        }
    }
}

#[test]
fn quote_and_hook_cancellation_leave_both_context_clones_stopped_without_publication() {
    let state = ByteStateId::exact(Arc::from([7u8; 32]));
    for path in PATHS {
        for (hook, fee) in [
            (Hook::CancelQuote, Some(FEE)),
            (Hook::CancelQuote, None),
            (Hook::CancelPacking, Some(FEE)),
            (Hook::CancelDecline, Some(FEE)),
        ] {
            let policy = Paid::new(default_domain(), fee, hook);
            let (_, _, exhausted) = context(path, usize::MAX, &policy, |context| {
                let token = analysis_control::Cancellation::new();
                let clone = context.clone();
                let domain = context.algebra();
                analysis_control::with_cancellation(&token, || {
                    assert_eq!(
                        word_plan::evaluate(
                            &state,
                            None,
                            &ExprId::constant(U256::ZERO),
                            context,
                            domain
                        ),
                        Err(ExprError::Cancelled.into())
                    );
                });
                assert_eq!(clone.check(), Err(ExprError::Cancelled));
                assert_eq!(policy.quotes.get(), 1);
                assert_eq!(
                    policy.packs.get(),
                    usize::from(matches!(hook, Hook::CancelPacking | Hook::CancelDecline))
                );
                assert!(policy.actions.borrow().is_empty());
                if matches!(path, Path::Prepared) {
                    assert_eq!(
                        numeric_receipts(path, context),
                        if matches!(hook, Hook::CancelPacking | Hook::CancelDecline) {
                            vec![FEE]
                        } else {
                            Vec::new()
                        }
                    );
                }
                let usage = context.demand_usage();
                assert_eq!(
                    word_plan::evaluate(
                        &state,
                        None,
                        &ExprId::constant(U256::ZERO),
                        &clone,
                        domain
                    ),
                    Err(ExprError::Cancelled.into())
                );
                assert_eq!(context.demand_usage(), usage);
                assert_eq!(policy.quotes.get(), 1);
            });
            assert!(
                !exhausted,
                "dynamic cancellation remains typed and sticky in the context"
            );
        }
    }
}

#[test]
fn already_cancelled_pack_never_quotes_or_executes_on_either_ledger() {
    for path in PATHS {
        let policy = Paid::new(default_domain(), Some(FEE), Hook::Delegate);
        context(path, usize::MAX, &policy, |context| {
            let token = analysis_control::Cancellation::new();
            token.cancel();
            let bytes = vec![AbstractValue::constant(U256::from(7)); 32];
            analysis_control::with_cancellation(&token, || {
                assert!(super::super::pack(&bytes, context.algebra(), context).is_none());
            });
            assert_eq!(context.check(), Err(ExprError::Cancelled));
            assert_eq!(policy.quotes.get(), 0);
            assert_eq!(policy.packs.get(), 0);
            assert!(numeric_receipts(path, context).is_empty());
        });
    }
}

#[test]
fn max_quote_cannot_authorize_a_hook_even_on_an_empty_prepaid_ledger() {
    let policy = Paid::new(default_domain(), Some(usize::MAX), Hook::Delegate);
    let domain = ValueAlgebra::new(&policy);
    let facts = Facts::new();
    let bytes = vec![AbstractValue::constant(U256::from(7)); 32];
    let mut budget = WorkBudget::new(usize::MAX);
    {
        let payer = BindingBudget::new(&mut budget);
        let context = Context::prepaid(&facts, None, domain, &payer);
        assert!(super::super::pack(&bytes, domain, &context).is_none());
        assert!(context.work_exhausted());
        assert!(super::super::pack(&bytes, domain, &context).is_none());
    }
    assert_eq!(policy.quotes.get(), 1);
    assert_eq!(policy.packs.get(), 0);
    assert_eq!(budget.used(), 0);
}

#[test]
fn prepared_payment_cancellation_retains_receipt_and_never_enters_pack_or_fallback() {
    let policy = Paid::new(default_domain(), Some(FEE), Hook::Delegate);
    context(Path::Prepared, usize::MAX, &policy, |context| {
        let token = analysis_control::Cancellation::new();
        context.cancel_after_numeric_payments(1, token.clone());
        analysis_control::with_cancellation(&token, || {
            let bytes = vec![AbstractValue::constant(U256::from(7)); 32];
            assert!(super::super::pack(&bytes, context.algebra(), context).is_none());
        });
        assert_eq!(context.check(), Err(ExprError::Cancelled));
        assert_eq!(numeric_receipts(Path::Prepared, context), [FEE]);
        assert_eq!(policy.packs.get(), 0);
        assert!(policy.actions.borrow().is_empty());
    });
}

#[test]
fn opaque_custom_carrier_is_borrowed_and_no_quote_or_refusal_never_enters_its_hook() {
    use crate::world::bytes::carrier_tests::{HEAVY_CLONES, HeavyWords, heavy_value};
    let _scope = HEAVY_CLONES.scope();
    let bytes: [_; 32] = std::array::from_fn(|_| heavy_value(64 * 1024));
    for path in PATHS {
        for fee in [None, Some(usize::MAX), Some(FEE)] {
            let policy = Paid::new(HeavyWords::default(), fee, Hook::Delegate);
            HEAVY_CLONES.replace(0);
            context(path, usize::MAX, &policy, |context| {
                assert!(super::super::pack(&bytes, context.algebra(), context).is_none());
                assert_eq!(policy.quotes.get(), 1);
                assert_eq!(policy.packs.get(), usize::from(fee == Some(FEE)));
                assert_eq!(policy.domain.packs.get(), policy.packs.get());
                assert_eq!(HEAVY_CLONES.get(), 0, "packing borrowed every original N");
            });
        }
    }
}
