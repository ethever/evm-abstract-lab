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

struct Trial {
    value: Option<AbstractValue>,
    payments: Vec<(bool, usize)>,
    contexts: usize,
    nodes: usize,
    refused: bool,
    work: bool,
}

impl Trial {
    fn paid(&self) -> usize {
        self.payments.iter().map(|(_, work)| work).sum()
    }

    fn numeric(&self) -> usize {
        self.payments
            .iter()
            .filter(|(numeric, _)| *numeric)
            .map(|(_, work)| work)
            .sum()
    }
}

fn trial(mut plan: Plan, budget: &mut WorkBudget) -> Trial {
    let offset = expr(&plan.offset).unwrap();
    operands::remember(
        &mut plan.facts,
        offset.clone(),
        &plan.offset,
        plan.domain,
        limits(plan.domain),
    )
    .unwrap();
    plan.conditional.check(limits(plan.domain)).unwrap();
    let context = conditional::Context::budgeted(
        &plan.facts,
        &plan.conditional,
        plan.domain,
        plan.preparation.as_ref().unwrap(),
        budget,
    )
    .unwrap()
    .unwrap();
    let value = evaluate(
        &plan.root,
        plan.length.as_ref(),
        &offset,
        &context,
        plan.domain,
    )
    .unwrap();
    context.check().unwrap();
    let (contexts, nodes) = context.demand_usage().unwrap();
    Trial {
        value,
        payments: context.demand_payments(),
        contexts,
        nodes,
        refused: context.cache_refused(),
        work: context.work_exhausted(),
    }
}

