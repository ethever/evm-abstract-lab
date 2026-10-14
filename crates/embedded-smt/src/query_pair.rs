//! Scoped finite batches: no public callback and no native Solver reuse.
use crate::{Bool, Bv, Error, Outcome, Provider, QueryBudget, Unknown};
use num_bigint::BigUint;

const CONTEXT_QUERIES: usize = 32;
pub(crate) fn checked_current(
    assertions: &[Bool],
    value: Option<&Bv>,
    provider: Provider,
    rlimit: u32,
) -> Outcome {
    if provider == Provider::Z3 {
        #[cfg(test)]
        tests::observe_query();
        crate::checked_query(provider, rlimit, || {
            crate::z3::solve_in_context(&::z3::Context::thread_local(), assertions, value, rlimit)
        })
    } else {
        crate::check(assertions, value, provider, rlimit)
    }
}

/// One candidate query either obtains a solver outcome or is refused by Work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetedOutcome {
    /// Native/encoding outcome, including typed UNKNOWN and errors.
    Solver(Outcome),
    /// The caller's existing ledger refused the next per-query reservation.
    Work,
}
impl BudgetedOutcome {
    fn undecided(&self) -> bool {
        matches!(
            self,
            Self::Work | Self::Solver(Outcome::Unknown(_) | Outcome::Error(_))
        )
    }
}
fn candidates_inner(
    base: &[Bool],
    value: &Bv,
    candidates: &[Bv],
    after: &[Bool],
    provider: Provider,
    rlimit: u32,
    mut budget: Option<&mut QueryBudget<'_>>,
) -> Vec<BudgetedOutcome> {
    let mut outcomes = Vec::new();
    for chunk in candidates.chunks(CONTEXT_QUERIES) {
        // A refused next query does not create even a replacement native Context.
        if budget
            .as_deref_mut()
            .is_some_and(|budget| !budget.reserve())
        {
            outcomes.push(BudgetedOutcome::Work);
            break;
        }
        let mut run = || {
            #[cfg(test)]
            if provider == Provider::Z3 {
                tests::observe_context();
            }
            let mut outcomes = Vec::new();
            for candidate in chunk {
                if budget
                    .as_deref_mut()
                    .is_some_and(|budget| !budget.consume())
                {
                    outcomes.push(BudgetedOutcome::Work);
                    break;
                }
                let outcome = if candidate.width() != value.width() {
                    Outcome::Error(Error {
                        provider,
                        message: "candidate width differs from value".into(),
                    })
                } else {
                    let mut assertions = base.to_vec();
                    assertions.push(value.eq(candidate));
                    assertions.extend_from_slice(after);
                    if let Some(mut query) = budget.as_deref_mut().and_then(QueryBudget::cache) {
                        crate::decided_cache::candidate_current(
                            &assertions,
                            provider,
                            rlimit,
                            &mut query,
                        )
                    } else {
                        checked_current(&assertions, None, provider, rlimit)
                    }
                };
                #[cfg(test)]
                tests::observe_finished_query();
                let outcome = BudgetedOutcome::Solver(outcome);
                let undecided = outcome.undecided();
                outcomes.push(outcome);
                if undecided {
                    break;
                }
            }
            outcomes
        };
        let results = if provider == Provider::Z3 {
            ::z3::with_z3_config(&::z3::Config::new(), run)
        } else {
            run()
        };
        let stopped = results.last().is_some_and(BudgetedOutcome::undecided);
        outcomes.extend(results);
        if stopped {
            break;
        }
    }
    outcomes
}
/// Check base AND value==candidate with a fresh Solver and full allowance each time.
/// Results keep candidate order and stop at the first Unknown/Error. Each chunk
/// of at most 32 checks owns a private Context, without changing the query limits.
pub fn check_candidates(
    base: &[Bool],
    value: &Bv,
    candidates: &[Bv],
    provider: Provider,
    rlimit: u32,
) -> Vec<Outcome> {
    candidates_inner(base, value, candidates, &[], provider, rlimit, None)
        .into_iter()
        .map(|outcome| match outcome {
            BudgetedOutcome::Solver(outcome) => outcome,
            BudgetedOutcome::Work => unreachable!("unbudgeted queries have no Work lease"),
        })
        .collect()
}
/// As check_candidates, preserving charge-before-query and first-Work/Unknown order.
/// The first reservation can be made before preparing the shared query encoder.
pub fn check_candidates_budgeted(
    base: &[Bool],
    value: &Bv,
    candidates: &[Bv],
    after: &[Bool],
    provider: Provider,
    rlimit: u32,
    budget: &mut QueryBudget<'_>,
) -> Vec<BudgetedOutcome> {
    candidates_inner(
        base,
        value,
        candidates,
        after,
        provider,
        rlimit,
        Some(budget),
    )
}

/// Model plus a separate full-conjunction exclusion check; no incremental Solver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniqueOutcome {
    /// No model exists.
    Infeasible,
    /// The second independently bounded query proves no other value exists.
    Unique(String),
    /// At least two values satisfy the conjunction.
    Multiple,
    /// No unique answer can be established.
    Unknown(Unknown),
    /// Native error, preserving its provider and reason.
    Error(Error),
    /// A SAT result did not deliver a parseable value.
    ModelUnavailable,
}
/// Run at most two independent checks inside one private Context.
pub fn check_unique(base: &[Bool], value: &Bv, provider: Provider, rlimit: u32) -> UniqueOutcome {
    let run = || {
        #[cfg(test)]
        if provider == Provider::Z3 {
            tests::observe_context();
        }
        let printed = match checked_current(base, Some(value), provider, rlimit) {
            Outcome::Sat(Some(printed)) => printed,
            Outcome::Sat(None) => return UniqueOutcome::ModelUnavailable,
            Outcome::Unsat => return UniqueOutcome::Infeasible,
            Outcome::Unknown(reason) => return UniqueOutcome::Unknown(reason),
            Outcome::Error(error) => return UniqueOutcome::Error(error),
        };
        let parsed = if let Some(hex) = printed.strip_prefix("#x") {
            BigUint::parse_bytes(hex.as_bytes(), 16)
        } else if let Some(binary) = printed.strip_prefix("#b") {
            BigUint::parse_bytes(binary.as_bytes(), 2)
        } else {
            None
        };
        let Some(model) =
            parsed.and_then(|value_num| Bv::from_str(value.width(), &value_num.to_str_radix(10)))
        else {
            return UniqueOutcome::ModelUnavailable;
        };
        let mut second = base.to_vec();
        second.push(value.eq(model).not());
        match checked_current(&second, None, provider, rlimit) {
            Outcome::Unsat => UniqueOutcome::Unique(printed),
            Outcome::Sat(_) => UniqueOutcome::Multiple,
            Outcome::Unknown(reason) => UniqueOutcome::Unknown(reason),
            Outcome::Error(error) => UniqueOutcome::Error(error),
        }
    };
    if provider == Provider::Z3 {
        ::z3::with_z3_config(&::z3::Config::new(), run)
    } else {
        run()
    }
}

#[cfg(test)]
pub(crate) fn observe_native() {
    tests::observe_native();
}

#[cfg(test)]
pub(crate) mod tests;
