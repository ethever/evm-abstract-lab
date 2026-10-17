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
use std::{rc::Rc, sync::Arc};

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

// Complete paid traces derive exact fixture budgets without arbitrary padding.
// The oracle is separate: every traced production action still uses WorkBudget.
fn trace_for(
    facts: &Facts,
    conditional: &ConditionalFacts,
    domain: Domain,
    quote: &Preparation,
    action: impl FnOnce(&Context<'_>),
) -> Vec<(bool, usize)> {
    let mut budget = WorkBudget::new(usize::MAX);
    let context = Context::budgeted(facts, conditional, domain, quote, &mut budget)
        .unwrap()
        .unwrap();
    action(&context);
    assert!(!context.stopped());
    let trace = context.demand.as_deref().unwrap().payments();
    drop(context);
    assert_eq!(budget.used(), total(&trace));
    trace
}

fn total(trace: &[(bool, usize)]) -> usize {
    trace.iter().map(|(_, work)| work).sum()
}

fn numeric_work(trace: &[(bool, usize)]) -> usize {
    trace
        .iter()
        .filter(|(numeric, _)| *numeric)
        .map(|(_, work)| work)
        .sum()
}

fn prefix(trace: &[(bool, usize)], available: usize) -> usize {
    let mut paid = 0;
    for (_, work) in trace {
        if *work > available - paid {
            break;
        }
        paid += work;
    }
    paid
}

// A complete positive memo hit pays entry, policy search, B-tree comparison
// envelope and the copied AV, with no retained object or numeric action.
fn hit_work(key: &ExprId, value: &AbstractValue, len: usize, maximum_key: usize) -> usize {
    let comparisons = len.min(11 * (len.saturating_add(1).ilog2() as usize + 1));
    1 + 2 + comparisons * (key.work_size() + maximum_key + 1) + 1 + value.work_size()
}

// Persistent Eq omits runtime identity. A runtime-only probe also detects its
// preservation or loss without an immutable input masking the alias result.
fn assert_complete_value(
    actual: &AbstractValue,
    expected: &AbstractValue,
    runtime_probe: &AbstractValue,
) {
    assert_eq!(actual, expected);
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.finite_constants(), expected.finite_constants());
    assert_eq!(actual.known_bits(), expected.known_bits());
    assert_eq!(actual.interval(), expected.interval());
    assert_eq!(actual.congruence(), expected.congruence());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(actual.expression(), expected.expression());
    assert_eq!(
        actual.symbolic_limit_reached(),
        expected.symbolic_limit_reached()
    );
    assert!(runtime_probe.identity().input().is_none());
    assert_eq!(
        actual.same_identity(runtime_probe),
        expected.same_identity(runtime_probe)
    );
}

