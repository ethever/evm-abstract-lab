//! Uninterpreted BV results with BV/array arguments and declaration identity.

use super::{Array, Bv, Context, Op, Sort, Term};
use std::sync::Arc;

/// An allowed argument sort for an uninterpreted function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionSort {
    /// A nonempty bit-vector.
    Bv(u32),
    /// A total BV-indexed, BV-valued array.
    Array {
        /// Positive width of each index.
        index_width: u32,
        /// Positive width of each element.
        element_width: u32,
    },
}
impl FunctionSort {
    fn sort(self) -> Sort {
        match self {
            Self::Bv(width) => {
                assert!(width > 0, "function argument width must be positive");
                Sort::Bv(width)
            }
            Self::Array {
                index_width,
                element_width,
            } => {
                assert!(
                    index_width > 0,
                    "function array index width must be positive"
                );
                assert!(
                    element_width > 0,
                    "function array element width must be positive"
                );
                Sort::Array {
                    index_width,
                    element_width,
                }
            }
        }
    }
}

/// A borrowed, typed argument supplied to a function application.
#[derive(Clone, Copy, Debug)]
pub enum FunctionArg<'a> {
    /// A bit-vector argument.
    Bv(&'a Bv),
    /// An array argument, compared by its entire contents.
    Array(&'a Array),
}
impl FunctionArg<'_> {
    fn term(self) -> Term {
        match self {
            Self::Bv(value) => value.0.clone(),
            Self::Array(value) => value.0.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct FunctionDecl {
    pub name: String,
    domain: Vec<Sort>,
    result_width: u32,
}

/// An uninterpreted function with a BV result and fixed BV/array argument sorts.
///
/// Reuse or clone one declaration for the same operation. Equal arguments imply
/// equal results, including array equality proved by path conditions. Different
/// arguments impose no inequality, so this does not assume absence of collisions.
/// Array arguments compare their entire contents. To model a digest of a range,
/// callers must supply normalized range contents and the relevant length; a
/// memory snapshot and length alone do not identify equal data at different offsets.
#[derive(Clone, Debug)]
pub struct Function(Arc<FunctionDecl>);

impl Context {
    /// Allocate a fresh function identity; widths must be positive.
    /// An empty domain denotes one unconstrained but consistent constant.
    pub fn fresh_function(
        &self,
        prefix: &str,
        domain: &[FunctionSort],
        result_width: u32,
    ) -> Function {
        assert!(result_width > 0, "function result width must be positive");
        Function(Arc::new(FunctionDecl {
            name: self.fresh_name(prefix),
            domain: domain.iter().map(|sort| sort.sort()).collect(),
            result_width,
        }))
    }
}

impl Function {
    /// Apply this declaration, checking the argument count and every argument sort.
    pub fn apply(&self, args: &[FunctionArg<'_>]) -> Bv {
        assert_eq!(
            args.len(),
            self.0.domain.len(),
            "function argument count must match"
        );
        let args: Vec<_> = args.iter().map(|arg| arg.term()).collect();
        for (arg, sort) in args.iter().zip(&self.0.domain) {
            assert_eq!(arg.0.sort, *sort, "function argument sorts must match");
        }
        Bv(Term::new(
            Op::Apply(Arc::clone(&self.0)),
            Sort::Bv(self.0.result_width),
            args,
        ))
    }
}
