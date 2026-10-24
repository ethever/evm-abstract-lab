//! Independent excluded-word proof producer; it never publishes StackFacts.
use super::super::rule::Failure;
use super::contracts::{ProofRoute, ZeroGuards};
use crate::carrier::ExcludingWord;
use alloy_primitives::U256;
use analysis_kernel::{Accepts, Change, Charges, LocalClosure, Read, Reduction, Scoped};
use evm_abstract::{
    analysis::cooperation::{
        InputRoute, Inputs, RefinementRoute, Refinements, StackFacts, Subject,
    },
    domain::WordElement,
};
use std::collections::BTreeSet;

pub struct Provider {
    enabled: bool,
    pub subjects: Vec<Subject>,
    pub publications: Vec<(Subject, Change)>,
    pub feedback_observed: usize,
}
impl Provider {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            subjects: Vec::new(),
            publications: Vec::new(),
            feedback_observed: 0,
        }
    }
}
impl<Cx> Reduction<Cx> for Provider
where
    Cx: Read<Inputs<ExcludingWord>, InputRoute>
        + Read<
            Refinements,
            RefinementRoute,
            Error = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::Error,
        > + Accepts<ZeroGuards, ProofRoute>
        + Charges<Error = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::Error>,
{
    type Task = Subject;
    type Error = Failure<
        <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::Error,
        <Cx as Accepts<ZeroGuards, ProofRoute>>::Error,
    >;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(64).map_err(Failure::Infrastructure)?;
        let input = <Cx as Read<Inputs<ExcludingWord>, InputRoute>>::query(
            cx,
            Scoped::new(subject.scope, ()),
        )
        .map_err(Failure::Infrastructure)?;
        let Some(input) = input.value() else {
            return Ok(LocalClosure::Stable);
        };
        if input.subject() != *subject {
            return Err(Failure::WrongSubject);
        }
        let feedback =
            <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))
                .map_err(Failure::Infrastructure)?;
        if matches!(feedback.value(), Some(StackFacts::Reachable(f)) if !f.is_empty()) {
            self.feedback_observed += 1;
        }
        self.subjects.push(*subject);
        let mut proofs = BTreeSet::new();
        if self.enabled {
            for (index, pair) in input.payload().active().stack.windows(2).enumerate() {
                cx.charge(
                    pair[0]
                        .work_size()
                        .saturating_add(pair[1].work_size())
                        .saturating_mul(4),
                )
                .map_err(Failure::Infrastructure)?;
                if pair[0].numeric().contains(U256::from(42)) {
                    continue;
                }
                let (Some(value), Some(guard)) =
                    (pair[0].symbolic_expression(), pair[1].expression())
                else {
                    continue;
                };
                if super::super::fixture::compares_excluded_word(guard, &value) {
                    proofs.insert(index + 1);
                }
            }
        }
        let change = <Cx as Accepts<ZeroGuards, ProofRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            proofs,
        )
        .map_err(Failure::Publication)?;
        self.publications.push((*subject, change));
        Ok(LocalClosure::Stable)
    }
}
