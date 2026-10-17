use super::*;
use crate::{
    domain::provenance::Origin,
    world::bytes::{constant, zero},
};

fn scoped_word(domain: Domain) -> ByteArray {
    let key = ExprId::fresh().unwrap();
    let mut broad = AbstractValue::unsigned_range(U256::ZERO, U256::from(30))
        .unwrap()
        .with_expression(key.clone());
    broad.set_origin(Origin::Calldata);
    let mut narrow = AbstractValue::unsigned_range(U256::from(5), U256::from(25))
        .unwrap()
        .with_expression(key.clone());
    narrow.set_origin(Origin::Storage);
    let mut memory = ByteArray::memory();
    memory.write_word(&zero(), &broad, 4096, domain).unwrap();
    let left = BTreeMap::from([(key.clone(), narrow)]);
    let right = BTreeMap::from([(key, broad)]);
    let empty = conditional::ConditionalFacts::default();
    memory.conditional = conditional::ConditionalFacts::join(
        (&left, &empty),
        (&right, &empty),
        &memory.operands,
        ExprId::constant(U256::ONE),
        limits(domain),
    )
    .unwrap();
    memory
}

fn context_tree(depth: usize, domain: Domain) -> ByteArray {
    if depth == 0 {
        let mut memory = ByteArray::memory();
        memory
            .write_word(
                &zero(),
                &constant(7)
                    .with_origin(Origin::Storage)
                    .with_expression(ExprId::fresh().unwrap()),
                4096,
                domain,
            )
            .unwrap();
        return memory;
    }
    context_tree(depth - 1, domain)
        .join_guarded(
            &context_tree(depth - 1, domain),
            domain,
            ExprId::fresh().unwrap(),
            limits(domain),
        )
        .unwrap()
}

fn prepare(memory: &ByteArray, domain: Domain, budget: &mut WorkBudget) -> Plan {
    let plan = memory
        .prepare_word_read(&zero(), domain, budget)
        .unwrap()
        .unwrap();
    assert!(plan.preparation.is_some());
    assert_eq!(plan.root, memory.root);
    assert_eq!(plan.facts, memory.operands);
    assert_eq!(plan.conditional, memory.conditional);
    assert_eq!(plan.offset, zero());
    assert_eq!(plan.domain.spec(), domain.spec());
    assert_eq!(
        plan.fallback_work,
        memory.word_numeric_work(&zero(), domain)
    );
    plan
}

#[test]
fn prepared_trial_matches_the_complete_evaluator_and_exact_whole_read_work_boundary() {
    let domain = Domain::default();
    let memory = scoped_word(domain);
    let frozen = memory.clone();
    let expected = memory.read_word_checked(&zero(), domain).unwrap();
    let mut probe = WorkBudget::new(usize::MAX);
    let plan = prepare(&memory, domain, &mut probe);
    let prepared = probe.used();
    let actual = plan.read_word_budgeted(&mut probe).unwrap().unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.provenance().origins().sources().unwrap().len(), 2);
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(actual.expression(), expected.expression());
    let price = probe.used();
    assert!(
        price > prepared,
        "this fixture must execute a live numeric action"
    );
    for available in [price - 1, price] {
        let mut budget = WorkBudget::new(101 + available);
        assert!(budget.charge(101));
        let plan = prepare(&memory, domain, &mut budget);
        let before = budget.used();
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == price {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 101 + price);
            assert!(!budget.exhausted());
        } else {
            assert_eq!(
                result, None,
                "a Reader Top sentinel is never a completed word"
            );
            assert!(budget.exhausted());
            assert!(budget.used() >= before);
            assert!(budget.used() < 101 + price);
        }
        assert_eq!(memory, frozen);
    }
}

#[test]
fn a_work_stopped_trial_does_not_attach_a_result_or_mutate_its_frozen_plan_payload() {
    let domain = Domain::default();
    let memory = scoped_word(domain);
    let mut prepaid = WorkBudget::new(usize::MAX);
    drop(prepare(&memory, domain, &mut prepaid));
    let preparation = prepaid.used();
    let mut budget = WorkBudget::new(43 + preparation);
    assert!(budget.charge(43));
    let plan = prepare(&memory, domain, &mut budget);
    let source = (
        plan.root.clone(),
        plan.facts.clone(),
        plan.conditional.clone(),
        plan.offset.clone(),
    );
    {
        let context = conditional::Context::budgeted(
            &plan.facts,
            &plan.conditional,
            domain,
            plan.preparation.as_ref().unwrap(),
            &mut budget,
        );
        assert_eq!(
            evaluate(
                &plan.root,
                None,
                &expr(&plan.offset).unwrap(),
                &context,
                domain
            )
            .unwrap(),
            None
        );
        assert!(context.work_exhausted());
        assert!(!context.cache_refused());
        context.check().unwrap();
        assert_eq!(
            evaluate(
                &plan.root,
                None,
                &expr(&plan.offset).unwrap(),
                &context,
                domain
            )
            .unwrap(),
            None
        );
    }
    assert_eq!(budget.used(), 43 + preparation);
    assert!(budget.exhausted());
    assert_eq!(
        (plan.root, plan.facts, plan.conditional, plan.offset),
        source
    );
}

