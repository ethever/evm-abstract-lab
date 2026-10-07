//! Common lowering and answer handling; providers only implement primitives.

use crate::{
    Bool, Bv, Error, Outcome, Unknown,
    term::{Node, Term},
};
use std::collections::BTreeMap;

pub(crate) enum Status {
    Sat,
    Unsat,
    Unknown(Unknown),
}

pub(crate) trait Backend {
    type Term: Clone;
    fn make(&mut self, node: &Node, args: &[Self::Term]) -> Result<Self::Term, Error>;
    fn assert(&mut self, term: &Self::Term) -> Result<(), Error>;
    fn check(&mut self) -> Result<Status, Error>;
    fn model(&mut self, term: &Self::Term) -> Result<String, Error>;
}

fn lower<B: Backend>(
    term: &Term,
    backend: &mut B,
    memo: &mut BTreeMap<usize, B::Term>,
) -> Result<B::Term, Error> {
    if let Some(native) = memo.get(&term.id()) {
        return Ok(native.clone());
    }
    let args = term
        .0
        .args
        .iter()
        .map(|arg| lower(arg, backend, memo))
        .collect::<Result<Vec<_>, _>>()?;
    let native = backend.make(&term.0, &args)?;
    memo.insert(term.id(), native.clone());
    Ok(native)
}

fn run<B: Backend>(
    assertions: &[Bool],
    value: Option<&Bv>,
    mut backend: B,
) -> Result<Outcome, Error> {
    let mut memo = BTreeMap::new();
    for assertion in assertions {
        let native = lower(&assertion.0, &mut backend, &mut memo)?;
        backend.assert(&native)?;
    }
    let value = value
        .map(|value| lower(&value.0, &mut backend, &mut memo))
        .transpose()?;
    Ok(match backend.check()? {
        Status::Sat => Outcome::Sat(
            value
                .as_ref()
                .map(|value| backend.model(value))
                .transpose()?,
        ),
        Status::Unsat => Outcome::Unsat,
        Status::Unknown(reason) => Outcome::Unknown(reason),
    })
}

pub(crate) fn execute<B: Backend>(assertions: &[Bool], value: Option<&Bv>, backend: B) -> Outcome {
    run(assertions, value, backend).unwrap_or_else(Outcome::Error)
}