#[test]
fn more_than_thirty_two_same_parent_guards_keep_exact_paid_values_without_refusal() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let cases: Vec<_> = (0..memo::CONTEXTS + 8)
        .map(|index| {
            let guard = ExprId::fresh().unwrap();
            let yes = source(range(index * 4 + 1, index * 4 + 3), Origin::Storage)
                .with_expression(key.clone())
                .with_symbolic_limit()
                .with_identity(993_122, index as u32);
            let no = source(range(index * 4 + 2, index * 4 + 4), Origin::Calldata)
                .with_expression(key.clone())
                .with_identity(993_123, index as u32);
            (guard, yes, no)
        })
        .collect();
    let facts = Facts::new();
    let conditional = ConditionalFacts::from_branches(
        cases
            .iter()
            .map(|(guard, yes, no)| {
                Arc::new(Branch {
                    condition: guard.clone(),
                    then_facts: Scope {
                        facts: Facts::from([(key.clone(), yes.clone())]),
                        conditional: ConditionalFacts::default(),
                    },
                    else_facts: Scope {
                        facts: Facts::from([(key.clone(), no.clone())]),
                        conditional: ConditionalFacts::default(),
                    },
                })
            })
            .collect(),
    );
    let frozen = (facts.clone(), conditional.clone());
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(usize::MAX);
    let receipt;
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let demand = root.demand.as_deref().unwrap();
        assert_eq!(root.value(&key, domain), None);
        for (index, (guard, value, _)) in cases.iter().enumerate() {
            let selected = root.branch(guard, true);
            let oracle = Context::new(&facts, Some(&conditional)).branch(guard, true);
            assert_eq!(selected.choices.len(), 1);
            assert_eq!(selected.choices.get(guard), Some(&true));
            assert_eq!(selected.active.len(), 1);
            assert!(std::ptr::eq(
                selected.active[0],
                &conditional.branches[index].then_facts.facts
            ));
            assert!(!Rc::ptr_eq(&selected.memo, &root.memo));
            let actual = selected.value(&key, domain).unwrap();
            let expected = oracle.value(&key, domain).unwrap();
            assert_complete_value(&actual, &expected, value);
            assert!(actual.same_identity(value));
            assert!(actual.symbolic_limit_reached());
            assert_complete_value(
                &selected.value(guard, domain).unwrap(),
                &oracle.value(guard, domain).unwrap(),
                value,
            );
            assert_eq!(demand.retained_contexts(), (index + 2).min(memo::CONTEXTS));
            assert_eq!(
                root.memo.borrow().branches.len(),
                (index + 1).min(memo::CONTEXTS - 1)
            );
            assert!(
                !selected.stopped(),
                "retention capacity alone cannot refuse a legal child"
            );
        }
        let (guard, _, value) = cases.last().unwrap();
        let selected = root.branch(guard, false);
        let expected = Context::new(&facts, Some(&conditional)).branch(guard, false);
        assert_complete_value(
            &selected.value(&key, domain).unwrap(),
            &expected.value(&key, domain).unwrap(),
            value,
        );
        assert_eq!(selected.choices.get(guard), Some(&false));
        assert!(std::ptr::eq(
            selected.active[0],
            &conditional.branches.last().unwrap().else_facts.facts
        ));
        assert_eq!(demand.retained_contexts(), memo::CONTEXTS);
        assert!(root.choices.is_empty());
        assert_eq!(root.value(&key, domain), None);
        for ((pointer, _), (guard, _)) in &root.memo.borrow().branches {
            assert_eq!(
                *pointer,
                guard.immutable_node_key(),
                "retained strong key owns the pointer"
            );
        }
        assert_eq!(
            Rc::strong_count(&root.memo),
            1,
            "children do not retain their parent"
        );
        assert!(!root.stopped());
        assert!(demand.used() <= quote.nodes);
        assert_eq!(numeric_work(&demand.payments()), 0);
        receipt = total(&demand.payments());
        assert_eq!(demand.paid_work(), receipt);
    }
    assert_eq!(budget.used(), receipt);
    assert!(!budget.exhausted());
    assert_eq!((facts, conditional), frozen);
}

