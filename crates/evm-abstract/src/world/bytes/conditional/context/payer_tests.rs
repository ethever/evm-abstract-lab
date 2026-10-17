use super::{Context, Facts, Preparation, memo, preparation};
use crate::{
    domain::{
        AbstractValue, Domain,
        identity::Symbol,
        provenance::Origin,
        relational::RelationLimits,
        symbolic::{ExprError, ExprId},
    },
    resource::WorkBudget,
    world::bytes::conditional::{Branch, ConditionalFacts, Scope},
};
use alloy_primitives::U256;
use std::sync::Arc;

fn constant(value: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn range(lower: usize, upper: usize) -> AbstractValue {
    AbstractValue::unsigned_range(U256::from(lower), U256::from(upper)).unwrap()
}

fn source(mut value: AbstractValue, origin: Origin) -> AbstractValue {
    value.set_origin(origin);
    value
}

fn branches(guard: &ExprId, yes: Facts, no: Facts) -> ConditionalFacts {
    ConditionalFacts::from_branches(vec![Arc::new(Branch {
        condition: guard.clone(),
        then_facts: Scope {
            facts: yes,
            conditional: ConditionalFacts::default(),
        },
        else_facts: Scope {
            facts: no,
            conditional: ConditionalFacts::default(),
        },
    })])
}

fn preparation_for(facts: &Facts, conditional: &ConditionalFacts, domain: Domain) -> Preparation {
    let work = facts.iter().fold(0usize, |work, (key, value)| {
        work.saturating_add(key.work_size())
            .saturating_add(value.work_size())
    });
    preparation(72, &constant(0), work, facts.len(), conditional, domain)
}

#[test]
fn actual_scope_meet_and_join_keep_the_complete_value_and_memo_receipt() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let left = source(range(0, 20), Origin::Calldata)
        .with_origin(Origin::Environment)
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(993_117))
        .with_expression(key.clone());
    let right = source(range(5, 25), Origin::Storage)
        .with_origin(Origin::Memory)
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(993_117))
        .with_expression(key.clone());
    let facts = Facts::from([(key.clone(), left.clone())]);
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), right.clone())]),
        Facts::new(),
    );
    let frozen = (facts.clone(), conditional.clone());
    let quote = preparation_for(&facts, &conditional, domain);
    let price = quote.reduction_work + memo::join_work(domain, &left, &right);
    let expected = Context::new(&facts, Some(&conditional))
        .branch(&guard, true)
        .value(&key, domain)
        .unwrap();
    let mut budget = WorkBudget::new(price + 37);
    assert!(budget.charge(37));
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget);
        let selected = root.branch(&guard, true);
        let actual = selected.value(&key, domain).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.numeric(), expected.numeric());
        assert_eq!(actual.provenance(), expected.provenance());
        assert_eq!(actual.expression(), expected.expression());
        assert_eq!(actual.identity(), expected.identity());
        assert_eq!(actual.provenance().origins().sources().unwrap().len(), 4);
        assert!(actual.same_identity(&left));
        assert_eq!(
            actual.interval().unsigned_bounds(),
            (U256::from(5), U256::from(20))
        );
        let demand = selected.demand.as_deref().unwrap();
        assert_eq!(demand.paid_work(), 37 + price);
        assert_eq!(selected.value(&key, domain), Some(expected));
        assert_eq!(
            demand.paid_work(),
            37 + price,
            "complete memo hit owes no numeric action"
        );
        assert!(!selected.stopped());
    }
    assert_eq!(budget.used(), 37 + price);
    assert!(!budget.exhausted());
    assert_eq!((facts, conditional), frozen);
}

#[test]
fn equal_and_top_scope_shortcuts_still_pay_the_real_metadata_join() {
    let domain = Domain::default();
    for (left, right) in [
        (range(2, 8), range(2, 8)),
        (AbstractValue::top(), range(2, 8)),
        (range(2, 8), AbstractValue::top()),
    ] {
        let key = ExprId::fresh().unwrap();
        let guard = ExprId::fresh().unwrap();
        // Full value equality omits temporary identity. Check alias loss too.
        let left = source(left, Origin::Calldata).with_identity(993_118, 1);
        let right = source(right, Origin::Storage).with_identity(993_118, 1);
        assert!(left.same_identity(&right));
        let facts = Facts::from([(key.clone(), left.clone())]);
        let conditional = branches(
            &guard,
            Facts::from([(key.clone(), right.clone())]),
            Facts::new(),
        );
        let quote = preparation_for(&facts, &conditional, domain);
        let price = memo::join_work(domain, &left, &right);
        let expected = Context::new(&facts, Some(&conditional))
            .branch(&guard, true)
            .value(&key, domain)
            .unwrap();
        let mut budget = WorkBudget::new(price);
        {
            let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .branch(&guard, true);
            let actual = selected.value(&key, domain).unwrap();
            assert_eq!(actual, expected);
            assert!(!actual.same_identity(&left));
            assert!(!expected.same_identity(&left));
            assert_eq!(selected.demand.as_deref().unwrap().paid_work(), price);
        }
        assert_eq!(budget.used(), price);
        assert!(!budget.exhausted());
    }
}

