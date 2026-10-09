//! Common lowering and answer handling; providers only implement primitives.

use crate::{
    Bool, Bv, Error, Outcome, Unknown,
    term::{Node, Term},
};
use std::collections::BTreeMap;

enum Failure {
    Native(Error),
    Cancelled,
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Native(error)
    }
}
fn checkpoint() -> Result<(), Failure> {
    if crate::current_cancellation().is_cancelled() {
        Err(Failure::Cancelled)
    } else {
        Ok(())
    }
}

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
) -> Result<B::Term, Failure> {
    checkpoint()?;
    if let Some(native) = memo.get(&term.id()) {
        return Ok(native.clone());
    }
    let args = term
        .0
        .args
        .iter()
        .map(|arg| lower(arg, backend, memo))
        .collect::<Result<Vec<_>, _>>()?;
    // A child native call can finish after cancellation was requested.
    checkpoint()?;
    let native = backend.make(&term.0, &args)?;
    memo.insert(term.id(), native.clone());
    Ok(native)
}

fn run<B: Backend>(
    assertions: &[Bool],
    value: Option<&Bv>,
    mut backend: B,
) -> Result<Outcome, Failure> {
    let mut memo = BTreeMap::new();
    for assertion in assertions {
        let native = lower(&assertion.0, &mut backend, &mut memo)?;
        checkpoint()?;
        backend.assert(&native)?;
    }
    let value = value
        .map(|value| lower(&value.0, &mut backend, &mut memo))
        .transpose()?;
    checkpoint()?;
    let status = backend.check()?;
    checkpoint()?;
    Ok(match status {
        Status::Sat => {
            let model = if let Some(value) = value.as_ref() {
                checkpoint()?;
                let model = backend.model(value)?;
                checkpoint()?;
                Some(model)
            } else {
                None
            };
            Outcome::Sat(model)
        }
        Status::Unsat => Outcome::Unsat,
        Status::Unknown(reason) => Outcome::Unknown(reason),
    })
}

pub(crate) fn execute<B: Backend>(assertions: &[Bool], value: Option<&Bv>, backend: B) -> Outcome {
    match run(assertions, value, backend) {
        Ok(outcome) => outcome,
        Err(Failure::Native(error)) => Outcome::Error(error),
        Err(Failure::Cancelled) => Outcome::Unknown(Unknown::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::{Backend, Error, Node, Outcome, Status, Unknown, execute};
    use crate::{Bool, Bv, Cancellation, with_cancellation};
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        Make,
        Assert,
        Check,
        Model,
    }

    struct CancelDuringCall {
        token: Cancellation,
        at: Call,
        calls: Rc<RefCell<Vec<Call>>>,
    }
    impl CancelDuringCall {
        fn record(&self, call: Call) {
            self.calls.borrow_mut().push(call);
            if call == self.at {
                self.token.cancel();
            }
        }
    }
    impl Backend for CancelDuringCall {
        type Term = usize;
        fn make(&mut self, _: &Node, _: &[usize]) -> Result<usize, Error> {
            self.record(Call::Make);
            Ok(0)
        }
        fn assert(&mut self, _: &usize) -> Result<(), Error> {
            self.record(Call::Assert);
            Ok(())
        }
        fn check(&mut self) -> Result<Status, Error> {
            self.record(Call::Check);
            Ok(Status::Sat)
        }
        fn model(&mut self, _: &usize) -> Result<String, Error> {
            self.record(Call::Model);
            Ok("#b1".into())
        }
    }

    #[test]
    fn cancellation_inside_a_native_make_prevents_parent_make_and_assert() {
        for assertion in [Bool::from_bool(true), Bool::from_bool(true).not().not()] {
            let token = Cancellation::new();
            let calls = Rc::new(RefCell::new(Vec::new()));
            let backend = CancelDuringCall {
                token: token.clone(),
                at: Call::Make,
                calls: Rc::clone(&calls),
            };
            let outcome = with_cancellation(&token, || execute(&[assertion], None, backend));
            assert_eq!(outcome, Outcome::Unknown(Unknown::Cancelled));
            assert_eq!(*calls.borrow(), [Call::Make]);
        }
    }

    #[test]
    fn cancellation_inside_native_check_skips_model_and_inside_model_discards_answer() {
        for at in [Call::Check, Call::Model] {
            let token = Cancellation::new();
            let calls = Rc::new(RefCell::new(Vec::new()));
            let backend = CancelDuringCall {
                token: token.clone(),
                at,
                calls: Rc::clone(&calls),
            };
            let value = Bv::from_u64(1, 1);
            let outcome = with_cancellation(&token, || execute(&[], Some(&value), backend));
            assert_eq!(outcome, Outcome::Unknown(Unknown::Cancelled));
            assert_eq!(
                *calls.borrow(),
                if at == Call::Check {
                    vec![Call::Make, Call::Check]
                } else {
                    vec![Call::Make, Call::Check, Call::Model]
                }
            );
        }
    }
}