#[test]
fn full_retention_keeps_warm_hits_and_repeats_uncached_actions_for_both_full_policies() {
    let domain = Domain::default();
    let numeric = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        enabled: false,
        ..domain.spec().relations()
    }));
    let key = ExprId::fresh().unwrap();
    let warm_guard = ExprId::fresh().unwrap();
    let cold_guard = ExprId::operation(
        revm_bytecode::opcode::ADD,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    let left = source(range(0, 30), Origin::Calldata)
        .with_identity(993_124, 1)
        .with_expression(key.clone());
    let right = source(range(5, 20), Origin::Storage)
        .with_identity(993_124, 1)
        .with_expression(key.clone())
        .with_symbolic_limit();
    let facts = Facts::from([(key.clone(), left.clone())]);
    let conditional = ConditionalFacts::from_branches(
        [&warm_guard, &cold_guard]
            .into_iter()
            .map(|guard| {
                Arc::new(Branch {
                    condition: guard.clone(),
                    then_facts: Scope {
                        facts: Facts::from([(key.clone(), right.clone())]),
                        conditional: ConditionalFacts::default(),
                    },
                    else_facts: Scope {
                        facts: Facts::new(),
                        conditional: ConditionalFacts::default(),
                    },
                })
            })
            .collect(),
    );
    let quote = preparation_for(&facts, &conditional, domain);
    let mut budget = WorkBudget::new(usize::MAX);
    let receipt;
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let demand = root.demand.as_deref().unwrap();
        let warm = root.branch(&warm_guard, true);
        for policy in [domain, numeric] {
            let before = numeric_work(&demand.payments());
            let expected = Context::new(&facts, Some(&conditional))
                .branch(&warm_guard, true)
                .value(&key, policy)
                .unwrap();
            let actual = warm.value(&key, policy).unwrap();
            assert_complete_value(&actual, &expected, &left);
            assert!(
                !actual.same_identity(&left),
                "the real metadata join drops runtime identity"
            );
            assert_eq!(
                numeric_work(&demand.payments()),
                before + quote.reduction_work + memo::join_work(policy, &left, &right)
            );
        }
        for _ in 2..memo::CONTEXTS {
            root.branch(&ExprId::fresh().unwrap(), true);
        }
        assert_eq!(demand.retained_contexts(), memo::CONTEXTS);
        // This longer guard is absent from the source maps. Its temporary choice
        // must enlarge subsequent comparison envelopes even though not retained.
        let large = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[cold_guard.clone(), ExprId::fresh().unwrap()],
            domain.spec().relations().expressions(),
        )
        .unwrap();
        assert!(large.work_size() > conditional.largest_expression);
        let temporary = root.branch(&large, true);
        assert!(!temporary.stopped());
        assert_eq!(temporary.choices.get(&large), Some(&true));
        for policy in [domain, numeric] {
            let before = demand.paid_work();
            let before_numeric = numeric_work(&demand.payments());
            let hit = root.branch(&warm_guard, true);
            assert!(Rc::ptr_eq(&hit.memo, &warm.memo));
            let expected = Context::new(&facts, Some(&conditional))
                .branch(&warm_guard, true)
                .value(&key, policy)
                .unwrap();
            assert_complete_value(&hit.value(&key, policy).unwrap(), &expected, &left);
            assert_eq!(
                demand.paid_work(),
                before + 1 + 1 + memo::CONTEXTS + hit_work(&key, &expected, 1, large.work_size())
            );
            assert_eq!(numeric_work(&demand.payments()), before_numeric);
        }
        let mut previous: Option<Context<'_>> = None;
        let mut previous_trace = None;
        for _ in 0..2 {
            let start = demand.payments().len();
            let selected = root.branch(&cold_guard, true);
            if let Some(previous) = &previous {
                assert!(!Rc::ptr_eq(&selected.memo, &previous.memo));
            }
            assert!(!Rc::ptr_eq(&selected.memo, &warm.memo));
            assert_eq!(selected.choices.len(), 1);
            assert_eq!(selected.active.len(), 1);
            assert!(std::ptr::eq(
                selected.active[0],
                &conditional.branches[1].then_facts.facts
            ));
            for policy in [domain, numeric] {
                let before = numeric_work(&demand.payments());
                let expected = Context::new(&facts, Some(&conditional))
                    .branch(&cold_guard, true)
                    .value(&key, policy)
                    .unwrap();
                assert_complete_value(&selected.value(&key, policy).unwrap(), &expected, &left);
                assert_eq!(
                    numeric_work(&demand.payments()),
                    before + quote.reduction_work + memo::join_work(policy, &left, &right)
                );
                let paid = demand.paid_work();
                assert_complete_value(&selected.value(&key, policy).unwrap(), &expected, &left);
                assert_eq!(
                    demand.paid_work(),
                    paid + hit_work(&key, &expected, 1, large.work_size())
                );
                assert_eq!(
                    numeric_work(&demand.payments()),
                    before + quote.reduction_work + memo::join_work(policy, &left, &right)
                );
            }
            let trace = demand.payments()[start..].to_vec();
            if let Some(previous_trace) = previous_trace {
                assert_eq!(
                    trace, previous_trace,
                    "an unretained child repeats its complete paid work"
                );
            }
            previous_trace = Some(trace);
            previous = Some(selected);
            assert_eq!(demand.retained_contexts(), memo::CONTEXTS);
            assert_eq!(root.memo.borrow().branches.len(), memo::CONTEXTS - 1);
            assert!(
                !root
                    .memo
                    .borrow()
                    .branches
                    .contains_key(&(cold_guard.immutable_node_key(), true))
            );
            assert!(!root.stopped());
        }
        receipt = total(&demand.payments());
        assert_eq!(demand.paid_work(), receipt);
        assert!(demand.used() <= quote.nodes);
    }
    assert_eq!(budget.used(), receipt);
    assert!(!budget.exhausted());
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
    let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
        assert_eq!(
            root.branch(&guard, true).value(&key, domain),
            Some(expected.clone())
        );
    });
    assert_eq!(numeric_work(&trace), price);
    let receipt = total(&trace);
    let hit = hit_work(&key, &expected, 1, key.work_size().max(guard.work_size()));
    let mut budget = WorkBudget::new(receipt + hit + 37);
    assert!(budget.charge(37));
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
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
        assert_eq!(demand.paid_work(), 37 + receipt);
        assert_eq!(selected.value(&key, domain), Some(expected));
        assert_eq!(
            demand.paid_work(),
            37 + receipt + hit,
            "complete memo hit pays probes/copy but no new numeric action or lifecycle"
        );
        assert_eq!(numeric_work(&demand.payments()), price);
        assert!(!selected.stopped());
    }
    assert_eq!(budget.used(), 37 + receipt + hit);
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
        let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
            assert_eq!(
                root.branch(&guard, true).value(&key, domain),
                Some(expected.clone())
            );
        });
        assert_eq!(numeric_work(&trace), price);
        let receipt = total(&trace);
        let mut budget = WorkBudget::new(receipt);
        {
            let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .unwrap()
                .unwrap()
                .branch(&guard, true);
            let actual = selected.value(&key, domain).unwrap();
            assert_eq!(actual, expected);
            assert!(!actual.same_identity(&left));
            assert!(!expected.same_identity(&left));
            assert_eq!(selected.demand.as_deref().unwrap().paid_work(), receipt);
            assert_eq!(
                numeric_work(&selected.demand.as_deref().unwrap().payments()),
                price
            );
        }
        assert_eq!(budget.used(), receipt);
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
    let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
        let selected = root.branch(&guard, true);
        assert_eq!(selected.value(&key, domain), Some(source.clone()));
        assert!(selected.value(&guard, domain).is_some());
    });
    assert_eq!(numeric_work(&trace), 0);
    let receipt = total(&trace);
    assert!(receipt > 0);
    let mut budget = WorkBudget::new(receipt);
    {
        let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap()
            .branch(&guard, true);
        let actual = selected.value(&key, domain).unwrap();
        assert_eq!(actual, source);
        assert!(actual.same_identity(&source));
        let truth = selected.value(&guard, domain).unwrap();
        let expected = Context::new(&facts, Some(&conditional)).branch(&guard, true);
        assert_eq!(Some(truth), expected.value(&guard, domain));
        assert_eq!(selected.demand.as_deref().unwrap().paid_work(), receipt);
        assert_eq!(
            numeric_work(&selected.demand.as_deref().unwrap().payments()),
            0
        );
        assert!(!selected.stopped());
    }
    assert_eq!(budget.used(), receipt);
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
        let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
            assert_eq!(root.branch(&guard, true).value(&guard, domain), expected);
        });
        assert_eq!(numeric_work(&trace), quote.reduction_work);
        let receipt = total(&trace);
        let hit = hit_work(&guard, expected.as_ref().unwrap(), 1, guard.work_size());
        let mut budget = WorkBudget::new(receipt + hit);
        {
            let selected = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .unwrap()
                .unwrap()
                .branch(&guard, true);
            assert_eq!(selected.value(&guard, domain), expected);
            let demand = selected.demand.as_deref().unwrap();
            assert_eq!(demand.paid_work(), receipt);
            assert_eq!(selected.value(&guard, domain), expected);
            assert_eq!(demand.paid_work(), receipt + hit);
            assert_eq!(numeric_work(&demand.payments()), quote.reduction_work);
            if value.singleton() == Some(U256::ZERO) {
                assert_eq!(expected, Some(value));
            }
        }
        assert_eq!(budget.used(), receipt + hit);
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
    let expected = Context::new(&facts, Some(&conditional))
        .branch(&guard, true)
        .value(&key, domain);
    let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
        let selected = root.branch(&guard, true);
        assert_eq!(selected.value(&cheap, domain), Some(constant(7)));
        assert_eq!(selected.value(&key, domain), expected);
    });
    let numeric_prices: Vec<_> = trace.iter().filter(|(n, _)| *n).map(|(_, w)| *w).collect();
    assert_eq!(
        numeric_prices,
        [quote.reduction_work, memo::join_work(domain, &left, &right)]
    );
    let before_meet = total(&trace[..trace.iter().position(|(n, _)| *n).unwrap()]);
    let before_join = total(&trace[..trace.iter().rposition(|(n, _)| *n).unwrap()]);
    let receipt = total(&trace);
    let hit = hit_work(
        &key,
        expected.as_ref().unwrap(),
        2,
        key.work_size()
            .max(guard.work_size())
            .max(cheap.work_size()),
    );
    for available in [before_meet, before_join, receipt - 1, receipt + hit] {
        let mut budget = WorkBudget::new(53 + available);
        assert!(budget.charge(53));
        {
            let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .unwrap()
                .unwrap();
            let selected = root.branch(&guard, true);
            let clone = selected.clone();
            assert_eq!(selected.value(&cheap, domain), Some(constant(7)));
            let actual = selected.value(&key, domain);
            let demand = selected.demand.as_deref().unwrap();
            if available == receipt + hit {
                assert_eq!(actual, expected);
                assert_eq!(clone.value(&key, domain), expected);
                assert!(!selected.work_exhausted());
                assert_eq!(demand.paid_work(), 53 + receipt + hit);
                assert_eq!(numeric_work(&demand.payments()), numeric_work(&trace));
            } else {
                assert_eq!(actual, None);
                assert!(selected.work_exhausted());
                assert!(!selected.cache_refused());
                selected.check().unwrap();
                let paid = demand.paid_work();
                let used = demand.used();
                for context in [&root, &selected, &clone] {
                    assert_eq!(context.value(&cheap, domain), None);
                    assert_eq!(context.value(&key, domain), None);
                    assert!(context.work_exhausted());
                }
                assert!(!demand.pay(0), "a smaller retry cannot clear Work");
                assert!(!demand.reserve(0));
                assert_eq!(demand.used(), used);
                assert_eq!(
                    selected.memo.borrow().get(&key, domain, demand, &|_| true),
                    None,
                    "interrupted resolve is neither a complete value nor a completed absence"
                );
                assert_eq!(demand.paid_work(), paid);
                assert_eq!(paid, 53 + prefix(&trace, available));
            }
        }
        assert_eq!(
            budget.used(),
            53 + if available == receipt + hit {
                receipt + hit
            } else {
                prefix(&trace, available)
            }
        );
        assert_eq!(budget.exhausted(), available != receipt + hit);
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
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let demand = root.demand.as_deref().unwrap();
        for (truth, value) in [(true, &yes), (false, &no)] {
            let selected = root.branch(&guard, truth);
            for policy in [domain, numeric] {
                let expected = Context::new(&facts, Some(&conditional))
                    .branch(&guard, truth)
                    .value(&key, policy);
                let before = demand.paid_work();
                let before_numeric = numeric_work(&demand.payments());
                assert_eq!(selected.value(&key, policy), expected);
                assert!(demand.paid_work() > before);
                assert_eq!(
                    numeric_work(&demand.payments()),
                    before_numeric + quote.reduction_work + memo::join_work(policy, &left, value)
                );
                let after = demand.paid_work();
                assert_eq!(root.branch(&guard, truth).value(&key, policy), expected);
                let hit = hit_work(
                    &key,
                    expected.as_ref().unwrap(),
                    1,
                    key.work_size().max(guard.work_size()),
                );
                assert_eq!(demand.paid_work(), after + 1 + 1 + memo::CONTEXTS + hit);
                assert_eq!(
                    numeric_work(&demand.payments()),
                    before_numeric + quote.reduction_work + memo::join_work(policy, &left, value)
                );
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
    )
    .unwrap()
    .unwrap();
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
        assert!(
            Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
                .unwrap()
                .is_none()
        );
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
                .unwrap()
                .unwrap()
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
                numeric_work(&demand.payments()),
                quote.reduction_work + memo::join_work(domain, &left, &right)
            );
            assert_eq!(demand.paid_work(), 71 + total(&demand.payments()));
            assert!(total(&demand.payments()) > numeric_work(&demand.payments()));
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
    let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
        assert!(root.branch(&guard, true).value(&key, domain).is_some());
    });
    assert_eq!(numeric_work(&trace), quote.reduction_work + join);
    let mut budget = WorkBudget::new(total(&trace));
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap()
            .branch(&guard, true);
        let expected = Context::new(&facts, Some(&conditional))
            .branch(&guard, true)
            .value(&key, domain);
        assert_eq!(context.value(&key, domain), expected);
        assert_eq!(
            context.demand.as_deref().unwrap().paid_work(),
            total(&trace)
        );
        assert_eq!(
            numeric_work(&context.demand.as_deref().unwrap().payments()),
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
    let paid;
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let clone = root.clone();
        for _ in 1..memo::CONTEXTS {
            root.branch(&ExprId::fresh().unwrap(), true);
        }
        let child = root.branch(&ExprId::fresh().unwrap(), true);
        assert_eq!(root.demand_usage().unwrap().0, memo::CONTEXTS);
        assert_eq!(root.memo.borrow().branches.len(), memo::CONTEXTS - 1);
        assert_eq!(clone.value(&key, domain), Some(constant(7)));
        paid = root.demand.as_deref().unwrap().paid_work();
        assert!(paid > 97);
        let used = root.demand.as_deref().unwrap().used();
        let payments = root.demand_payments();
        let token = analysis_control::Cancellation::new();
        token.cancel();
        analysis_control::with_cancellation(&token, || {
            assert_eq!(clone.check(), Err(ExprError::Cancelled));
        });
        assert!(!analysis_control::current_cancellation().is_cancelled());
        for context in [&root, &clone, &child] {
            assert_eq!(context.check(), Err(ExprError::Cancelled));
            let stopped_branch = context.branch(&ExprId::fresh().unwrap(), true);
            assert!(Rc::ptr_eq(&stopped_branch.memo, &context.memo));
            assert_eq!(stopped_branch.demand_usage().unwrap().0, memo::CONTEXTS);
            assert_eq!(context.value(&key, domain), None);
            assert_eq!(context.value(&missing, domain), None);
            assert!(!context.work_exhausted());
            assert!(!context.cache_refused());
            let demand = context.demand.as_deref().unwrap();
            assert_eq!(demand.paid_work(), paid);
            assert_eq!(demand.used(), used);
            assert_eq!(demand.payments(), payments);
            assert_eq!(
                context
                    .memo
                    .borrow()
                    .get(&missing, domain, demand, &|_| true),
                None
            );
        }
    }
    assert_eq!(budget.used(), paid);
}