#[test]
fn first_scope_value_and_new_guard_truth_need_no_numeric_action() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let source = source(range(5, 25), Origin::Storage).with_identity(993_119, 1);
    let facts = Facts::new();
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), source.clone())]),
        Facts::new(),
    );
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(0);
    {
        let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .branch(&guard, true);
        let actual = selected.value(&key, domain).unwrap();
        assert_eq!(actual, source);
        assert!(actual.same_identity(&source));
        let truth = selected.value(&guard, domain).unwrap();
        let expected = Context::new(&facts, Some(&conditional)).branch(&guard, true);
        assert_eq!(Some(truth), expected.value(&guard, domain));
        assert_eq!(selected.demand.as_deref().unwrap().paid_work(), 0);
        assert!(!selected.stopped());
    }
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
}

#[test]
fn guard_truth_meets_pay_the_original_bound_and_keep_contradiction_fallback() {
    let domain = Domain::default();
    for value in [range(0, 20), constant(0)] {
        let guard = ExprId::fresh().unwrap();
        let value = source(value, Origin::Calldata).with_expression(guard.clone());
        let facts = Facts::from([(guard.clone(), value.clone())]);
        let conditional = ConditionalFacts::default();
        let quote = preparation_for(&facts, &conditional, domain);
        let expected = Context::new(&facts, Some(&conditional))
            .branch(&guard, true)
            .value(&guard, domain);
        let mut budget = WorkBudget::new(quote.reduction_work);
        {
            let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .branch(&guard, true);
            assert_eq!(selected.value(&guard, domain), expected);
            let demand = selected.demand.as_deref().unwrap();
            assert_eq!(demand.paid_work(), quote.reduction_work);
            assert_eq!(selected.value(&guard, domain), expected);
            assert_eq!(demand.paid_work(), quote.reduction_work);
            if value.singleton() == Some(U256::ZERO) {
                assert_eq!(expected, Some(value));
            }
        }
        assert_eq!(budget.used(), quote.reduction_work);
        assert!(!budget.exhausted());
    }
}

#[test]
fn work_boundaries_preserve_the_prefix_and_never_install_negative_or_partial_results() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let cheap = ExprId::fresh().unwrap();
    let left = source(range(0, 20), Origin::Calldata);
    let right = source(range(5, 25), Origin::Storage);
    let facts = Facts::from([(key.clone(), left.clone()), (cheap.clone(), constant(7))]);
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), right.clone())]),
        Facts::new(),
    );
    let frozen = (facts.clone(), conditional.clone());
    let quote = preparation_for(&facts, &conditional, domain);
    let meet = quote.reduction_work;
    let price = meet + memo::join_work(domain, &left, &right);
    let expected = Context::new(&facts, Some(&conditional))
        .branch(&guard, true)
        .value(&key, domain);
    for available in [0, meet - 1, price - 1, price] {
        let mut budget = WorkBudget::new(53 + available);
        assert!(budget.charge(53));
        {
            let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .branch(&guard, true);
            // An already warm independent key must not escape after Work.
            assert_eq!(selected.value(&cheap, domain), Some(constant(7)));
            let actual = selected.value(&key, domain);
            let demand = selected.demand.as_deref().unwrap();
            if available == price {
                assert_eq!(actual, expected);
                assert_eq!(selected.value(&key, domain), expected);
                assert!(!selected.work_exhausted());
                assert_eq!(demand.paid_work(), 53 + price);
            } else {
                assert_eq!(actual, None);
                assert!(selected.work_exhausted());
                assert!(!selected.cache_refused());
                selected.check().unwrap();
                assert_eq!(selected.value(&cheap, domain), None);
                assert_eq!(selected.value(&key, domain), None);
                assert!(!demand.pay(0), "a smaller retry cannot clear Work");
                assert_eq!(
                    selected.memo.borrow().get(&key, domain, demand, &|_| true),
                    None,
                    "interrupted resolve is neither a complete value nor a completed absence"
                );
                assert_eq!(
                    demand.paid_work(),
                    53 + if available >= meet { meet } else { 0 }
                );
            }
        }
        assert_eq!(
            budget.used(),
            53 + if available == price {
                price
            } else if available >= meet {
                meet
            } else {
                0
            }
        );
        assert_eq!(budget.exhausted(), available != price);
        assert_eq!((&facts, &conditional), (&frozen.0, &frozen.1));
    }
}

