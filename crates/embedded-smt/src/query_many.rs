//! Joint exclusion proofs over one complete, immutable assertion prefix.
//! A model supplies candidates, never a uniqueness proof. Every SAT exclusion
//! query must supply a complete packed model and change a remaining target.
use crate::{Bool, Bv, Outcome, Provider, QueryBudget, UniqueOutcome};
use num_bigint::BigUint;

/// Maximum number of projections in one joint proof.
pub const MAX_UNIQUE_TARGETS: usize = 16;
/// Maximum total bit width, checked before constructing the packed term.
pub const MAX_UNIQUE_PACKED_WIDTH: u32 = 4096;

/// Admission failures do not make any mathematical claim about the targets.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueryManyError {
    /// Too many targets for a bounded native session.
    #[error("{actual} uniqueness targets exceed the maximum {maximum}", maximum = MAX_UNIQUE_TARGETS)]
    TooManyTargets {
        /// Number supplied by the caller.
        actual: usize,
    },
    /// The sum of the targets' positive widths overflowed u32.
    #[error("packed uniqueness width overflow")]
    PackedWidthOverflow,
    /// The representable sum exceeds the packing bound.
    #[error("packed uniqueness width {actual} exceeds the maximum {maximum}", maximum = MAX_UNIQUE_PACKED_WIDTH)]
    PackedWidthLimit {
        /// Sum of the supplied target widths.
        actual: u32,
    },
}

/// A per-target proof result or a refusal from the existing query ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetedUniqueOutcome {
    /// A mathematical result or the original typed solver/model failure.
    Solver(UniqueOutcome),
    /// Reservation of the next query's original cost was refused.
    Work,
}

/// Prove uniqueness of at most 16 BV projections over the same full conjunction.
///
/// Results preserve target order, including aliases. The caller must construct
/// all targets and definitions before this call; no assertions are sliced. The
/// first check obtains all candidate values in one model. Later checks append
/// one speculative AND(target != first_value), then OR for unresolved targets.
/// A complete AND model proves every target Multiple; its UNSAT or non-cancelled
/// Unknown result proves nothing and falls through to the OR loop. OR UNSAT proves
/// those targets Unique; a complete second model proves only changed targets
/// Multiple. An Unknown in the ordinary OR loop, Error or model failure stops
/// the batch and fills
/// unresolved targets with that reason, retaining already proved Multiple.
///
/// Empty targets do no native work. Two or more targets perform at most N+2
/// checks (18 at the limit); a single target needs at most two. Z3 owns one scoped
/// Context, using a fresh Solver and the
/// unchanged `rlimit` and current cancellation checks for every query. Other
/// providers use their ordinary private-query implementation. Combining DAGs
/// can exhaust an allowance that suffices for individual projections.
pub fn check_unique_many(
    base: &[Bool],
    targets: &[Bv],
    provider: Provider,
    rlimit: u32,
) -> Result<Vec<UniqueOutcome>, QueryManyError> {
    many_inner(base, targets, provider, rlimit, None).map(|outcomes| {
        outcomes
            .into_iter()
            .map(|outcome| match outcome {
                BudgetedUniqueOutcome::Solver(outcome) => outcome,
                BudgetedUniqueOutcome::Work => {
                    unreachable!("unbudgeted queries have no Work lease")
                }
            })
            .collect()
    })
}

/// As [`check_unique_many`], with the caller's unchanged per-query cost.
///
/// Each query consumes one permit before preparing its terms or native Context;
/// a held reservation is reused, never double charged or refunded. Empty calls
/// and excessive target counts do no charging or encoding. For an
/// admitted nonempty count, even a packed-width failure consumes the first
/// permit. First Work refusal creates no Context or packed term. Later refusal
/// retains proved results and marks the rest Work. Captured cancellation on an
/// unpaid reservation remains Work, as in `check_candidates_budgeted`; the
/// ordinary oracle still observes current cancellation for paid queries.
///
/// An attached QueryBudget cache is deliberately unused: no SAT model or local
/// singleton is read from or installed in a mathematical uniqueness cache.
pub fn check_unique_many_budgeted(
    base: &[Bool],
    targets: &[Bv],
    provider: Provider,
    rlimit: u32,
    budget: &mut QueryBudget<'_>,
) -> Result<Vec<BudgetedUniqueOutcome>, QueryManyError> {
    many_inner(base, targets, provider, rlimit, Some(budget))
}