#[test]
fn budgeted_metadata_keeps_all_source_keys_beyond_the_word_width_with_actual_charges() {
    let domain = Domain::default();
    let facts: Facts = (0..40)
        .map(|i| (ExprId::fresh().unwrap(), constant(i)))
        .collect();
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let trace = trace_for(&facts, &conditional, domain, &quote, |context| {
        for _ in 0..2 {
            for (key, value) in &facts {
                assert_eq!(context.value(key, domain), Some(value.clone()));
            }
        }
    });
    assert_eq!(numeric_work(&trace), 0);
    let receipt = total(&trace);
    assert!(receipt > 0);
    let mut budget = WorkBudget::new(receipt);
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        for _ in 0..2 {
            for (key, value) in &facts {
                assert_eq!(context.value(key, domain), Some(value.clone()));
            }
        }
        let demand = context.demand.as_deref().unwrap();
        assert_eq!(demand.source_key_visits(), 40);
        assert_eq!(demand.paid_work(), receipt);
        assert_eq!(numeric_work(&demand.payments()), 0);
        assert!(!context.stopped());
    }
    assert_eq!(budget.used(), receipt);
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
        let mut budget = WorkBudget::new(usize::MAX);
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let selected = if node_limit {
            root.branch(&large, true)
        } else {
            root.branch(&ExprId::fresh().unwrap(), true)
                .branch(&ExprId::fresh().unwrap(), true)
                .branch(&ExprId::fresh().unwrap(), true)
        };
        assert!(selected.cache_refused());
        assert!(!selected.work_exhausted());
        assert_eq!(
            selected.demand_usage().unwrap().0,
            if node_limit { 1 } else { 3 }
        );
        assert!(selected.memo.borrow().branches.is_empty());
        selected.check().unwrap();
        assert_eq!(selected.value(&key, domain), None);
        let demand = selected.demand.as_deref().unwrap();
        assert!(demand.paid_work() > 0);
        assert_eq!(numeric_work(&demand.payments()), 0);
        let paid = demand.paid_work();
        assert!(!demand.reserve(1));
        assert_eq!(demand.paid_work(), paid);
        assert_eq!(
            selected.memo.borrow().get(&key, domain, demand, &|_| true),
            None
        );
    }
}