// Shared only by the existing inline fixtures: measure a real paid trial,
// excluding its already paid planning/freeze and excluding complete fallback.
pub(super) fn trial_work(plan: Plan, budget: &mut WorkBudget) -> usize {
    let before = budget.used();
    let paid = trial(plan, budget).paid();
    assert_eq!(budget.used(), before + paid);
    paid
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
    let mut receipt_budget = WorkBudget::new(usize::MAX);
    let trace = trial(
        prepare(&memory, domain, &mut receipt_budget),
        &mut receipt_budget,
    );
    let setup = trace.payments[0].1;
    assert!(!trace.payments[0].0);
    let mut budget = WorkBudget::new(43 + preparation + setup);
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
        )
        .unwrap()
        .unwrap();
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
    assert_eq!(budget.used(), 43 + preparation + setup);
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
    let mut plan = prepare(&memory, domain, &mut setup);
    let fallback = plan.fallback_work;
    let preparation = setup.used();
    plan.preparation.as_mut().unwrap().nodes = 0;
    let probe = trial(plan, &mut setup);
    assert!(probe.refused);
    assert!(!probe.work);
    assert_eq!(probe.value, None);
    assert_eq!(probe.nodes, 0);
    let prefix = probe.paid();
    assert!(prefix > 0);
    assert_eq!(setup.used(), preparation + prefix);
    for available in [prefix + fallback - 1, prefix + fallback] {
        let mut budget = WorkBudget::new(67 + preparation + available);
        assert!(budget.charge(67));
        let mut plan = prepare(&memory, domain, &mut budget);
        plan.preparation.as_mut().unwrap().nodes = 0;
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == prefix + fallback {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 67 + preparation + prefix + fallback);
            assert!(!budget.exhausted());
        } else {
            assert_eq!(result, None);
            assert_eq!(budget.used(), 67 + preparation + prefix);
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
    let fallback = plan.fallback_work;
    let full = trial(plan, &mut setup);
    assert_eq!(full.value, Some(expected.clone()));
    assert!(!full.refused && !full.work);
    assert!(full.numeric() > 0);
    assert!(full.nodes > 1);
    let mut plan = prepare(&memory, domain, &mut setup);
    plan.preparation.as_mut().unwrap().nodes = full.nodes - 1;
    let refused = trial(plan, &mut setup);
    assert!(refused.refused && !refused.work);
    assert_eq!(refused.value, None);
    assert!(
        refused.numeric() > 0,
        "refusal must follow the real numeric operation"
    );
    assert_eq!(
        refused.payments,
        full.payments[..full.payments.len() - 1],
        "the final rejected N action pays nothing, including no lifecycle"
    );
    let prefix = refused.paid();
    assert!(prefix < full.paid());
    for available in [prefix + fallback - 1, prefix + fallback] {
        let mut budget = WorkBudget::new(79 + preparation + available);
        assert!(budget.charge(79));
        let mut plan = prepare(&memory, domain, &mut budget);
        plan.preparation.as_mut().unwrap().nodes = full.nodes - 1;
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == prefix + fallback {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 79 + preparation + prefix + fallback);
            assert!(!budget.exhausted());
        } else {
            assert_eq!(result, None);
            assert_eq!(
                budget.used(),
                79 + preparation + prefix,
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
    let maximum = plan.preparation.as_ref().unwrap().nodes;
    let fallback = plan.fallback_work;
    let refused = trial(plan, &mut setup);
    assert_eq!(refused.value, None);
    assert!(refused.refused);
    assert!(!refused.work);
    assert_eq!(refused.contexts, conditional::CONTEXTS);
    assert!(refused.nodes < maximum);
    let prefix = refused.paid();
    assert!(prefix > 0);
    assert_eq!(setup.used(), preparation + prefix);
    for available in [prefix + fallback - 1, prefix + fallback] {
        let mut budget = WorkBudget::new(83 + preparation + available);
        assert!(budget.charge(83));
        let plan = prepare(&memory, domain, &mut budget);
        let result = plan.read_word_budgeted(&mut budget).unwrap();
        if available == prefix + fallback {
            assert_eq!(result, Some(expected.clone()));
            assert_eq!(budget.used(), 83 + preparation + prefix + fallback);
        } else {
            assert_eq!(result, None);
            assert_eq!(budget.used(), 83 + preparation + prefix);
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

#[test]
fn actual_metadata_finishes_when_the_unused_n_maximum_cannot_be_prepaid() {
    let domain = Domain::default();
    let memory = context_tree(2, domain);
    let frozen = memory.clone();
    let expected = memory.read_word_checked(&zero(), domain).unwrap();
    let mut probe = WorkBudget::new(usize::MAX);
    let plan = prepare(&memory, domain, &mut probe);
    let prepared = probe.used();
    let n = plan.preparation.as_ref().unwrap().nodes;
    let offset_work = zero().work_size();
    let facts = fact_work(&memory.operands);
    let quantum = (4 * (memory.root.work_size() + offset_work) + memory.operands.len() + 8)
        .min(PLANNING_QUANTUM);
    let freeze = 2 * (facts + memory.conditional.work_size() + offset_work + 32);
    assert_eq!(
        prepared,
        8 * quantum + 32 * memory.root.work_size() + 8192 + freeze,
        "N is capacity only; planning, selected base and freeze are each paid once"
    );
    let actual = plan.read_word_budgeted(&mut probe).unwrap().unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(actual.expression(), expected.expression());
    let receipt = probe.used();
    assert!(receipt > prepared);
    assert!(receipt < prepared + n);
    let mut budget = WorkBudget::new(receipt);
    let plan = prepare(&memory, domain, &mut budget);
    assert!(
        !budget.can_afford(n),
        "this exact boundary cannot prepay the unused N maximum"
    );
    let actual = plan.read_word_budgeted(&mut budget).unwrap().unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.numeric(), expected.numeric());
    assert_eq!(actual.provenance(), expected.provenance());
    assert_eq!(actual.identity(), expected.identity());
    assert_eq!(actual.expression(), expected.expression());
    assert!(!actual.symbolic_limit_reached());
    assert_eq!(budget.used(), receipt);
    assert!(!budget.exhausted());
    assert_eq!(memory, frozen);
}
