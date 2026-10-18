//! Real prepared payer traces; original receipt fixtures remain untouched.

use super::super::{Context, Facts, Preparation, preparation};
use super::*;
use crate::{
    domain::{
        DomainSpec, NumericValue, identity::Symbol, provenance::Origin, relational::RelationLimits,
        symbolic::ExprError,
    },
    resource::WorkBudget,
    world::bytes::conditional::ConditionalFacts,
};
use alloy_primitives::U256;
use std::{cell::Cell, num::NonZeroUsize, rc::Rc};

fn domain(capacity: usize) -> Domain {
    Domain::from_spec(
        DomainSpec::new(
            Profile::Product,
            NonZeroUsize::new(capacity).unwrap(),
            NonZeroUsize::new(16).unwrap(),
            NonZeroUsize::new(1024).unwrap(),
        )
        .with_relations(RelationLimits {
            enabled: false,
            ..RelationLimits::default()
        }),
    )
}

fn constant(value: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn range(lower: usize, upper: usize) -> AbstractValue {
    AbstractValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap()
}

fn fixture(domain: Domain) -> (Facts, ConditionalFacts, Preparation) {
    let facts = Facts::new();
    let conditional = ConditionalFacts::default();
    let quote = preparation(1, &constant(0), 0, 0, &conditional, domain);
    (facts, conditional, quote)
}

fn total(trace: &[(bool, usize)]) -> usize {
    trace.iter().map(|(_, work)| work).sum()
}

fn numeric(trace: &[(bool, usize)]) -> Vec<usize> {
    trace
        .iter()
        .filter(|(kind, _)| *kind)
        .map(|(_, work)| *work)
        .collect()
}

fn finite(values: impl IntoIterator<Item = usize>, capacity: usize) -> AbstractValue {
    let constants = Domain::new(NonZeroUsize::new(capacity).unwrap());
    values
        .into_iter()
        .map(constant)
        .reduce(|a, b| constants.join(&a, &b))
        .unwrap()
}

fn compact_byte() -> (AbstractValue, AbstractValue) {
    let dense = finite(0..256, 512);
    let mut compact = dense
        .clone()
        .with_numeric(dense.numeric().compact_for_retention());
    compact.set_origin(Origin::Calldata);
    compact = compact
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(871_023))
        .with_identity(871_024, 1)
        .with_symbolic_limit();
    let expanded = compact.clone().with_numeric(dense.numeric().clone());
    assert!(compact.constants().is_none());
    (compact, expanded)
}

fn trace(domain: Domain, op: u8, args: &[AbstractValue]) -> (Vec<(bool, usize)>, AbstractValue) {
    let (facts, conditional, quote) = fixture(domain);
    let mut budget = WorkBudget::new(usize::MAX);
    let (payments, value);
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        value = context.apply(domain, op, args);
        assert!(!context.stopped());
        assert_eq!(context.reader_calls(), (1, 0));
        payments = context.demand_payments();
    }
    assert_eq!(budget.used(), total(&payments));
    (payments, value)
}

fn assert_metadata(actual: &AbstractValue, original: &AbstractValue, runtime: &AbstractValue) {
    assert_eq!(actual.known_bits(), original.known_bits());
    assert_eq!(actual.interval(), original.interval());
    assert_eq!(actual.congruence(), original.congruence());
    assert_eq!(actual.may_be_zero(), original.may_be_zero());
    assert_eq!(actual.provenance(), original.provenance());
    assert_eq!(actual.expression(), original.expression());
    assert_eq!(actual.identity(), original.identity());
    assert_eq!(
        actual.symbolic_limit_reached(),
        original.symbolic_limit_reached()
    );
    assert!(runtime.identity().input().is_none());
    assert!(actual.same_identity(runtime));
    assert!(original.same_identity(runtime));
}