#[test]
fn metadata_payment_failure_is_shared_and_never_publishes_a_completed_absence() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let missing = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let value = source(constant(7), Origin::Storage).with_identity(993_120, 1);
    let facts = Facts::from([(key.clone(), value.clone())]);
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let retained_guards: Vec<_> = (1..memo::CONTEXTS)
        .map(|_| ExprId::fresh().unwrap())
        .collect();
    let trace = trace_for(&facts, &conditional, domain, &quote, |root| {
        for guard in &retained_guards {
            root.branch(guard, true);
        }
        root.branch(&guard, true);
        assert_eq!(root.value(&key, domain), Some(value.clone()));
    });
    assert_eq!(numeric_work(&trace), 0);
    let receipt = total(&trace);
    let mut budget = WorkBudget::new(109 + receipt);
    assert!(budget.charge(109));
    {
        let root = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        let clone = root.clone();
        for guard in &retained_guards {
            root.branch(guard, true);
        }
        let child = root.branch(&guard, true);
        assert_eq!(root.demand_usage().unwrap().0, memo::CONTEXTS);
        assert!(
            !root
                .memo
                .borrow()
                .branches
                .contains_key(&(guard.immutable_node_key(), true))
        );
        let actual = clone.value(&key, domain).unwrap();
        assert_eq!(actual, value);
        assert_eq!(actual.identity(), value.identity());
        assert!(actual.same_identity(&value));
        let demand = root.demand.as_deref().unwrap();
        let used = demand.used();
        assert_eq!(demand.paid_work(), 109 + receipt);
        // The first real metadata action for this miss cannot be paid.
        assert_eq!(child.value(&missing, domain), None);
        assert_eq!(demand.used(), used, "an unpaid action commits no N usage");
        for context in [&root, &clone, &child] {
            assert!(context.work_exhausted());
            assert!(!context.cache_refused());
            let stopped_branch = context.branch(&ExprId::fresh().unwrap(), true);
            assert!(Rc::ptr_eq(&stopped_branch.memo, &context.memo));
            assert_eq!(stopped_branch.demand_usage().unwrap().0, memo::CONTEXTS);
            context.check().unwrap();
            assert_eq!(
                context.value(&key, domain),
                None,
                "warm values cannot escape shared Work"
            );
            assert_eq!(context.value(&missing, domain), None);
        }
        assert!(!demand.reserve(0));
        assert!(!demand.pay(0));
        assert_eq!(
            child.memo.borrow().get(&missing, domain, demand, &|_| true),
            None
        );
        assert_eq!(
            root.memo.borrow().get(&key, domain, demand, &|_| true),
            Some(Some(value))
        );
        assert_eq!(demand.paid_work(), 109 + receipt);
        assert_eq!(demand.used(), used);
    }
    assert_eq!(budget.used(), 109 + receipt);
    assert!(budget.exhausted());
}

