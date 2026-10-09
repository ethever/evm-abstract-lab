//! Solver-independent Boolean/bit-vector queries with private native sessions.
//!
//! Terms belong to this library, never a native solver. Each check lowers the
//! same DAG into the selected provider and releases all native handles before
//! returning. UNKNOWN is distinct from UNSAT. Resource units are provider-specific
//! and never wall-clock time; none of the providers launches a child process.

mod backend;
mod cancellation;
pub use cancellation::{Cancellation, current_cancellation, with_cancellation};
mod bitwuzla;
mod cvc5;
mod term;
mod z3;

use serde::Serialize;
use std::fmt;
pub use term::{Bool, Bv, Context};

/// Available embedded SMT engines. Selection never silently falls back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Z3, using its native per-check resource counter.
    #[default]
    Z3,
    /// Bitwuzla, using cooperative termination-check fuel.
    Bitwuzla,
    /// cvc5, using its native per-query resource counter.
    Cvc5,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Z3 => "z3",
            Self::Bitwuzla => "bitwuzla",
            Self::Cvc5 => "cvc5",
        })
    }
}

impl Provider {
    /// What one unit of the configured allowance measures for this provider.
    pub const fn resource_unit(self) -> &'static str {
        match self {
            Self::Z3 => "z3 resource units",
            Self::Bitwuzla => "termination checks (cooperative)",
            Self::Cvc5 => "cvc5 resource units",
        }
    }
}

/// Default allowance per check; provider counters are not mutually comparable.
pub const DEFAULT_RLIMIT: u32 = 100_000;

/// Failure to obtain an answer does not prove the formula false.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unknown {
    /// The caller explicitly cancelled this query.
    Cancelled,
    /// The selected provider exhausted its non-wall-clock allowance.
    ResourceLimit,
    /// The provider returned UNKNOWN for another or unspecified reason.
    Solver(String),
}

/// A native or encoding error, distinct from a satisfiability answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{provider}: {message}")]
pub struct Error {
    /// Provider that could not execute the request.
    pub provider: Provider,
    /// Original error text from the native binding or checked adapter.
    pub message: String,
}

/// Result of checking all assertions and optionally evaluating one bit-vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// At least one satisfying assignment exists; the requested model value is
    /// returned as an SMT-LIB hexadecimal or binary literal.
    Sat(Option<String>),
    /// No satisfying assignment exists.
    Unsat,
    /// The request remains undecided.
    Unknown(Unknown),
    /// The binding failed; callers must conservatively retain the path.
    Error(Error),
}

/// Check a conjunction with an optional model projection in a private session.
///
/// `rlimit == 0` is rejected rather than interpreted as unlimited. Constructed
/// terms are bounded by the caller; native resource limits govern solving, not
/// every allocation, term-construction operation, or model extraction step.
pub fn check(assertions: &[Bool], value: Option<&Bv>, provider: Provider, rlimit: u32) -> Outcome {
    let cancellation = current_cancellation();
    if cancellation.is_cancelled() {
        return Outcome::Unknown(Unknown::Cancelled);
    }
    if rlimit == 0 {
        return Outcome::Error(Error {
            provider,
            message: "rlimit must be positive".into(),
        });
    }
    let outcome = match provider {
        Provider::Z3 => z3::solve(assertions, value, rlimit),
        Provider::Bitwuzla => bitwuzla::solve(assertions, value, rlimit),
        Provider::Cvc5 => cvc5::solve(assertions, value, rlimit),
    };
    if cancellation.is_cancelled() {
        Outcome::Unknown(Unknown::Cancelled)
    } else {
        outcome
    }
}

#[cfg(test)]
mod tests;