#[test]
fn compact_byte_and_literal_use_the_existing_finite_apply_and_actual_operand_quote() {
    let domain = domain(512);
    let (compact, expanded) = compact_byte();
    let args = [compact, constant(255)];
    let frozen = args.clone();
    let finite_args = [expanded, constant(255)];
    let expected = domain.apply(opcode::AND, &finite_args);
    let (payments, actual) = trace(domain, opcode::AND, &args);
    assert_eq!(actual, expected);
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.expression(), expected.expression());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(
        actual.symbolic_limit_reached(),
        expected.symbolic_limit_reached()
    );
    assert_eq!(actual.constants().unwrap().len(), 256);
    let plan = args[0]
        .numeric()
        .complete_enumeration(domain.capacity())
        .unwrap();
    assert_eq!(
        numeric(&payments),
        [
            256 * args.len(),
            plan.work(),
            domain.operation_work(&finite_args)
        ]
    );
    assert!(domain.operation_work(&finite_args) < domain.operation_work(&args));
    assert_eq!(args, frozen);
    assert!(args[0].same_identity(&AbstractValue::top().with_identity(871_024, 1)));
}

#[test]
fn temporary_inputs_keep_all_metadata_even_for_singletons_and_never_enter_a_root_cache() {
    for relations in [false, true] {
        let domain = Domain::from_spec(domain(8).spec().with_relations(RelationLimits {
            enabled: relations,
            ..RelationLimits::default()
        }));
        let (facts, conditional, quote) = fixture(domain);
        let frozen = (facts.clone(), conditional.clone());
        let runtime = AbstractValue::top().with_identity(871_025, 1);
        for (lower, upper) in [(2, 4), (5, 5)] {
            let mut original = range(0, 7)
                .with_symbol(Symbol::CalldataWord(U256::from(32)), Some(871_026))
                .with_identity(871_025, 1)
                .with_numeric(
                    NumericValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap(),
                )
                .with_symbolic_limit();
            original.set_origin(Origin::Storage);
            let existing = constant(9).with_symbolic_limit();
            let args = [original.clone(), existing.clone()];
            let mut budget = WorkBudget::new(usize::MAX);
            let receipt;
            {
                let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                    .unwrap()
                    .unwrap();
                let memo = Rc::clone(&root.memo);
                let demand = root.demand.as_deref().unwrap();
                let before = root.demand_payments().len();
                let temporary =
                    prepare(domain, opcode::ADD, &args, demand, || !root.stopped()).unwrap();
                assert_metadata(&temporary[0], &original, &runtime);
                assert_eq!(temporary[0].constants().unwrap().len(), upper - lower + 1);
                assert_eq!(
                    temporary[1], existing,
                    "already finite operands stay verbatim"
                );
                assert!(temporary[1].symbolic_limit_reached());
                let first = root.demand_payments()[before..].to_vec();
                let again =
                    prepare(domain, opcode::ADD, &args, demand, || !root.stopped()).unwrap();
                assert_eq!(again, temporary);
                assert!(!std::ptr::eq(
                    again[0].constants().unwrap(),
                    temporary[0].constants().unwrap()
                ));
                assert_eq!(root.demand_payments()[before + first.len()..], first);
                assert!(Rc::ptr_eq(&memo, &root.memo));
                assert!(root.memo.borrow().branches.is_empty());
                assert_eq!(root.reader_calls(), (0, 0));
                assert_eq!(args, [original.clone(), existing]);
                assert_metadata(&args[0], &original, &runtime);
                let count = upper - lower + 1;
                let scratch = args.len()
                    + args.iter().map(AbstractValue::work_size).sum::<usize>()
                    + count
                    + 1;
                assert_eq!(first[1], (false, 2 * scratch));
                assert_eq!(demand.used(), 1 + 2 * scratch);
                receipt = demand.paid_work();
                let payments = demand.payments();
                drop(temporary);
                drop(again);
                assert_eq!(demand.paid_work(), receipt, "release is fully prepaid");
                assert_eq!(demand.payments(), payments);
            }
            assert_eq!(budget.used(), receipt);
        }
        assert_eq!((facts, conditional), frozen);
    }
}