#[test]
fn scratch_admission_precedes_payment_and_unpaid_usage_never_commits() {
    let domain = Domain::default();
    let conditional = ConditionalFacts::default();
    let facts = Facts::new();
    let mut quote = preparation_for(&facts, &conditional, domain);
    quote.nodes = 3;
    quote.work = quote.nodes;
    let mut budget = WorkBudget::new(0);
    {
        let demand = memo::Demand::new(domain, &quote, 0, &mut budget);
        assert!(!demand.reserve(4));
        assert!(demand.refused());
        assert!(!demand.work_exhausted());
        assert_eq!(demand.used(), 0);
        assert_eq!(demand.paid_work(), 0);
        assert!(demand.payments().is_empty());
    }
    assert!(
        !budget.exhausted(),
        "unexecuted optional work never reaches the ledger"
    );

    quote.nodes = 8;
    quote.work = quote.nodes;
    let mut budget = WorkBudget::new(5);
    {
        let demand = memo::Demand::new(domain, &quote, 0, &mut budget);
        assert!(demand.reserve(2));
        assert!(!demand.reserve(4));
        assert!(demand.work_exhausted());
        assert!(!demand.refused());
        assert_eq!(demand.used(), 2);
        assert_eq!(demand.paid_work(), 2);
        assert!(!demand.reserve(1));
        assert_eq!(demand.payments(), [(false, 2)]);
    }
    assert_eq!(budget.used(), 2);
    assert!(budget.exhausted());

    // This reference probe reaches checked_add overflow without huge objects.
    let demand = memo::Demand::oracle(domain, usize::MAX, 0);
    assert!(demand.reserve(usize::MAX));
    assert!(!demand.reserve(1));
    assert!(demand.refused());
    assert!(!demand.work_exhausted());
    assert_eq!(demand.used(), usize::MAX);
    assert_eq!(demand.payments(), [(false, usize::MAX)]);
}

