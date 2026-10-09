//! Session-owned native Bitwuzla Boolean and bit-vector terms.
//!
//! Native objects never escape the solver. Terms are immutable, owner-tagged
//! IDs, so terms surviving their solver cannot be dereferenced or reused by
//! another session. Solvers are deliberately neither `Send` nor `Sync`.
//!
//! The resource limit counts cooperative termination polls, including polls
//! forwarded to CaDiCaL. It is not a wall-clock timeout or a bound on primitive
//! work inside each preprocessing pass, bit-blast or model computation.

#[allow(unsafe_code)]
mod ffi;

use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

/// Failure to construct or use a native solver session.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Invalid input detected before calling the native library.
    #[error("invalid Bitwuzla input: {0}")]
    InvalidInput(&'static str),
    /// A term belongs to another native solver session.
    #[error("Bitwuzla term belongs to another solver session")]
    CrossSession,
    /// No more unique solver ownership IDs can be allocated.
    #[error("Bitwuzla solver ownership IDs exhausted")]
    OwnerLimit,
    /// Native validation or a caught native exception.
    #[error("Bitwuzla: {0}")]
    Native(String),
}

/// Immutable ID of a term owned by one solver session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Term {
    owner: u64,
    id: u64,
}

/// Primitive Boolean or fixed-width bit-vector operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Kind {
    /// Boolean negation.
    Not = 0,
    /// Boolean conjunction; empty arguments denote true.
    And = 1,
    /// Boolean disjunction; empty arguments denote false.
    Or = 2,
    /// Equality of two terms with the same sort.
    Eq = 3,
    /// Conditional with a Boolean condition and equally sorted branches.
    Ite = 4,
    /// Modular bit-vector addition.
    Add = 5,
    /// Modular bit-vector subtraction.
    Sub = 6,
    /// Modular bit-vector multiplication.
    Mul = 7,
    /// SMT-LIB unsigned division, including its defined division by zero.
    Udiv = 8,
    /// SMT-LIB signed division, including its defined division by zero.
    Sdiv = 9,
    /// Unsigned remainder.
    Urem = 10,
    /// Signed remainder; the result follows the dividend's sign.
    Srem = 11,
    /// Unsigned less than.
    Ult = 12,
    /// Unsigned less than or equal.
    Ule = 13,
    /// Unsigned greater than.
    Ugt = 14,
    /// Unsigned greater than or equal.
    Uge = 15,
    /// Signed less than.
    Slt = 16,
    /// Signed less than or equal.
    Sle = 17,
    /// Signed greater than.
    Sgt = 18,
    /// Signed greater than or equal.
    Sge = 19,
    /// Bitwise conjunction.
    BvAnd = 20,
    /// Bitwise disjunction.
    BvOr = 21,
    /// Bitwise exclusive disjunction.
    BvXor = 22,
    /// Bitwise complement.
    BvNot = 23,
    /// Left shift by an equally wide unsigned bit-vector amount.
    Shl = 24,
    /// Logical right shift.
    Lshr = 25,
    /// Arithmetic right shift.
    Ashr = 26,
    /// Zero extension by the single supplied index.
    ZeroExtend = 27,
    /// Inclusive bit extraction; indices are `[high, low]`.
    Extract = 28,
}

/// Result of the latest satisfiability check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The asserted formula is satisfiable.
    Sat,
    /// The asserted formula is unsatisfiable.
    Unsat,
    /// The solver did not decide; neither branch may be discarded.
    Unknown {
        /// The configured cooperative termination poll budget was reached.
        exhausted: bool,
        /// Number of termination polls during this check.
        polls: u64,
    },
}

/// A single-threaded native Bitwuzla session and its immutable terms.
pub struct Solver {
    native: ffi::Session,
    owner: u64,
}

/// Thread-safe one-way stop signal, independent of the solver's term storage.
/// It may outlive the originating solver; a signal never affects another solver.
pub struct Interrupt {
    native: ffi::Interrupt,
}
impl Interrupt {
    /// Ask Bitwuzla and its SAT backend to stop at their next termination poll.
    pub fn interrupt(&self) {
        self.native.interrupt();
    }
}

