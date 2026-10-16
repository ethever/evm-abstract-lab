//! Demand-local guard choices select only their corresponding operand facts.

use super::{ConditionalFacts, Facts};
use crate::domain::{AbstractValue, Domain, relational::intersect_numeric, symbolic::ExprId};
use alloy_primitives::U256;
use std::collections::BTreeMap;

/// Resolve the active metadata scopes once per guard environment. Scalar
/// lookups then inspect only their own key in those maps, never the fact DAG.
#[derive(Clone)]
pub(in crate::world::bytes) struct Context<'a> {
    facts: &'a Facts,
    conditional: Option<&'a ConditionalFacts>,
    choices: BTreeMap<ExprId, bool>,
    active: Vec<&'a Facts>,
}

impl<'a> Context<'a> {
    pub(in crate::world::bytes) fn new(
        facts: &'a Facts,
        conditional: Option<&'a ConditionalFacts>,
    ) -> Self {
        let mut out = Self {
            facts,
            conditional,
            choices: BTreeMap::new(),
            active: Vec::new(),
        };
        out.select();
        out
    }

    fn select(&mut self) {
        fn selected<'a>(
            conditional: &'a ConditionalFacts,
            facts: &Facts,
            choices: &BTreeMap<ExprId, bool>,
            active: &mut Vec<&'a Facts>,
        ) {
            for branch in &conditional.branches {
                let truth = choices.get(&branch.condition).copied().or_else(|| {
                    branch
                        .condition
                        .as_constant()
                        .map(|n| n != U256::ZERO)
                        .or_else(|| {
                            active
                                .iter()
                                .rev()
                                .filter_map(|facts| facts.get(&branch.condition))
                                .chain(facts.get(&branch.condition))
                                .find_map(|value| {
                                    if !value.may_be_zero() {
                                        Some(true)
                                    } else if !value.may_be_nonzero() {
                                        Some(false)
                                    } else {
                                        None
                                    }
                                })
                        })
                });
                let Some(nonzero) = truth else { continue };
                let scope = if nonzero {
                    &branch.then_facts
                } else {
                    &branch.else_facts
                };
                if !scope.facts.is_empty() {
                    active.push(&scope.facts);
                }
                selected(&scope.conditional, facts, choices, active);
            }
        }
        self.active.clear();
        if let Some(conditional) = self.conditional {
            selected(conditional, self.facts, &self.choices, &mut self.active);
        }
    }

    pub(in crate::world::bytes) fn branch(&self, condition: &ExprId, nonzero: bool) -> Self {
        let mut out = self.clone();
        out.choices.insert(condition.clone(), nonzero);
        out.select();
        out
    }

    pub(in crate::world::bytes) fn value(
        &self,
        expression: &ExprId,
        domain: Domain,
    ) -> Option<AbstractValue> {
        let mut out = self.facts.get(expression).cloned();
        for value in self.active.iter().filter_map(|facts| facts.get(expression)) {
            out = Some(match out {
                Some(old) => {
                    let numeric = if old.numeric() == value.numeric() || value.numeric().is_top() {
                        old.numeric().clone()
                    } else if old.numeric().is_top() {
                        value.numeric().clone()
                    } else {
                        // A contradictory branch is unreachable. Keeping its
                        // previous upper bound cannot remove a concrete state.
                        intersect_numeric(old.numeric(), value.numeric(), domain)
                            .unwrap_or_else(|| old.numeric().clone())
                    };
                    domain.join(&old, value).with_numeric(numeric)
                }
                None => value.clone(),
            });
        }
        if let Some(nonzero) = self.choices.get(expression) {
            let truth = if *nonzero {
                AbstractValue::unsigned_range(U256::ONE, U256::MAX).expect("nonempty range")
            } else {
                AbstractValue::constant(U256::ZERO)
            };
            out = Some(match out {
                Some(value) => {
                    let numeric = intersect_numeric(value.numeric(), truth.numeric(), domain)
                        .unwrap_or_else(|| value.numeric().clone());
                    value.with_numeric(numeric)
                }
                None => truth,
            });
        }
        out
    }

    pub(in crate::world::bytes) fn write_lower_bound(
        &self,
        destination: &ExprId,
        domain: Domain,
    ) -> Option<U256> {
        self.value(destination, domain)
            .map(|value| value.interval().unsigned_bounds().0)
    }
}