fn pay(budget: &mut Option<&mut QueryBudget<'_>>) -> bool {
    budget.as_deref_mut().is_none_or(QueryBudget::consume)
}

fn many_inner(
    base: &[Bool],
    targets: &[Bv],
    provider: Provider,
    rlimit: u32,
    mut budget: Option<&mut QueryBudget<'_>>,
) -> Result<Vec<BudgetedUniqueOutcome>, QueryManyError> {
    if targets.len() > MAX_UNIQUE_TARGETS {
        return Err(QueryManyError::TooManyTargets {
            actual: targets.len(),
        });
    }
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    // consume() reserves first, including a permit the caller already holds.
    // No packing, width-dependent allocation or private native Context precedes it.
    if !pay(&mut budget) {
        return Ok(vec![BudgetedUniqueOutcome::Work; targets.len()]);
    }
    let width = targets.iter().try_fold(0_u32, |width, target| {
        width
            .checked_add(target.width())
            .ok_or(QueryManyError::PackedWidthOverflow)
    })?;
    if width > MAX_UNIQUE_PACKED_WIDTH {
        return Err(QueryManyError::PackedWidthLimit { actual: width });
    }
    let mut run = || {
        #[cfg(test)]
        if provider == Provider::Z3 {
            crate::query_pair::observe_context();
        }
        solve_many(
            base,
            targets,
            width,
            budget.as_deref_mut(),
            |assertions, packed| {
                let outcome =
                    crate::query_pair::checked_current(assertions, Some(packed), provider, rlimit);
                #[cfg(test)]
                crate::query_pair::observe_finished_query();
                outcome
            },
        )
    };
    Ok(if provider == Provider::Z3 {
        ::z3::with_z3_config(&::z3::Config::new(), run)
    } else {
        run()
    })
}

// This base has no concat primitive. Zero extension, constant shift and OR
// implement concatenation without allocating symbols or losing DAG identities.
// The first target occupies the most significant bits.
fn pack(targets: &[Bv], width: u32) -> Bv {
    let mut packed = targets[0].clone();
    for target in &targets[1..] {
        let next_width = packed.width() + target.width();
        packed = packed
            .zero_ext(target.width())
            .bvshl(Bv::from_u64(u64::from(target.width()), next_width))
            .bvor(target.zero_ext(next_width - target.width()));
    }
    debug_assert_eq!(packed.width(), width);
    packed
}

struct ModelValue {
    number: BigUint,
    literal: Bv,
    printed: String,
}

fn split_model(printed: &str, targets: &[Bv], width: u32) -> Option<Vec<ModelValue>> {
    let (digits, radix, bits) = if let Some(hex) = printed.strip_prefix("#x") {
        (hex, 16, 4)
    } else {
        (printed.strip_prefix("#b")?, 2, 1)
    };
    // Exact literal width matters: a truncated all-zero model is still incomplete.
    if u32::try_from(digits.len()).ok()?.checked_mul(bits)? != width
        || !digits.bytes().all(|digit| match radix {
            16 => digit.is_ascii_hexdigit(),
            _ => matches!(digit, b'0' | b'1'),
        })
    {
        return None;
    }
    let packed = BigUint::parse_bytes(digits.as_bytes(), radix)?;
    let mut remaining = width;
    targets
        .iter()
        .map(|target| {
            remaining = remaining.checked_sub(target.width())?;
            let mask = (BigUint::from(1_u8) << target.width() as usize) - 1_u8;
            let number = (&packed >> remaining as usize) & mask;
            let literal = Bv::from_str(target.width(), &number.to_str_radix(10))?;
            let printed = if target.width().is_multiple_of(4) {
                format!(
                    "#x{:0>digits$}",
                    number.to_str_radix(16),
                    digits = (target.width() / 4) as usize,
                )
            } else {
                format!(
                    "#b{:0>digits$}",
                    number.to_str_radix(2),
                    digits = target.width() as usize,
                )
            };
            Some(ModelValue {
                number,
                literal,
                printed,
            })
        })
        .collect()
}

fn finish(
    results: Vec<Option<BudgetedUniqueOutcome>>,
    unresolved: BudgetedUniqueOutcome,
) -> Vec<BudgetedUniqueOutcome> {
    results
        .into_iter()
        .map(|result| result.unwrap_or_else(|| unresolved.clone()))
        .collect()
}

