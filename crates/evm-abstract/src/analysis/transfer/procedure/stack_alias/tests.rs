use super::*;
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{fixture, word},
    },
    domain::{Domain, symbolic::ExprId},
};
use alloy_primitives::U256;

fn fresh() -> AbstractValue {
    AbstractValue::top().with_expression(ExprId::fresh().unwrap())
}

#[test]
fn form_uses_node_identity_and_uniform_bounds_only() {
    let x = fresh();
    let y = fresh();
    let stack = [x.clone(), y.clone(), x.clone(), y];
    assert_eq!(StackAliases::form(&stack, true).unwrap().0, [0, 1, 0, 1]);
    assert_eq!(StackAliases::form(&stack, false).unwrap().0, [0, 1, 2, 3]);

    // Equal numeric bounds and structurally equal, separately allocated input
    // nodes are deliberately insufficient evidence in this small prototype.
    let a = ExprId::constant(U256::from(7));
    let b = ExprId::constant(U256::from(7));
    assert_eq!(a, b);
    let separate = [
        AbstractValue::top().with_expression(a),
        AbstractValue::top().with_expression(b),
    ];
    assert_eq!(StackAliases::form(&separate, true).unwrap().0, [0, 1]);

    let narrow = x.clone().with_numeric(
        crate::domain::NumericValue::unsigned_range(U256::from(2), U256::from(9)).unwrap(),
    );
    let mixed = [x, narrow];
    let aliases = StackAliases::form(&mixed, true).unwrap();
    assert_eq!(aliases.0, [0, 1]);
    let scalar = mixed
        .iter()
        .map(|v| AbstractValue::from_numeric(v.numeric().clone()))
        .collect::<Vec<_>>();
    let restored = aliases.instantiate(&scalar, true).unwrap();
    for (old, new) in mixed.iter().zip(&restored) {
        assert_eq!(old.numeric(), new.numeric());
    }
    assert_ne!(restored[0].expression(), restored[1].expression());
}

#[test]
fn intersection_preserves_common_pairs_even_when_leaders_differ() {
    let left = StackAliases(vec![0, 0, 2, 2]);
    let right = StackAliases(vec![0, 1, 1, 1]);
    let intersection = left.intersect(&right).unwrap();
    assert_eq!(intersection.0, [0, 1, 2, 2]);
    assert_eq!(left.intersect(&left).unwrap(), left);
    assert_eq!(
        intersection
            .intersect(&StackAliases(vec![0, 1, 2, 3]))
            .unwrap()
            .0,
        [0, 1, 2, 3]
    );
    assert!(left.intersect(&StackAliases(vec![])).is_err());
}

#[test]
fn payload_join_and_widen_drop_only_unshared_guarantees() {
    use super::super::{form, instantiate, join, semantic_eq};
    let domain = Domain::default();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = super::super::super::initial(&world, &entry, domain).unwrap();
    let x = fresh();
    let y = fresh();
    input.active_mut().stack = vec![x.clone(), x, y.clone(), y];
    let left = form(&input, &entry, domain).unwrap();
    let a = fresh();
    let b = fresh();
    input.active_mut().stack = vec![a, b.clone(), b.clone(), b];
    let right = form(&input, &entry, domain).unwrap();
    assert!(!semantic_eq(&left, &right));
    for widen in [false, true] {
        let joined = join(&left, &right, domain, widen).unwrap();
        assert_eq!(joined.active().stack_aliases.0, [0, 1, 2, 2]);
        let first = instantiate(&joined, domain).unwrap();
        let second = instantiate(&joined, domain).unwrap();
        let a = &first.active().stack;
        let b = &second.active().stack;
        assert_eq!(a[2].expression(), a[3].expression());
        assert_ne!(a[0].expression(), a[1].expression());
        assert_ne!(a[1].expression(), a[2].expression());
        assert_ne!(a[2].expression(), b[2].expression());
        assert!(first.relations.is_empty());
    }
    let disabled = left
        .active()
        .stack_aliases
        .instantiate(&left.active().stack, false)
        .unwrap();
    assert!(disabled.iter().all(|value| value.expression().is_none()));
}

#[test]
fn wider_alias_input_reexecutes_instead_of_replaying_a_narrow_result() {
    use super::super::{Counters, OutcomeKind, StorageReadPolicy, Table, WorkBudget};
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let (world, entry) = fixture(&[(0x101, "185f5260205ff3")]);
    let mut input = super::super::super::initial(&world, &entry, domain).unwrap();
    let x = fresh();
    input.active_mut().stack = vec![x.clone(), x];
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let (first, record) = table.execute(
        0,
        &world,
        &entry,
        input.clone(),
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed && first.frontiers.is_empty());
    let first_returns = first
        .outcomes
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect::<Vec<_>>();
    assert!(!first_returns.is_empty());
    assert!(
        first_returns
            .iter()
            .all(|o| o.data.read_word(&word(0), domain).singleton() == Some(U256::ZERO))
    );

    input.active_mut().stack[1] = fresh();
    let (second, record) = table.execute(
        1,
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed && second.frontiers.is_empty());
    assert!(record.reused > 0 && record.transfers > 0);
    assert!(second.outcomes.iter().any(|o| o.kind == OutcomeKind::Return
        && o.data.read_word(&word(0), domain).contains(U256::from(1))));
}

#[test]
fn formation_is_prepaid_and_cancellation_does_not_install_a_partial_entry() {
    use super::super::{Counters, Solver, StorageReadPolicy, Table, WorkBudget};
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input = super::super::super::initial(&world, &entry, domain).unwrap();
    let x = fresh();
    input.active_mut().stack = vec![x.clone(), x];
    let first_fee = input.work_size().saturating_mul(4);
    let alias_fee = super::super::payload::alias_form_work(&input);
    let history_fee = super::super::context::work_size(input.active().key.jump_history.len());
    assert!(alias_fee > 0);
    for maximum in [0, first_fee + alias_fee + history_fee - 1] {
        let mut table = Table::default();
        let mut budget = WorkBudget::new(maximum);
        let mut counters = Counters::default();
        let result = Solver {
            table: &mut table,
            world: &world,
            entry: &entry,
            config: &config,
            domain,
            budget: &mut budget,
            counters: &mut counters,
            storage_reads: StorageReadPolicy::Abstract,
        }
        .procedure(input.clone());
        assert_eq!(
            result.unwrap_err().frontier_reason(),
            crate::analysis::FrontierReason::Work,
        );
        assert!(table.nodes.is_empty() && table.procedures.is_empty());
        assert!(budget.exhausted());
        assert_eq!(budget.used(), if maximum == 0 { 0 } else { first_fee });
    }

    let control = crate::analysis::control::Control::new(Default::default());
    control.cancellation().cancel();
    control.scope(|| {
        assert!(StackAliases::form(&input.active().stack, true).is_err());
    });
    assert!(StackAliases::form(&vec![fresh(); 1025], true).is_err());
}