#[test]
fn fixed_setup_is_paid_before_construction_and_cancellation_stays_typed() {
    let domain = Domain::default();
    let facts = Facts::new();
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let setup = memo::CONTEXT_LIFETIME_WORK + memo::DEMAND_LIFETIME_WORK;
    let mut budget = WorkBudget::new(113 + setup - 1);
    assert!(budget.charge(113));
    assert!(
        Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(budget.used(), 113);
    assert!(budget.exhausted());
    let mut budget = WorkBudget::new(setup);
    {
        let context = Context::budgeted(&facts, &conditional, domain, &quote, &mut budget)
            .unwrap()
            .unwrap();
        assert!(context.work_exhausted());
        assert!(!context.cache_refused());
        assert_eq!(context.demand_usage(), Some((1, 0)));
        assert_eq!(
            context.demand.as_deref().unwrap().payments(),
            [(false, setup)]
        );
    }
    assert_eq!(budget.used(), setup);
    let token = analysis_control::Cancellation::new();
    token.cancel();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(usize::MAX);
        assert!(matches!(
            Context::budgeted(&facts, &conditional, domain, &quote, &mut budget),
            Err(ExprError::Cancelled)
        ));
        assert_eq!(budget.used(), 0);
    });
}

#[test]
fn negative_replacement_and_positive_memo_hits_pay_each_owned_lifecycle_once() {
    let domain = Domain::default();
    let facts = Facts::new();
    let conditional = ConditionalFacts::default();
    let quote = preparation_for(&facts, &conditional, domain);
    let first = ExprId::fresh().unwrap();
    let next = ExprId::operation(
        revm_bytecode::opcode::ADD,
        &[ExprId::fresh().unwrap(), ExprId::constant(U256::ONE)],
        domain.spec().relations().expressions(),
    )
    .unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    let expected;
    {
        let demand = memo::Demand::new(domain, &quote, next.work_size(), &mut budget);
        let mut memo = memo::Memo::default();
        memo.insert(&first, &None, domain, &demand, &|n| demand.reserve(n));
        let first_fee = 2 + 2 + 2 * first.work_size() + 1;
        let first_nodes = 2 + 1 + first.work_size() + 1;
        assert_eq!(demand.paid_work(), first_fee);
        assert_eq!(demand.used(), first_nodes);
        memo.insert(&next, &None, domain, &demand, &|n| demand.reserve(n));
        let next_fee = 2 + 2 * next.work_size() + 1;
        assert_eq!(
            demand.paid_work(),
            first_fee + next_fee,
            "old negative release was already prepaid"
        );
        assert_eq!(
            demand.used(),
            first_nodes + 2 + next.work_size() + first.work_size() + 1
        );
        let negative_hit = 2 + next.comparison_work(&next);
        assert_eq!(
            memo.get(&next, domain, &demand, &|n| demand.reserve(n)),
            Some(None)
        );
        assert_eq!(demand.paid_work(), first_fee + next_fee + negative_hit);

        let value = source(constant(7), Origin::Storage).with_identity(993_121, 1);
        let before = demand.paid_work();
        memo.insert(&first, &Some(value.clone()), domain, &demand, &|n| {
            demand.reserve(n)
        });
        // Policy search2, empty tree probe1, copy/insertion V+1, release V+1.
        assert_eq!(
            demand.paid_work() - before,
            2 + 1 + 2 * (value.work_size() + 1)
        );
        let before = demand.paid_work();
        let actual = memo
            .get(&first, domain, &demand, &|n| demand.reserve(n))
            .unwrap()
            .unwrap();
        assert_eq!(actual, value);
        assert_eq!(actual.identity(), value.identity());
        assert!(actual.same_identity(&value));
        // Memo::get omits Context's entry1; the retained negative is compared.
        let hit = 2
            + first.comparison_work(&next)
            + first.work_size()
            + next.work_size()
            + 1
            + 1
            + value.work_size();
        assert_eq!(demand.paid_work(), before + hit);
        assert_eq!(numeric_work(&demand.payments()), 0);
        expected = demand.paid_work();
    }
    assert_eq!(
        budget.used(),
        expected,
        "drop is prepaid and never charges again"
    );
}