#[test]
fn full_domain_policies_guard_scopes_and_fact_owners_have_separate_complete_memos() {
    let domain = Domain::default();
    let numeric = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        enabled: false,
        ..domain.spec().relations()
    }));
    let changed = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: domain.spec().relations().max_nodes + 1,
        ..domain.spec().relations()
    }));
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let left = source(range(0, 30), Origin::Calldata);
    let facts = Facts::from([(key.clone(), left.clone())]);
    let yes = source(range(5, 20), Origin::Storage);
    let no = source(range(21, 25), Origin::Storage);
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), yes.clone())]),
        Facts::from([(key.clone(), no.clone())]),
    );
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(usize::MAX);
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget);
        let demand = root.demand.as_deref().unwrap();
        for (truth, value) in [(true, &yes), (false, &no)] {
            let selected = root.branch(&guard, truth);
            for policy in [domain, numeric] {
                let expected = Context::new(&facts, Some(&conditional))
                    .branch(&guard, truth)
                    .value(&key, policy);
                let before = demand.paid_work();
                assert_eq!(selected.value(&key, policy), expected);
                assert_eq!(
                    demand.paid_work(),
                    before + quote.reduction_work + memo::join_work(policy, &left, value)
                );
                let after = demand.paid_work();
                assert_eq!(root.branch(&guard, truth).value(&key, policy), expected);
                assert_eq!(demand.paid_work(), after);
            }
        }
        let reversed = root.branch(&guard, true).branch(&guard, false);
        assert_eq!(
            reversed.value(&key, domain),
            Context::new(&facts, Some(&conditional))
                .branch(&guard, false)
                .value(&key, domain)
        );
        assert_eq!(root.value(&key, domain), Some(left));
        assert_eq!(root.value(&key, changed), None);
        assert!(root.cache_refused());
        assert!(!root.work_exhausted());
        root.check().unwrap();
    }
    let other = Facts::from([(key.clone(), range(26, 29))]);
    let other_quote = preparation_for(&other, &conditional, domain);
    let mut other_budget = WorkBudget::new(usize::MAX);
    let other_context = Context::budgeted(
        &other,
        &conditional,
        domain,
        &other_quote,
        &mut other_budget,
    );
    assert_eq!(other_context.value(&key, domain), Some(range(26, 29)));
}

#[test]
fn an_already_exhausted_original_budget_cannot_return_a_cheap_or_cached_value() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(7);
    assert!(budget.charge(3));
    assert!(!budget.charge(5));
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget);
        assert!(context.work_exhausted());
        assert!(!context.cache_refused());
        assert_eq!(context.value(&key, domain), None);
    }
    assert_eq!(budget.used(), 3);
    assert!(budget.exhausted());
}

#[test]
fn cancellation_during_numeric_payment_is_typed_and_does_not_complete_the_memo() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let left = range(0, 20);
    let right = range(5, 25);
    let facts = Facts::from([(key.clone(), left.clone())]);
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), right.clone())]),
        Facts::new(),
    );
    let quote = preparation_for(&facts, &conditional, domain);
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(usize::MAX);
        assert!(budget.charge(71));
        {
            let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .branch(&guard, true);
            let demand = selected.demand.as_deref().unwrap();
            // The meet completes; cancellation arrives during payment for join.
            demand.cancel_after_payments(2, token.clone());
            assert_eq!(selected.value(&key, domain), None);
            assert_eq!(selected.check(), Err(ExprError::Cancelled));
            assert!(!selected.work_exhausted());
            assert!(!selected.cache_refused());
            assert_eq!(
                selected.memo.borrow().get(&key, domain, demand, &|_| true),
                None
            );
            assert_eq!(
                demand.paid_work(),
                71 + quote.reduction_work + memo::join_work(domain, &left, &right)
            );
        }
        assert!(budget.exhausted());
    });
}

