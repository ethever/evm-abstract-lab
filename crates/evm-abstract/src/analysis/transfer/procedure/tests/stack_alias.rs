use super::super::{AbstractValue, stack_alias::StackAliases};
use crate::{
    analysis::{
        ExecutionConfig,
        machine::fixtures::{fixture, word},
    },
    domain::symbolic::ExprId,
};
use alloy_primitives::U256;

fn fresh() -> AbstractValue {
    AbstractValue::top().with_expression(ExprId::fresh().unwrap())
}

#[test]
fn wider_alias_input_reexecutes_instead_of_replaying_a_narrow_result() {
    use super::super::{Counters, OutcomeKind, StorageReadPolicy, Table, WorkBudget};
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let (world, entry) = fixture(&[(0x101, "185f5260205ff3")]);
    let mut input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
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
    use super::super::{Counters, StorageReadPolicy, Table, WorkBudget};
    let config = ExecutionConfig::default();
    let domain = config.domain().unwrap();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
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
        let result = table
            .solver(
                &world,
                &entry,
                &config,
                &mut budget,
                &mut counters,
                StorageReadPolicy::Abstract,
            )
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
