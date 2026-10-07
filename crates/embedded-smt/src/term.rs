//! Typed native-independent DAG. Pointer identities are only query-local memo keys.

use num_bigint::BigUint;
use num_traits::ToPrimitive;
use std::{
    borrow::Borrow,
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sort {
    Bool,
    Bv(u32),
}

#[derive(Clone, Debug)]
pub(crate) enum Op {
    Bool(bool),
    Literal(String),
    Variable(String),
    Eq,
    Not,
    And,
    Or,
    Ite,
    Add,
    Sub,
    Mul,
    Udiv,
    Sdiv,
    Urem,
    Srem,
    Ult,
    Ule,
    Ugt,
    Uge,
    Slt,
    Sle,
    Sgt,
    Sge,
    BvAnd,
    BvOr,
    BvXor,
    BvNot,
    Shl,
    Lshr,
    Ashr,
    ZeroExtend(u32),
    Extract(u32, u32),
}

#[derive(Debug)]
pub(crate) struct Node {
    pub op: Op,
    pub sort: Sort,
    pub args: Vec<Term>,
}

#[derive(Clone, Debug)]
pub(crate) struct Term(pub Arc<Node>);
impl Term {
    fn new(op: Op, sort: Sort, args: Vec<Self>) -> Self {
        Self(Arc::new(Node { op, sort, args }))
    }
    pub fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
}

/// A typed Boolean formula, independent of the selected solver.
#[derive(Clone, Debug)]
pub struct Bool(pub(crate) Term);

/// A nonempty fixed-width bit-vector term, independent of the selected solver.
#[derive(Clone, Debug)]
pub struct Bv(pub(crate) Term);

/// Factory for fresh query-local symbols. Keep one factory for a complete query.
pub struct Context {
    next: Cell<u64>,
    scope: u64,
}
impl Default for Context {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let scope = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("symbol scope space exhausted");
        Self {
            next: Cell::new(0),
            scope,
        }
    }
}
impl Context {
    /// Allocate a fresh bit-vector symbol; width must be positive.
    pub fn fresh_bv(&self, prefix: &str, width: u32) -> Bv {
        assert!(width > 0, "bit-vector width must be positive");
        let next = self.next.get();
        self.next.set(
            next.checked_add(1)
                .expect("query-local symbol space exhausted"),
        );
        Bv(Term::new(
            Op::Variable(format!("{prefix}!{}!{next}", self.scope)),
            Sort::Bv(width),
            vec![],
        ))
    }
}