#[test]
fn constants_only_join_pays_finite_insertions_and_numeric_summary_rebuilding() {
    let domain = Domain::new(std::num::NonZeroUsize::new(8).unwrap());
    let left = domain
        .join(&constant(2), &constant(4))
        .with_origin(Origin::Calldata);
    let right = domain
        .join(&constant(4), &constant(6))
        .with_origin(Origin::Storage);
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), left.clone())]);
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), right.clone())]),
        Facts::new(),
    );
    let quote = preparation_for(&facts, &conditional, domain);
    let join = memo::join_work(domain, &left, &right);
    let mut budget = WorkBudget::new(quote.reduction_work + join);
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .branch(&guard, true);
        let expected = Context::new(&facts, Some(&conditional))
            .branch(&guard, true)
            .value(&key, domain);
        assert_eq!(context.value(&key, domain), expected);
        assert_eq!(
            context.demand.as_deref().unwrap().paid_work(),
            quote.reduction_work + join
        );
        assert_eq!(expected.unwrap().singleton(), Some(U256::from(4)));
    }
    assert!(!budget.exhausted());
}

#[test]
fn cancellation_latches_across_all_existing_clones_after_the_cancel_scope_ends() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let missing = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(budget.charge(97));
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget);
        let clone = root.clone();
        let child = root.branch(&ExprId::fresh().unwrap(), true);
        assert_eq!(clone.value(&key, domain), Some(constant(7)));
        let token = analysis_control::Cancellation::new();
        token.cancel();
        analysis_control::with_cancellation(&token, || {
            assert_eq!(clone.check(), Err(ExprError::Cancelled));
        });
        assert!(!analysis_control::current_cancellation().is_cancelled());
        for context in [&root, &clone, &child] {
            assert_eq!(context.check(), Err(ExprError::Cancelled));
            assert_eq!(context.value(&key, domain), None);
            assert_eq!(context.value(&missing, domain), None);
            assert!(!context.work_exhausted());
            assert!(!context.cache_refused());
            let demand = context.demand.as_deref().unwrap();
            assert_eq!(demand.paid_work(), 97);
            assert_eq!(
                context
                    .memo
                    .borrow()
                    .get(&missing, domain, demand, &|_| true),
                None
            );
        }
    }
    assert_eq!(budget.used(), 97);
}

#[test]
fn budgeted_metadata_keeps_all_source_keys_beyond_the_word_width_without_live_charges() {
    let domain = Domain::default();
    let facts: Facts = (0..40)
        .map(|i| (ExprId::fresh().unwrap(), constant(i)))
        .collect();
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(0);
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget);
        for _ in 0..2 {
            for (key, value) in &facts {
                assert_eq!(context.value(key, domain), Some(value.clone()));
            }
        }
        let demand = context.demand.as_deref().unwrap();
        assert_eq!(demand.source_key_visits(), 40);
        assert_eq!(demand.paid_work(), 0);
        assert!(!context.stopped());
    }
    assert_eq!(budget.used(), 0);
    assert!(!budget.exhausted());
}

#[test]
fn choice_node_and_depth_refusals_are_optional_and_do_not_create_negative_memos() {
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = ConditionalFacts::default();
    let large = ExprId::operation(
        revm_bytecode::opcode::ADD,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
        Domain::default().spec().relations().expressions(),
    )
    .unwrap();
    for node_limit in [true, false] {
        let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
            max_nodes: if node_limit { 2 } else { 32 },
            max_depth: 2,
            ..Domain::default().spec().relations()
        }));
        let quote = preparation_for(&facts, &conditional, domain);
        if node_limit {
            assert!(large.nodes() > domain.spec().relations().max_nodes);
        }
        let mut budget = WorkBudget::new(0);
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget);
        let selected = if node_limit {
            root.branch(&large, true)
        } else {
            root.branch(&ExprId::fresh().unwrap(), true)
                .branch(&ExprId::fresh().unwrap(), true)
                .branch(&ExprId::fresh().unwrap(), true)
        };
        assert!(selected.cache_refused());
        assert!(!selected.work_exhausted());
        selected.check().unwrap();
        assert_eq!(selected.value(&key, domain), None);
        let demand = selected.demand.as_deref().unwrap();
        assert_eq!(demand.paid_work(), 0);
        assert_eq!(
            selected.memo.borrow().get(&key, domain, demand, &|_| true),
            None
        );
    }
}