#[test]
fn scratch_refusal_prepays_the_complete_original_fallback_on_the_same_budget() {
    let domain = Domain::default();
    let memory = scoped_word(domain);
    let frozen = memory.clone();
    let expected = memory.read_word_checked(&zero(), domain).unwrap();
    let mut setup = WorkBudget::new(usize::MAX);
    let fallback = prepare(&memory, domain, &mut setup).fallback_work;
    let preparation = setup.used();
    for available in [fallback - 1, fallback] {
        let mut budget = WorkBudget::new(67 + preparation + available);
        assert!(budget.charge(67));
        let mut plan = prepare(&memory, domain, &mut budget);
        plan.preparation.as_mut().unwrap().nodes = 0;
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == fallback {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 67 + preparation + fallback);
            assert!(!budget.exhausted());
        } else {
            assert_eq!(result, None);
            assert_eq!(budget.used(), 67 + preparation);
            assert!(budget.exhausted());
        }
        assert_eq!(memory, frozen);
    }
}

#[test]
fn scratch_refusal_after_numeric_work_keeps_that_work_and_pays_the_full_fallback() {
    let domain = Domain::default();
    let memory = scoped_word(domain);
    let frozen = memory.clone();
    let expected = memory.read_word_checked(&zero(), domain).unwrap();
    let mut setup = WorkBudget::new(usize::MAX);
    let plan = prepare(&memory, domain, &mut setup);
    let preparation = setup.used();
    let mut probe = WorkBudget::new(usize::MAX);
    let nodes = {
        let context = conditional::Context::budgeted(
            &plan.facts,
            &plan.conditional,
            domain,
            plan.preparation.as_ref().unwrap(),
            &mut probe,
        );
        assert_eq!(
            evaluate(&plan.root, None, &expr(&zero()).unwrap(), &context, domain).unwrap(),
            Some(expected.clone())
        );
        assert!(!context.stopped());
        context.demand_usage().unwrap().1
    };
    let numeric = probe.used();
    assert!(numeric > 0);
    assert!(nodes > 1);
    let fallback = plan.fallback_work;
    for available in [numeric + fallback - 1, numeric + fallback] {
        let mut budget = WorkBudget::new(79 + preparation + available);
        assert!(budget.charge(79));
        let mut plan = prepare(&memory, domain, &mut budget);
        // Fail the final arena reservation, after the numeric operation ran.
        plan.preparation.as_mut().unwrap().nodes = nodes - 1;
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == numeric + fallback {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 79 + preparation + numeric + fallback);
            assert!(!budget.exhausted());
        } else {
            assert_eq!(result, None);
            assert_eq!(
                budget.used(),
                79 + preparation + numeric,
                "no refund or partial fallback reservation"
            );
            assert!(budget.exhausted());
        }
        assert_eq!(memory, frozen);
    }
}

#[test]
fn thirty_two_context_refusal_drops_the_graph_and_pays_the_full_original_fallback() {
    let domain = Domain::default();
    let memory = context_tree(5, domain);
    let frozen = memory.clone();
    let expected = memory.read_word_checked(&zero(), domain).unwrap();
    let mut setup = WorkBudget::new(usize::MAX);
    let plan = prepare(&memory, domain, &mut setup);
    let preparation = setup.used();
    let mut probe = WorkBudget::new(usize::MAX);
    {
        let context = conditional::Context::budgeted(
            &plan.facts,
            &plan.conditional,
            domain,
            plan.preparation.as_ref().unwrap(),
            &mut probe,
        );
        assert_eq!(
            evaluate(&plan.root, None, &expr(&zero()).unwrap(), &context, domain).unwrap(),
            None
        );
        assert!(context.cache_refused());
        assert!(!context.work_exhausted());
        assert_eq!(context.demand_usage().unwrap().0, conditional::CONTEXTS);
        assert!(context.demand_usage().unwrap().1 < plan.preparation.as_ref().unwrap().nodes);
    }
    let numeric = probe.used();
    let fallback = plan.fallback_work;
    for available in [numeric + fallback - 1, numeric + fallback] {
        let mut budget = WorkBudget::new(83 + preparation + available);
        assert!(budget.charge(83));
        let plan = prepare(&memory, domain, &mut budget);
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == numeric + fallback {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 83 + preparation + numeric + fallback);
        } else {
            assert_eq!(result, None);
            assert_eq!(budget.used(), 83 + preparation + numeric);
            assert!(budget.exhausted());
        }
        assert_eq!(memory, frozen);
    }
}

#[test]
fn zero_and_pre_cancelled_reads_preserve_work_and_typed_expression_errors() {
    let domain = Domain::default();
    let memory = scoped_word(domain);
    let frozen = memory.clone();
    let mut zero_budget = WorkBudget::new(0);
    assert!(
        memory
            .prepare_word_read(&zero(), domain, &mut zero_budget)
            .unwrap()
            .is_none()
    );
    assert!(zero_budget.exhausted());
    assert_eq!(zero_budget.used(), 0);
    let mut setup = WorkBudget::new(usize::MAX);
    let plan = prepare(&memory, domain, &mut setup);
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(usize::MAX);
        assert!(budget.charge(89));
        token.cancel();
        assert_eq!(
            plan.read_word_budgeted(&mut budget),
            Err(ExprError::Cancelled.into())
        );
        assert_eq!(budget.used(), 89);
    });
    let mut plan = prepare(&memory, domain, &mut setup);
    let error = ExprError::Limit {
        nodes: 100,
        depth: 10,
    };
    plan.failure = Some(error.clone().into());
    let mut budget = WorkBudget::new(0);
    assert_eq!(plan.read_word_budgeted(&mut budget), Err(error.into()));
    assert_eq!(budget.used(), 0);
    assert_eq!(memory, frozen);
}
