use super::{Context, Facts, memo, preparation};
use crate::domain::{
    AbstractValue, Domain,
    identity::Symbol,
    provenance::Origin,
    relational::RelationLimits,
    symbolic::{ExprError, ExprId},
};
use crate::world::bytes::conditional::{Branch, ConditionalFacts, Scope};
use alloy_primitives::U256;
use std::{rc::Rc, sync::Arc};

fn constant(value: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
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

#[test]
fn memo_keeps_complete_values_negative_results_and_exact_guard_environments() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let source = AbstractValue::top()
        .with_origin(Origin::Storage)
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(993_017))
        .with_numeric(constant(7).numeric().clone());
    let facts = Facts::new();
    let conditional = branches(
        &guard,
        Facts::from([(key.clone(), source.clone())]),
        Facts::from([(key.clone(), constant(9))]),
    );
    let context = Context::prepared(&facts, &conditional, domain, 1_000_000);
    assert_eq!(context.value(&key, domain), None);
    assert_eq!(
        context
            .memo
            .borrow()
            .get(&key, domain, context.demand.as_deref().unwrap(), &|_| true),
        Some(None)
    );
    let yes = context.branch(&guard, true);
    let first = yes.value(&key, domain).unwrap();
    assert_eq!(first, source);
    assert!(first.same_identity(&source));
    let before = yes.demand.as_ref().unwrap().used();
    let second = yes.value(&key, domain).unwrap();
    assert_eq!(second, first);
    assert!(second.same_identity(&source));
    assert!(yes.demand.as_ref().unwrap().used() - before < 64);
    assert!(Rc::ptr_eq(&yes.memo, &context.branch(&guard, true).memo));
    assert_eq!(
        context.branch(&guard, false).value(&key, domain),
        Some(constant(9))
    );
    assert_eq!(
        yes.branch(&guard, false).value(&key, domain),
        Some(constant(9))
    );
    assert_eq!(context.value(&key, domain), None);
    context.check().unwrap();
}

#[test]
fn intersections_and_metadata_joins_are_reused_without_changing_the_meet() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let left = AbstractValue::unsigned_range(U256::ZERO, U256::from(20))
        .unwrap()
        .with_origin(Origin::Calldata);
    let right = AbstractValue::unsigned_range(U256::from(5), U256::from(25))
        .unwrap()
        .with_origin(Origin::Storage);
    let facts = Facts::from([(key.clone(), left)]);
    let conditional = branches(&guard, Facts::from([(key.clone(), right)]), Facts::new());
    let selected = Context::prepared(&facts, &conditional, domain, 1_000_000).branch(&guard, true);
    let expected = Context::new(&facts, Some(&conditional))
        .branch(&guard, true)
        .resolve(&key, domain)
        .unwrap();
    let before = selected.demand.as_ref().unwrap().used();
    assert_eq!(selected.value(&key, domain), Some(expected.clone()));
    let after = selected.demand.as_ref().unwrap().used();
    assert_eq!(
        expected.interval().unsigned_bounds(),
        (U256::from(5), U256::from(20))
    );
    assert_eq!(selected.value(&key, domain), Some(expected));
    assert!(selected.demand.as_ref().unwrap().used() - after < after - before);
    selected.check().unwrap();
}

#[test]
fn inactive_contradictions_and_a_missing_key_are_not_impossibility_or_zero() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let missing = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = branches(
        &guard,
        Facts::new(),
        Facts::from([(key.clone(), constant(9))]),
    );
    let context = Context::prepared(&facts, &conditional, domain, 1_000_000);
    for nonzero in [true, false] {
        let selected = context.branch(&guard, nonzero);
        assert_eq!(selected.value(&key, domain), Some(constant(7)));
        assert_eq!(selected.value(&missing, domain), None);
        selected.check().unwrap();
    }
}

#[test]
fn guard_truth_intersection_is_cached_and_preserves_the_old_fallback() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let facts = Facts::from([(guard.clone(), constant(0))]);
    let conditional = ConditionalFacts::default();
    let context = Context::prepared(&facts, &conditional, domain, 1_000_000).branch(&guard, true);
    assert_eq!(context.value(&guard, domain), Some(constant(0)));
    assert_eq!(context.value(&guard, domain), Some(constant(0)));
    context.check().unwrap();
}

