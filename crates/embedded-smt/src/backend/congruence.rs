//! Exact finite UF elimination for BV results and extensional array arguments.
//!
//! For each pair of applications of one declaration, assert equal arguments
//! imply equal results. Any satisfying finite assignment extends to a function;
//! no injectivity or relation between different declarations is introduced.
//! This uses the same QF_ABV primitives on every provider, with no quantifiers
//! or provider-specific function-domain restrictions. The worst-case expansion
//! is quadratic in distinct applications, linear in their arity, and bounded by
//! the query's separate congruence allowance before entering native solving.

use super::{Backend, Failure, checkpoint};
use crate::term::{Node, Op, Sort, Term};
use std::{collections::BTreeMap, sync::Arc};

struct Application<T> {
    ids: Vec<usize>,
    args: Vec<T>,
    result: T,
}

pub(super) struct Congruence<T> {
    by_function: BTreeMap<usize, Vec<Application<T>>>,
    remaining: u32,
}

fn make<B: Backend>(
    backend: &mut B,
    op: Op,
    sort: Sort,
    args: &[B::Term],
) -> Result<B::Term, Failure> {
    checkpoint()?;
    backend.make(
        &Node {
            op,
            sort,
            args: vec![],
        },
        args,
    )
}

fn bounded_make<B: Backend>(
    remaining: &mut u32,
    backend: &mut B,
    op: Op,
    args: &[B::Term],
) -> Result<B::Term, Failure> {
    checkpoint()?;
    *remaining = remaining.checked_sub(1).ok_or(Failure::ResourceLimit)?;
    make(backend, op, Sort::Bool, args)
}

impl<T: Clone> Congruence<T> {
    pub(super) fn new(rlimit: u32) -> Self {
        Self {
            by_function: BTreeMap::new(),
            remaining: rlimit,
        }
    }

    pub(super) fn apply<B: Backend<Term = T>>(
        &mut self,
        term: &Term,
        args: &[T],
        backend: &mut B,
    ) -> Result<T, Failure> {
        let Op::Apply(declaration) = &term.0.op else {
            unreachable!("function application")
        };
        let previous = self
            .by_function
            .entry(Arc::as_ptr(declaration) as usize)
            .or_default();
        let ids: Vec<_> = term.0.args.iter().map(Term::id).collect();
        // Identical DAG arguments require no new result or congruence clauses.
        for application in previous.iter() {
            checkpoint()?;
            if application.ids == ids {
                return Ok(application.result.clone());
            }
        }
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(Failure::ResourceLimit)?;
        let result = make(
            backend,
            Op::Variable(format!("{}!application!{}", declaration.name, term.id())),
            term.0.sort,
            &[],
        )?;
        for application in previous.iter() {
            checkpoint()?;
            let equal_args = args
                .iter()
                .zip(&application.args)
                .map(|(a, b)| {
                    bounded_make(
                        &mut self.remaining,
                        backend,
                        Op::Eq,
                        &[a.clone(), b.clone()],
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let equal_args = match equal_args.as_slice() {
                [] => bounded_make(&mut self.remaining, backend, Op::Bool(true), &[])?,
                [arg] => arg.clone(),
                _ => bounded_make(&mut self.remaining, backend, Op::And, &equal_args)?,
            };
            let equal_results = bounded_make(
                &mut self.remaining,
                backend,
                Op::Eq,
                &[result.clone(), application.result.clone()],
            )?;
            let different_args =
                bounded_make(&mut self.remaining, backend, Op::Not, &[equal_args])?;
            let implication = bounded_make(
                &mut self.remaining,
                backend,
                Op::Or,
                &[different_args, equal_results],
            )?;
            checkpoint()?;
            backend.assert(&implication)?;
        }
        previous.push(Application {
            ids,
            args: args.to_vec(),
            result: result.clone(),
        });
        Ok(result)
    }
}
