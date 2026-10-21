use super::observation;
use crate::{
    U256,
    domain::{WordElement, WordObservation},
    resource::WorkBudget,
    ssa::{FactScope, FactSource, FactValidity, SsaError},
};
use serde::Serialize;
use std::fmt;

/// Any unquoted query/copy is a test failure, not an accidentally cheap builtin call.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Unquoted;
impl Clone for Unquoted {
    fn clone(&self) -> Self {
        panic!("unfunded native copy")
    }
}
impl fmt::Display for Unquoted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unquoted")
    }
}
impl WordElement for Unquoted {
    fn top() -> Self {
        Self
    }
    fn constant(_: U256) -> Self {
        panic!("this refusal fixture never constructs literals")
    }
    fn from_observation(_: &WordObservation) -> Self {
        Self
    }
    fn observation(&self) -> WordObservation {
        panic!("unfunded native export")
    }
    fn singleton(&self) -> Option<U256> {
        panic!("unfunded native singleton")
    }
    fn is_top(&self) -> bool {
        true
    }
    fn work_size(&self) -> usize {
        1
    }
    fn clone_work(&self) -> usize {
        usize::MAX
    }
    fn singleton_work(&self) -> usize {
        usize::MAX
    }
}
#[test]
fn unknown_native_copy_or_singleton_quote_refuses_before_either_operation() {
    let mut budget = WorkBudget::new(2_000_000_000);
    let error = observation(
        0,
        FactScope::Definition,
        FactValidity::Current,
        FactSource::Entry,
        Some(&Unquoted),
        None,
        &mut budget,
    )
    .unwrap_err();
    assert!(matches!(error, SsaError::Work));
    assert_eq!(budget.used(), 0);
}
#[test]
fn cancellation_refuses_native_postprocessing_before_query_or_copy() {
    let control = crate::analysis::control::Control::new(Default::default());
    control.cancellation().cancel();
    control.scope(|| {
        let mut budget = WorkBudget::new(2_000_000_000);
        let error = observation(
            0,
            FactScope::Definition,
            FactValidity::Current,
            FactSource::Entry,
            Some(&Unquoted),
            None,
            &mut budget,
        )
        .unwrap_err();
        assert!(matches!(error, SsaError::Cancelled));
        assert_eq!(budget.used(), 0);
    });
}
#[test]
fn a_separate_postprocessing_budget_must_cover_native_fact_copy_and_query() {
    let numeric = crate::domain::FiniteConstantSet::constant(U256::from(7));
    let quote = numeric.clone_work() + numeric.singleton_work();
    let mut budget = WorkBudget::new(quote - 1);
    assert!(matches!(
        observation(
            0,
            FactScope::Definition,
            FactValidity::Current,
            FactSource::Entry,
            Some(&numeric),
            None,
            &mut budget
        ),
        Err(SsaError::Work)
    ));
    assert_eq!(budget.used(), 0);
    let mut budget = WorkBudget::new(quote);
    let fact = observation(
        0,
        FactScope::Definition,
        FactValidity::Current,
        FactSource::Entry,
        Some(&numeric),
        None,
        &mut budget,
    )
    .unwrap();
    assert_eq!(fact.numeric, Some(numeric));
    assert_eq!(fact.constant, Some(U256::from(7)));
    assert_eq!(budget.used(), quote);
}
