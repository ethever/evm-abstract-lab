//! Immutable, extensional arrays with checked BV index and element widths.

use super::{Bool, Bv, Context, Op, Sort, Term};
use std::borrow::Borrow;

/// A total array from fixed-width bit-vector indices to bit-vector elements.
///
/// A store produces a new snapshot; earlier arrays and reads remain valid.
/// Fresh arrays are unconstrained. Use a constant array for zero initialization.
#[derive(Clone, Debug)]
pub struct Array(pub(crate) Term);

impl Context {
    /// Allocate an unconstrained array; both widths must be positive.
    pub fn fresh_array(&self, prefix: &str, index_width: u32, element_width: u32) -> Array {
        assert!(index_width > 0, "array index width must be positive");
        assert!(element_width > 0, "array element width must be positive");
        Array(Term::new(
            Op::Variable(self.fresh_name(prefix)),
            Sort::Array {
                index_width,
                element_width,
            },
            vec![],
        ))
    }
}

impl Array {
    /// Construct an array whose every index has `value`; index width must be positive.
    pub fn constant(index_width: u32, value: &Bv) -> Self {
        assert!(index_width > 0, "array index width must be positive");
        Self(Term::new(
            Op::ConstantArray,
            Sort::Array {
                index_width,
                element_width: value.width(),
            },
            vec![value.0.clone()],
        ))
    }

    /// Return the positive index width.
    pub fn index_width(&self) -> u32 {
        let Sort::Array { index_width, .. } = self.0.0.sort else {
            unreachable!("typed array")
        };
        index_width
    }

    /// Return the positive element width.
    pub fn element_width(&self) -> u32 {
        let Sort::Array { element_width, .. } = self.0.0.sort else {
            unreachable!("typed array")
        };
        element_width
    }

    /// Read at an index of the array's index width.
    pub fn select(&self, index: &Bv) -> Bv {
        assert_eq!(
            self.index_width(),
            index.width(),
            "array index widths must match"
        );
        Bv(Term::new(
            Op::Select,
            Sort::Bv(self.element_width()),
            vec![self.0.clone(), index.0.clone()],
        ))
    }

    /// Write one element, checking index and element widths, without changing this array.
    pub fn store(&self, index: &Bv, value: &Bv) -> Self {
        assert_eq!(
            self.index_width(),
            index.width(),
            "array index widths must match"
        );
        assert_eq!(
            self.element_width(),
            value.width(),
            "array element widths must match"
        );
        Self(Term::new(
            Op::Store,
            self.0.0.sort,
            vec![self.0.clone(), index.0.clone(), value.0.clone()],
        ))
    }

    /// Extensional equality; both arrays must have the same index and element widths.
    pub fn eq(&self, other: impl Borrow<Self>) -> Bool {
        let other = other.borrow();
        assert_eq!(self.0.0.sort, other.0.0.sort, "array sorts must match");
        Bool(Term::new(
            Op::Eq,
            Sort::Bool,
            vec![self.0.clone(), other.0.clone()],
        ))
    }
}

impl Bool {
    /// Select between arrays of the same index and element widths.
    pub fn ite_array(&self, yes: &Array, no: &Array) -> Array {
        assert_eq!(
            yes.0.0.sort, no.0.0.sort,
            "conditional array sorts must match"
        );
        Array(Term::new(
            Op::Ite,
            yes.0.0.sort,
            vec![self.0.clone(), yes.0.clone(), no.0.clone()],
        ))
    }
}