#[test]
fn cache_identity_includes_all_domain_limits_and_the_immutable_fact_owner() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let guard = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = ConditionalFacts::default();
    let context = Context::prepared(&facts, &conditional, domain, 1_000_000);
    assert_eq!(context.value(&key, domain), Some(constant(7)));
    let numeric = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        enabled: false,
        ..domain.spec().relations()
    }));
    assert_eq!(context.value(&key, numeric), Some(constant(7)));
    let changed = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: domain.spec().relations().max_nodes + 1,
        ..domain.spec().relations()
    }));
    assert_eq!(context.value(&key, changed), None);
    assert!(context.cache_refused());
    context.check().unwrap();
    let other = Facts::from([(key.clone(), constant(9))]);
    assert_eq!(
        Context::new(&other, None).value(&key, domain),
        Some(constant(9))
    );
    // Unprepared callers resolve each policy with the original complete meet.
    let fallback = Context::new(&facts, None).branch(&guard, true);
    for policy in [domain, numeric, changed] {
        assert_eq!(fallback.value(&key, policy), fallback.resolve(&key, policy));
    }
}

#[test]
fn prepared_cache_keeps_every_source_fact_beyond_the_word_width() {
    let domain = Domain::default();
    let facts: Facts = (0..40)
        .map(|i| (ExprId::fresh().unwrap(), constant(i)))
        .collect();
    let conditional = ConditionalFacts::default();
    let context = Context::prepared(&facts, &conditional, domain, 1_000_000);
    for _ in 0..2 {
        for (key, value) in &facts {
            assert_eq!(context.value(key, domain), Some(value.clone()));
        }
    }
    context.check().unwrap();
}

#[test]
fn metadata_walk_and_context_capacity_refuse_only_the_private_attempt() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = ConditionalFacts::default();
    let short = Context::prepared(&facts, &conditional, domain, 1);
    assert_eq!(short.value(&key, domain), None);
    assert!(short.cache_refused());
    short.check().unwrap();
    assert_eq!(
        short
            .memo
            .borrow()
            .get(&key, domain, short.demand.as_deref().unwrap(), &|_| true),
        None
    );
    let root = Context::prepared(&facts, &conditional, domain, 1_000_000);
    let mut context = root.clone();
    for _ in 1..memo::CONTEXTS {
        context = context.branch(&ExprId::fresh().unwrap(), true);
        context.check().unwrap();
    }
    context = context.branch(&ExprId::fresh().unwrap(), true);
    assert!(context.cache_refused());
    context.check().unwrap();
    assert!(root.cache_refused());
    root.check().unwrap();
}

#[test]
fn cancellation_is_observed_even_on_a_warm_memo_hit() {
    let domain = Domain::default();
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), constant(7))]);
    let conditional = ConditionalFacts::default();
    let context = Context::prepared(&facts, &conditional, domain, 1_000_000);
    assert_eq!(context.value(&key, domain), Some(constant(7)));
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert_eq!(context.value(&key, domain), None);
        assert_eq!(context.check(), Err(ExprError::Cancelled));
    });
    assert_eq!(
        context
            .memo
            .borrow()
            .get(&key, domain, context.demand.as_deref().unwrap(), &|_| true),
        Some(Some(constant(7)))
    );
}

#[test]
fn guarded_word_preparation_pays_a_bounded_arena_instead_of_every_lookup_reduction() {
    let domain = Domain::default();
    let guard = ExprId::fresh().unwrap();
    let facts = Facts::from([
        (ExprId::constant(U256::ZERO), constant(0)),
        (ExprId::constant(U256::from(7)), constant(7)),
    ]);
    let conditional = branches(
        &guard,
        Facts::from([(ExprId::fresh().unwrap(), constant(7))]),
        Facts::from([(ExprId::fresh().unwrap(), constant(7))]),
    );
    let old = 72usize * 32 * (1 + conditional.lookup_work(domain));
    let fact_work = facts.iter().fold(0usize, |work, (key, value)| {
        work.saturating_add(key.work_size())
            .saturating_add(value.work_size())
    });
    let prepared = preparation(
        72,
        &constant(0),
        fact_work,
        facts.len(),
        &conditional,
        domain,
    );
    // Preparation now prepays only metadata; payer tests cover actual numeric
    // actions and atomic payment refusal.
    assert_eq!(prepared.work, prepared.nodes);
    assert!(prepared.work < old);
    assert!(prepared.work < 20_000_000);
}

#[test]
fn warm_key_lookups_do_not_rescan_the_frozen_source_map() {
    let domain = Domain::default();
    for count in [40, 80] {
        let facts: Facts = (0..count)
            .map(|i| (ExprId::fresh().unwrap(), constant(i)))
            .collect();
        let conditional = ConditionalFacts::default();
        let context = Context::prepared(&facts, &conditional, domain, 10_000_000);
        let demand = context.demand.as_deref().unwrap();
        assert_eq!(demand.source_key_visits(), count);
        for (key, expected) in &facts {
            assert_eq!(context.value(key, domain), Some(expected.clone()));
        }
        let before = demand.source_key_visits();
        for _ in 0..2 {
            for (key, expected) in &facts {
                assert_eq!(context.value(key, domain), Some(expected.clone()));
            }
        }
        assert_eq!(demand.source_key_visits(), before);
        assert!(!context.cache_refused());
        context.check().unwrap();
    }
}