impl Solver {
    /// Obtain a shareable cancellation signal without moving the native solver.
    pub fn interrupt_handle(&self) -> Result<Interrupt, Error> {
        self.native
            .interrupt_handle()
            .map(|native| Interrupt { native })
    }
    /// Create a session with a positive per-check termination poll budget.
    ///
    /// Model generation is enabled, the SAT backend is CaDiCaL, and all
    /// wall-clock limits remain disabled. Each `check` resets the poll count.
    pub fn new(rlimit: u32) -> Result<Self, Error> {
        if rlimit == 0 {
            return Err(Error::InvalidInput("rlimit must be positive"));
        }
        let owner = NEXT_OWNER
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| Error::OwnerLimit)?;
        Ok(Self {
            native: ffi::Session::new(owner, rlimit)?,
            owner,
        })
    }

    /// Create a Boolean constant.
    pub fn bool(&mut self, value: bool) -> Result<Term, Error> {
        let id = self.native.bool(value)?;
        Ok(self.term(id))
    }

    /// Create a bit-vector constant from hexadecimal digits.
    ///
    /// Optional `0x` and `#x` prefixes are accepted. Native validation rejects
    /// values that do not fit the positive width.
    pub fn bv(&mut self, width: u32, hex: &str) -> Result<Term, Error> {
        if width == 0 {
            return Err(Error::InvalidInput("bit-vector width must be positive"));
        }
        let hex = hex
            .strip_prefix("0x")
            .or_else(|| hex.strip_prefix("#x"))
            .unwrap_or(hex);
        if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::InvalidInput(
                "expected hexadecimal bit-vector digits",
            ));
        }
        let id = self.native.bv(width, hex)?;
        Ok(self.term(id))
    }

    /// Create a fresh variable; `None` denotes Bool, `Some(width)` a bit-vector.
    ///
    /// The name is an explanatory label, not an interning key. Reuse the
    /// returned term to refer to the same variable again.
    pub fn variable(&mut self, width: Option<u32>, name: &str) -> Result<Term, Error> {
        if width == Some(0) {
            return Err(Error::InvalidInput("bit-vector width must be positive"));
        }
        let id = self.native.variable(width.unwrap_or(0), name)?;
        Ok(self.term(id))
    }

    /// Apply a primitive operation after validating all term owners.
    ///
    /// Native validation also checks arity, sorts, widths and indexed bounds.
    pub fn apply(&mut self, kind: Kind, args: &[Term], indices: &[u32]) -> Result<Term, Error> {
        let args = args
            .iter()
            .map(|term| self.reference(term))
            .collect::<Result<Vec<_>, _>>()?;
        let id = self.native.apply(kind as u32, &args, indices)?;
        Ok(self.term(id))
    }

    /// Add a Boolean assertion and invalidate the preceding model.
    pub fn assert(&mut self, term: &Term) -> Result<(), Error> {
        let term = self.reference(term)?;
        self.native.assert(term)
    }

    /// Decide the current assertions with a fresh cooperative poll budget.
    pub fn check(&mut self) -> Result<Status, Error> {
        self.native.check()
    }

    /// Read a model value after SAT, as `#b` or `#x` digits.
    ///
    /// Boolean values are returned as `#b0` or `#b1`. Adding an assertion or a
    /// non-SAT check makes the previous model unavailable.
    pub fn model(&mut self, term: &Term) -> Result<String, Error> {
        let term = self.reference(term)?;
        self.native.model(term)
    }

    fn term(&self, id: u64) -> Term {
        Term {
            owner: self.owner,
            id,
        }
    }

    fn reference(&self, term: &Term) -> Result<ffi::TermRef, Error> {
        if term.owner != self.owner {
            return Err(Error::CrossSession);
        }
        Ok(ffi::TermRef {
            owner: term.owner,
            id: term.id,
        })
    }
}

#[cfg(test)]
mod tests;
