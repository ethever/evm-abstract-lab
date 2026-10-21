//! This consumer knows ZeroGuards and the EVM feedback protocol, not expressions.
use super::contracts::{Failure, ProofRoute, ZeroGuards};
use evm_abstract::{
    U256,
    analysis::cooperation::{RefinementRoute, Refinements, StackFacts, Subject},
    domain::WordObservation,
    kernel::{Accepts, Availability, Change, Charges, LocalClosure, Read, Reduction, Scoped},
};
use std::collections::BTreeMap;

pub struct Consumer {
    enabled: bool,
    pub pending: usize,
    pub refinements: usize,
}
impl Consumer {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            pending: 0,
            refinements: 0,
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
        let useful = !facts.is_empty();
        let change = <Cx as Accepts<Refinements, RefinementRoute>>::accept(
            cx,
            Scoped::new(subject.scope, ()),
            StackFacts::Reachable(facts),
        )
        .map_err(Failure::Publication)?;
        if useful && change == Change::Changed {
            self.refinements += 1;
        }
        Ok(LocalClosure::Stable)
    }
}