// The private query seam tests malformed/partial provider outcomes against this
// same state machine, rather than reimplementing the algorithm in a reference.
// The first permit was consumed by many_inner, before packing/Context creation.
fn solve_many(
    base: &[Bool],
    targets: &[Bv],
    width: u32,
    mut budget: Option<&mut QueryBudget<'_>>,
    mut query: impl FnMut(&[Bool], &Bv) -> Outcome,
) -> Vec<BudgetedUniqueOutcome> {
    use BudgetedUniqueOutcome::Solver;
    let packed = pack(targets, width);
    let mut results = vec![None; targets.len()];
    let first = match query(base, &packed) {
        Outcome::Sat(Some(printed)) => match split_model(&printed, targets, width) {
            Some(model) => model,
            None => return finish(results, Solver(UniqueOutcome::ModelUnavailable)),
        },
        Outcome::Sat(None) => return finish(results, Solver(UniqueOutcome::ModelUnavailable)),
        Outcome::Unsat => return finish(results, Solver(UniqueOutcome::Infeasible)),
        Outcome::Unknown(reason) => return finish(results, Solver(UniqueOutcome::Unknown(reason))),
        Outcome::Error(error) => return finish(results, Solver(UniqueOutcome::Error(error))),
    };
    if targets.len() >= 2 {
        if !pay(&mut budget) {
            return finish(results, BudgetedUniqueOutcome::Work);
        }
        let exclusions: Vec<_> = targets
            .iter()
            .zip(&first)
            .map(|(target, model)| target.eq(&model.literal).not())
            .collect();
        let mut assertions = base.to_vec();
        assertions.push(Bool::and(&exclusions));
        match query(&assertions, &packed) {
            Outcome::Sat(Some(printed)) => {
                let Some(second) = split_model(&printed, targets, width) else {
                    return finish(results, Solver(UniqueOutcome::ModelUnavailable));
                };
                // A provider must supply the complete model requested by this
                // very query. Never infer unchanged targets from its SAT flag.
                if first.iter().zip(second).all(|(a, b)| a.number != b.number) {
                    return vec![Solver(UniqueOutcome::Multiple); targets.len()];
                }
                return finish(results, Solver(UniqueOutcome::ModelUnavailable));
            }
            Outcome::Sat(None) => return finish(results, Solver(UniqueOutcome::ModelUnavailable)),
            Outcome::Unsat => {}
            Outcome::Unknown(crate::Unknown::Cancelled) => {
                return finish(
                    results,
                    Solver(UniqueOutcome::Unknown(crate::Unknown::Cancelled)),
                );
            }
            Outcome::Unknown(_) => {}
            Outcome::Error(error) => return finish(results, Solver(UniqueOutcome::Error(error))),
        }
    }
    for _ in 0..targets.len() {
        if results.iter().all(Option::is_some) {
            break;
        }
        if !pay(&mut budget) {
            return finish(results, BudgetedUniqueOutcome::Work);
        }
        let exclusions: Vec<_> = targets
            .iter()
            .zip(&first)
            .zip(&results)
            .filter(|(_, result)| result.is_none())
            .map(|((target, model), _)| target.eq(&model.literal).not())
            .collect();
        let mut assertions = base.to_vec();
        assertions.push(Bool::or(&exclusions));
        match query(&assertions, &packed) {
            Outcome::Unsat => {
                for (result, model) in results.iter_mut().zip(&first) {
                    if result.is_none() {
                        *result = Some(Solver(UniqueOutcome::Unique(model.printed.clone())));
                    }
                }
                break;
            }
            Outcome::Sat(Some(printed)) => {
                let Some(second) = split_model(&printed, targets, width) else {
                    return finish(results, Solver(UniqueOutcome::ModelUnavailable));
                };
                let mut progress = false;
                for ((result, first), second) in results.iter_mut().zip(&first).zip(&second) {
                    if result.is_none() && first.number != second.number {
                        *result = Some(Solver(UniqueOutcome::Multiple));
                        progress = true;
                    }
                }
                if !progress {
                    return finish(results, Solver(UniqueOutcome::ModelUnavailable));
                }
            }
            Outcome::Sat(None) => return finish(results, Solver(UniqueOutcome::ModelUnavailable)),
            Outcome::Unknown(reason) => {
                return finish(results, Solver(UniqueOutcome::Unknown(reason)));
            }
            Outcome::Error(error) => return finish(results, Solver(UniqueOutcome::Error(error))),
        }
    }
    // Each SAT iteration resolves at least one target; any broken invariant
    // remains a model failure rather than a fabricated proof or an extra query.
    finish(results, Solver(UniqueOutcome::ModelUnavailable))
}

#[cfg(test)]
mod tests;
