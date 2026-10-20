//! Independent adapter: it knows only the declared proof capability, not Policy/N.
use super::super::rule::Failure;
use super::contracts::{ProofRoute, ZeroGuards};
use alloy_primitives::U256;
use analysis_kernel::{
    Accepts, Availability, Change, Charges, LocalClosure, Read, Reduction, Scoped,
};
use evm_abstract::{
    analysis::cooperation::{RefinementRoute, Refinements, StackFacts, Subject},
    domain::WordObservation,
};
use std::collections::BTreeMap;

pub struct Consumer {
    enabled: bool,
    pub subjects: Vec<Subject>,
    pub feedback: Vec<(Subject, Change)>,
    pub pending: usize,
}
impl Consumer {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            subjects: Vec::new(),
            feedback: Vec::new(),
            pending: 0,
        }
    }
}
impl<Cx> Reduction<Cx> for Consumer
where
    Cx: Read<ZeroGuards, ProofRoute>
        + Read<Refinements, RefinementRoute, Error = <Cx as Read<ZeroGuards, ProofRoute>>::Error>
        + Accepts<Refinements, RefinementRoute>
        + Charges<Error = <Cx as Read<ZeroGuards, ProofRoute>>::Error>,
{
    type Task = Subject;
    type Error = Failure<
        <Cx as Read<ZeroGuards, ProofRoute>>::Error,
        <Cx as Accepts<Refinements, RefinementRoute>>::Error,
    >;
    fn reduce(&mut self, subject: &Subject, cx: &mut Cx) -> Result<LocalClosure, Self::Error> {
        cx.charge(64).map_err(Failure::Infrastructure)?;
        self.subjects.push(*subject);
        let proof = <Cx as Read<ZeroGuards, ProofRoute>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        if proof.state == Availability::Pending {
            self.pending += 1;
        }
        let Some(proof) = proof.value() else {
            return Ok(LocalClosure::Stable);
        };
        <Cx as Read<Refinements, RefinementRoute>>::query(cx, Scoped::new(subject.scope, ()))
            .map_err(Failure::Infrastructure)?;
        cx.charge(proof.len().saturating_add(1).saturating_mul(1024))
            .map_err(Failure::Infrastructure)?;
        let mut facts = BTreeMap::new();
        if self.enabled {
            for index in proof {
                facts.insert(*index, WordObservation::constant(U256::ZERO));
            }
        }
        let change = <Cx as Accepts<Refinements, RefinementRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            StackFacts::Reachable(facts),
        )
        .map_err(Failure::Publication)?;
        self.feedback.push((*subject, change));
        Ok(LocalClosure::Stable)
    }
}