#[test]
fn capacity_tuple_wide_and_span_overflow_fallbacks_pay_planning_then_one_original_action() {
    let domain = domain(8);
    for args in [
        [range(0, 8), constant(1)],
        [range(0, 2), range(0, 2)],
        [AbstractValue::top(), constant(1)],
        [
            AbstractValue::unsigned_range(U256::ZERO, U256::MAX - U256::from(1)).unwrap(),
            constant(1),
        ],
        [finite(0..9, 9), range(0, 0)],
    ] {
        let expected = domain.apply(opcode::ADD, &args);
        let (payments, actual) = trace(domain, opcode::ADD, &args);
        assert_eq!(actual, expected);
        assert_eq!(
            numeric(&payments),
            [args.len() * 256, domain.operation_work(&args)]
        );
        assert_eq!(
            payments.iter().filter(|(kind, _)| !kind).count(),
            2,
            "rejected plans allocate no scratch"
        );
    }
    let args = [range(0, 1), range(0, 1), range(0, 1)];
    let (payments, actual) = trace(domain, opcode::ADDMOD, &args);
    let finite_args = [finite(0..2, 8), finite(0..2, 8), finite(0..2, 8)];
    assert_eq!(actual, domain.apply(opcode::ADDMOD, &finite_args));
    assert_eq!(
        numeric(&payments).len(),
        3,
        "capacity equality and three operands are admitted"
    );
}

#[test]
fn checked_tuple_overflow_is_a_paid_plan_rejection_without_building_large_sets() {
    let domain = domain(usize::MAX);
    let (facts, conditional, quote) = fixture(domain);
    let args = [range(0, usize::MAX - 1), range(0, 1)];
    let mut budget = WorkBudget::new(usize::MAX);
    let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
        .unwrap()
        .unwrap();
    let before = root.demand_payments().len();
    let mut checkpoints = 0;
    assert!(
        prepare(
            domain,
            opcode::ADD,
            &args,
            root.demand.as_deref().unwrap(),
            || {
                checkpoints += 1;
                !root.stopped()
            }
        )
        .is_none()
    );
    assert_eq!(checkpoints, 0);
    assert_eq!(root.demand_payments()[before..], [(true, 512)]);
    assert_eq!(root.demand_usage(), Some((1, 1)));
    assert!(!root.stopped());
    // Deliberately do not call Domain::apply with this enormous test policy.
}

#[test]
fn unprepared_oracle_all_finite_profile_and_unsupported_shapes_keep_their_old_routes() {
    let product = domain(8);
    let constants = Domain::new(NonZeroUsize::new(8).unwrap());
    for (domain, op, args) in [
        (product, opcode::ADD, vec![constant(1), constant(2)]),
        (constants, opcode::ADD, vec![range(0, 2), constant(1)]),
        (product, opcode::STOP, vec![range(0, 2), constant(1)]),
        (product, opcode::ADD, vec![range(0, 2); 4]),
        (product, opcode::ADD, vec![]),
        (product, opcode::NOT, vec![range(0, 2), constant(1)]),
    ] {
        let (facts, conditional, quote) = fixture(domain);
        let expected = domain.apply(op, &args);
        let unprepared = Context::new(&facts, Some(&conditional));
        assert_eq!(unprepared.apply(domain, op, &args), expected);
        let oracle = Context::prepared(&facts, &conditional, domain, quote.nodes);
        let before = oracle.demand_payments();
        assert_eq!(oracle.apply(domain, op, &args), expected);
        assert_eq!(oracle.demand_payments(), before);
        let (payments, actual) = trace(domain, op, &args);
        assert_eq!(actual, expected);
        assert_eq!(numeric(&payments), [domain.operation_work(&args)]);
    }
    // The same eligible small current interval still uses the original Oracle
    // algorithm and original receipt, rather than an unmetered new algorithm.
    let (facts, conditional, quote) = fixture(product);
    let args = [range(0, 2), constant(1)];
    let oracle = Context::prepared(&facts, &conditional, product, quote.nodes);
    let before = oracle.demand_payments();
    assert_eq!(
        Context::new(&facts, Some(&conditional)).apply(product, opcode::ADD, &args),
        product.apply(opcode::ADD, &args)
    );
    assert_eq!(
        oracle.apply(product, opcode::ADD, &args),
        product.apply(opcode::ADD, &args)
    );
    assert_eq!(oracle.demand_payments(), before);
}