impl Bv {
    /// Construct a decimal literal if it fits a positive bit width.
    pub fn from_str(width: u32, decimal: &str) -> Option<Self> {
        let value = BigUint::parse_bytes(decimal.as_bytes(), 10)?;
        if width == 0 || value.bits() > u64::from(width) {
            return None;
        }
        Some(Self(Term::new(
            Op::Literal(value.to_str_radix(16)),
            Sort::Bv(width),
            vec![],
        )))
    }
    /// Construct a small literal; panics if the value does not fit the width.
    pub fn from_u64(value: u64, width: u32) -> Self {
        Self::from_str(width, &value.to_string()).expect("literal fits bit-vector width")
    }
    /// Return the term's positive bit width.
    pub fn width(&self) -> u32 {
        match self.0.0.sort {
            Sort::Bv(width) => width,
            Sort::Bool => unreachable!("typed bit-vector"),
        }
    }
    /// Read a syntactic literal when it fits a machine integer.
    pub fn as_u64(&self) -> Option<u64> {
        let Op::Literal(value) = &self.0.0.op else {
            return None;
        };
        BigUint::parse_bytes(value.as_bytes(), 16)?.to_u64()
    }
    /// Read a syntactic literal as unprefixed hexadecimal digits.
    pub fn literal_hex(&self) -> Option<&str> {
        match &self.0.0.op {
            Op::Literal(value) => Some(value),
            _ => None,
        }
    }
    fn binary(&self, other: &Self, op: Op) -> Self {
        assert_eq!(self.width(), other.width(), "bit-vector widths must match");
        Self(Term::new(
            op,
            self.0.0.sort,
            vec![self.0.clone(), other.0.clone()],
        ))
    }
    fn compare(&self, other: &Self, op: Op) -> Bool {
        assert_eq!(self.width(), other.width(), "bit-vector widths must match");
        Bool(Term::new(
            op,
            Sort::Bool,
            vec![self.0.clone(), other.0.clone()],
        ))
    }
    /// Equality between bit-vectors of equal width.
    pub fn eq(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Eq)
    }
    /// Bitwise complement at the original width.
    pub fn bvnot(&self) -> Self {
        Self(Term::new(Op::BvNot, self.0.0.sort, vec![self.0.clone()]))
    }
    /// Extend with zero most-significant bits.
    pub fn zero_ext(&self, extra: u32) -> Self {
        let width = self
            .width()
            .checked_add(extra)
            .expect("bit-vector width overflow");
        Self(Term::new(
            Op::ZeroExtend(extra),
            Sort::Bv(width),
            vec![self.0.clone()],
        ))
    }
    /// Extract an inclusive range of bits, with bit zero least significant.
    pub fn extract(&self, high: u32, low: u32) -> Self {
        assert!(
            low <= high && high < self.width(),
            "valid bit extraction range"
        );
        Self(Term::new(
            Op::Extract(high, low),
            Sort::Bv(high - low + 1),
            vec![self.0.clone()],
        ))
    }
    /// Modular addition; operands must have equal width.
    pub fn bvadd(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Add)
    }
    /// Modular subtraction; operands must have equal width.
    pub fn bvsub(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Sub)
    }
    /// Modular multiplication; operands must have equal width.
    pub fn bvmul(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Mul)
    }
    /// Unsigned SMT bit-vector division; operands must have equal width.
    pub fn bvudiv(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Udiv)
    }
    /// Signed SMT bit-vector division; operands must have equal width.
    pub fn bvsdiv(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Sdiv)
    }
    /// Unsigned SMT bit-vector remainder; operands must have equal width.
    pub fn bvurem(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Urem)
    }
    /// Signed SMT bit-vector remainder; operands must have equal width.
    pub fn bvsrem(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Srem)
    }
    /// Bitwise conjunction; operands must have equal width.
    pub fn bvand(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::BvAnd)
    }
    /// Bitwise disjunction; operands must have equal width.
    pub fn bvor(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::BvOr)
    }
    /// Bitwise exclusive or; operands must have equal width.
    pub fn bvxor(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::BvXor)
    }
    /// Logical shift left; operands must have equal width.
    pub fn bvshl(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Shl)
    }
    /// Logical shift right; operands must have equal width.
    pub fn bvlshr(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Lshr)
    }
    /// Arithmetic shift right; operands must have equal width.
    pub fn bvashr(&self, other: impl Borrow<Self>) -> Self {
        self.binary(other.borrow(), Op::Ashr)
    }
    /// Unsigned less than; operands must have equal width.
    pub fn bvult(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Ult)
    }
    /// Unsigned less than or equal; operands must have equal width.
    pub fn bvule(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Ule)
    }
    /// Unsigned greater than; operands must have equal width.
    pub fn bvugt(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Ugt)
    }
    /// Unsigned greater than or equal; operands must have equal width.
    pub fn bvuge(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Uge)
    }
    /// Signed less than; operands must have equal width.
    pub fn bvslt(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Slt)
    }
    /// Signed less than or equal; operands must have equal width.
    pub fn bvsle(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Sle)
    }
    /// Signed greater than; operands must have equal width.
    pub fn bvsgt(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Sgt)
    }
    /// Signed greater than or equal; operands must have equal width.
    pub fn bvsge(&self, other: impl Borrow<Self>) -> Bool {
        self.compare(other.borrow(), Op::Sge)
    }
}

impl Bool {
    /// Construct a Boolean literal.
    pub fn from_bool(value: bool) -> Self {
        Self(Term::new(Op::Bool(value), Sort::Bool, vec![]))
    }
    /// Logical negation.
    pub fn not(&self) -> Self {
        Self(Term::new(Op::Not, Sort::Bool, vec![self.0.clone()]))
    }
    /// Conjunction; the empty conjunction is true.
    pub fn and(values: &[Self]) -> Self {
        if values.is_empty() {
            return Self::from_bool(true);
        }
        if values.len() == 1 {
            return values[0].clone();
        }
        Self(Term::new(
            Op::And,
            Sort::Bool,
            values.iter().map(|value| value.0.clone()).collect(),
        ))
    }
    /// Disjunction; the empty disjunction is false.
    pub fn or(values: &[Self]) -> Self {
        if values.is_empty() {
            return Self::from_bool(false);
        }
        if values.len() == 1 {
            return values[0].clone();
        }
        Self(Term::new(
            Op::Or,
            Sort::Bool,
            values.iter().map(|value| value.0.clone()).collect(),
        ))
    }
    /// Select between bit-vectors of equal width.
    pub fn ite(&self, yes: &Bv, no: &Bv) -> Bv {
        assert_eq!(
            yes.width(),
            no.width(),
            "conditional branches must have equal widths"
        );
        Bv(Term::new(
            Op::Ite,
            yes.0.0.sort,
            vec![self.0.clone(), yes.0.clone(), no.0.clone()],
        ))
    }
}