#[test]
fn full_policy_mismatch_and_scratch_refusal_retain_the_existing_private_refusal_handling() {
    let domain = domain(8);
    let (facts, conditional, mut quote) = fixture(domain);
    let args = [range(0, 2), constant(1)];
    let mismatched = Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(17).unwrap(),
        NonZeroUsize::new(1024).unwrap(),
    ));
    let mut budget = WorkBudget::new(usize::MAX);
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let before = root.demand_payments();
        assert_eq!(
            root.apply(mismatched, opcode::ADD, &args),
            AbstractValue::top()
        );
        assert!(root.cache_refused());
        assert!(!root.work_exhausted());
        assert_eq!(root.demand_payments(), before);
    }
    quote.nodes = 1; // The existing initial empty-source reservation exactly fits.
    quote.work = quote.nodes;
    let mut budget = WorkBudget::new(usize::MAX);
    let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
        .unwrap()
        .unwrap();
    let before = root.demand_payments().len();
    let checkpoints = Cell::new(0);
    assert!(
        prepare(
            domain,
            opcode::ADD,
            &args,
            root.demand.as_deref().unwrap(),
            || {
                checkpoints.set(checkpoints.get() + 1);
                !root.stopped()
            }
        )
        .is_none()
    );
    assert_eq!(checkpoints.get(), 0);
    assert!(root.cache_refused());
    assert!(!root.work_exhausted());
    assert_eq!(root.demand_usage(), Some((1, 1)));
    assert_eq!(root.demand_payments()[before..], [(true, 512)]);
}

#[test]
fn trace_derived_exact_and_one_less_budgets_stop_every_unfunded_stage_across_clones() {
    let domain = domain(8);
    let args = [range(0, 2), constant(1)];
    let (full, expected) = trace(domain, opcode::ADD, &args);
    let stages: Vec<_> = full
        .iter()
        .enumerate()
        .skip(2)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        stages.len(),
        4,
        "planning, lifetime, construction, original operation"
    );
    for position in stages {
        let prefix = total(&full[..position]);
        let fee = full[position].1;
        for exact in [false, true] {
            let available = prefix + fee - usize::from(!exact);
            let (facts, conditional, quote) = fixture(domain);
            let mut budget = WorkBudget::new(available);
            {
                let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                    .unwrap()
                    .unwrap();
                let clone = root.clone();
                let actual = root.apply(domain, opcode::ADD, &args);
                let accepted = position + usize::from(exact);
                assert_eq!(root.demand_payments(), full[..accepted]);
                if accepted == full.len() {
                    assert_eq!(actual, expected);
                    assert_eq!(root.reader_calls(), (1, 0));
                    assert!(!root.stopped());
                } else {
                    assert_eq!(actual, AbstractValue::top());
                    assert!(root.work_exhausted());
                    assert!(!root.cache_refused());
                    assert_eq!(root.reader_calls(), (0, 0));
                    let usage = root.demand_usage();
                    let paid = root.demand_payments();
                    assert_eq!(
                        clone.apply(domain, opcode::ADD, &args),
                        AbstractValue::top()
                    );
                    assert_eq!(
                        clone.join(domain, &constant(1), &constant(2)),
                        AbstractValue::top()
                    );
                    assert_eq!(root.demand_usage(), usage);
                    assert_eq!(root.demand_payments(), paid);
                    assert!(root.memo.borrow().branches.is_empty());
                }
            }
            assert_eq!(budget.used(), total(&full[..position + usize::from(exact)]));
            assert_eq!(
                budget.exhausted(),
                position + usize::from(exact) < full.len()
            );
        }
    }
}

#[test]
fn preparation_construction_and_preapply_cancellation_are_sticky_and_discard_the_read() {
    let domain = domain(8);
    let args = [range(0, 2), constant(1)];
    let (full, _) = trace(domain, opcode::ADD, &args);
    for stage in [1, 2, 3] {
        let (facts, conditional, quote) = fixture(domain);
        let token = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(usize::MAX);
        let paid;
        {
            let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .unwrap()
                .unwrap();
            let clone = root.clone();
            root.cancel_after_numeric_payments(stage, token.clone());
            analysis_control::with_cancellation(&token, || {
                assert_eq!(root.apply(domain, opcode::ADD, &args), AbstractValue::top());
                assert_eq!(root.check(), Err(ExprError::Cancelled));
            });
            assert!(!analysis_control::current_cancellation().is_cancelled());
            assert_eq!(root.reader_calls(), (0, 0));
            let position = full
                .iter()
                .enumerate()
                .filter(|(_, (kind, _))| *kind)
                .nth(stage - 1)
                .unwrap()
                .0;
            paid = root.demand_payments();
            assert_eq!(paid, full[..=position]);
            let usage = root.demand_usage();
            for context in [&root, &clone] {
                assert_eq!(context.check(), Err(ExprError::Cancelled));
                assert!(!context.work_exhausted());
                assert!(!context.cache_refused());
                assert_eq!(
                    context.apply(domain, opcode::ADD, &args),
                    AbstractValue::top()
                );
                assert_eq!(context.demand_payments(), paid);
                assert_eq!(context.demand_usage(), usage);
            }
        }
        assert_eq!(budget.used(), total(&paid));
        assert!(!budget.exhausted());
    }
}

#[test]
fn cancellation_inside_paid_candidate_construction_drops_partial_sets_and_latches_all_clones() {
    let domain = domain(8);
    let args = [range(0, 7), constant(1)];
    let frozen = args.clone();
    let (facts, conditional, quote) = fixture(domain);
    let token = analysis_control::Cancellation::new();
    let mut budget = WorkBudget::new(usize::MAX);
    let paid;
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let clone = root.clone();
        let checkpoints = Cell::new(0);
        analysis_control::with_cancellation(&token, || {
            let result = prepare(
                domain,
                opcode::ADD,
                &args,
                root.demand.as_deref().unwrap(),
                || {
                    checkpoints.set(checkpoints.get() + 1);
                    // Check 1 precedes the Vec allocation, checks 2/3 visit the
                    // first candidates; interrupt after a partial tree exists.
                    if checkpoints.get() == 4 {
                        token.cancel();
                    }
                    !root.stopped()
                },
            );
            assert!(result.is_none());
            assert_eq!(root.check(), Err(ExprError::Cancelled));
        });
        assert_eq!(checkpoints.get(), 4);
        assert_eq!(root.reader_calls(), (0, 0));
        assert_eq!(numeric(&root.demand_payments()).len(), 2);
        let usage = root.demand_usage();
        paid = root.demand_payments();
        for context in [&root, &clone] {
            assert_eq!(context.check(), Err(ExprError::Cancelled));
            assert_eq!(
                context.apply(domain, opcode::ADD, &args),
                AbstractValue::top()
            );
            assert!(!context.work_exhausted());
            assert!(!context.cache_refused());
            assert_eq!(context.demand_payments(), paid);
            assert_eq!(context.demand_usage(), usage);
        }
        assert_eq!(args, frozen);
        assert!(args[0].constants().is_none());
    }
    assert_eq!(budget.used(), total(&paid));
    assert!(!budget.exhausted());
}
