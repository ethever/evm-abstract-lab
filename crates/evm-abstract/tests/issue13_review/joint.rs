//! Exact shared truth cover for this fixture's three reads, four owner counters
//! (201, 202, 300, 101), and two Boolean routes. Admission deliberately rejects
//! other fragments or a nontrivial F instead of approximating them.

use evm_abstract::{
    U256,
    domain::{
        relational::{RelationPathKind, RelationState},
        symbolic::{ExprError, ExprId, ExprKind, ExprLimits},
    },
};
use revm_bytecode::opcode;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
#[path = "joint/tests.rs"]
mod tests;

type Terms = [ExprId; 9];
type Values = [U256; 9];
type Assignment = BTreeMap<u64, U256>;

#[derive(Debug, thiserror::Error)]
pub(super) enum Error {
    #[error(
        "full F must be True: bottom={bottom}, constraints={constraints}, path_true={path_true}"
    )]
    Relation {
        bottom: bool,
        constraints: usize,
        path_true: bool,
    },
    #[error("expression exceeds the unchanged policy: nodes={nodes}, depth={depth}")]
    ExpressionLimit { nodes: usize, depth: usize },
    #[error("{phase} exceeds {limit} unique nodes; no partial certificate")]
    Cap { phase: &'static str, limit: usize },
    #[error("unsupported fixture expression: {0:?}")]
    Unsupported(ExprId),
    #[error("Fresh {0} is observed as a word, not only for truth")]
    WordFresh(u64),
    #[error("{0}")]
    Construction(ExprError),
    #[error("complete assignment did not yield a constant: {0:?}")]
    NotConstant(ExprId),
    #[error("{name}: {values:?}")]
    Control {
        name: &'static str,
        values: Box<Values>,
    },
    #[error("missing common assignment for {0}")]
    Missing(&'static str),
    #[error("original tuple replay differs: expected={expected:?}, actual={actual:?}")]
    Replay {
        expected: Box<Values>,
        actual: Box<Values>,
    },
}

impl From<ExprError> for Error {
    fn from(error: ExprError) -> Self {
        Self::Construction(error)
    }
}

#[derive(Debug)]
pub(super) struct Cover {
    pub proof_nodes: usize,
    pub terminal_vectors: usize,
}

fn require_true_f(relations: &RelationState) -> Result<(), Error> {
    let bottom = relations.is_bottom();
    let constraints = relations.constraints().len();
    let path_true = matches!(relations.path().kind(), RelationPathKind::True);
    if bottom || constraints != 0 || !path_true || !relations.is_empty() {
        return Err(Error::Relation {
            bottom,
            constraints,
            path_true,
        });
    }
    Ok(())
}

fn bounded(term: &ExprId, limits: ExprLimits) -> Result<(), Error> {
    if term.nodes() > limits.max_nodes || term.depth() > limits.max_depth {
        return Err(Error::ExpressionLimit {
            nodes: term.nodes(),
            depth: term.depth(),
        });
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Demand {
    Truth,
    Word,
}

// Demand belongs to the whole view: an ITE used as a guard passes Truth to
// BOTH arms, even when a Fresh is not the immediate condition of an ITE.
// Word operations conservatively demand full words, including ISZERO's input.
fn admit(terms: &Terms, limits: ExprLimits) -> Result<BTreeSet<u64>, Error> {
    let mut pending: Vec<_> = terms.iter().cloned().map(|t| (t, Demand::Word)).collect();
    let mut seen = BTreeSet::new();
    let mut fresh = BTreeSet::new();
    while let Some((term, demand)) = pending.pop() {
        bounded(&term, limits)?;
        if seen.contains(&(term.clone(), demand)) {
            continue;
        }
        if seen.len() >= limits.max_nodes {
            return Err(Error::Cap {
                phase: "whole-view demand census",
                limit: limits.max_nodes,
            });
        }
        seen.insert((term.clone(), demand));
        match term.kind() {
            ExprKind::Constant(_) => {}
            ExprKind::Fresh(id) => {
                if demand == Demand::Word {
                    return Err(Error::WordFresh(*id));
                }
                fresh.insert(*id);
            }
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => pending.extend([
                (condition.clone(), Demand::Truth),
                (then_value.clone(), demand),
                (else_value.clone(), demand),
            ]),
            ExprKind::Operation { opcode: op, args }
                if matches!(
                    *op,
                    opcode::ADD | opcode::EQ | opcode::ISZERO | opcode::AND | opcode::OR
                ) && args.len() == if *op == opcode::ISZERO { 1 } else { 2 } =>
            {
                pending.extend(args.iter().cloned().map(|t| (t, Demand::Word)));
            }
            _ => return Err(Error::Unsupported(term)),
        }
    }
    Ok(fresh)
}

// A memo is local to ONE fixed assignment, shared across all nine components.
// It keys complete ExprIds; no source identity, arm, or word is abstracted away.
fn specialize(
    term: &ExprId,
    assignment: &Assignment,
    limits: ExprLimits,
    memo: &mut BTreeMap<ExprId, ExprId>,
) -> Result<ExprId, Error> {
    bounded(term, limits)?;
    if let Some(result) = memo.get(term) {
        return Ok(result.clone());
    }
    if memo.len() >= limits.max_nodes {
        return Err(Error::Cap {
            phase: "tuple cofactor memo",
            limit: limits.max_nodes,
        });
    }
    let result = match term.kind() {
        ExprKind::Constant(_) => term.clone(),
        ExprKind::Fresh(id) => assignment
            .get(id)
            .copied()
            .map(ExprId::constant)
            .unwrap_or_else(|| term.clone()),
        ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } => {
            let condition = specialize(condition, assignment, limits, memo)?;
            let yes = specialize(then_value, assignment, limits, memo)?;
            let no = specialize(else_value, assignment, limits, memo)?;
            ExprId::ite(condition, yes, no, limits)?
        }
        ExprKind::Operation { opcode: op, args }
            if matches!(
                *op,
                opcode::ADD | opcode::EQ | opcode::ISZERO | opcode::AND | opcode::OR
            ) =>
        {
            let args = args
                .iter()
                .map(|arg| specialize(arg, assignment, limits, memo))
                .collect::<Result<Vec<_>, _>>()?;
            // The public constructor retains Word256 arithmetic/bitwise
            // semantics and folds fully constant operands without a bool cast.
            ExprId::operation(*op, &args, limits)?
        }
        _ => return Err(Error::Unsupported(term.clone())),
    };
    // Descendants may have filled the memo since the entry check.
    if memo.len() >= limits.max_nodes {
        return Err(Error::Cap {
            phase: "tuple cofactor memo",
            limit: limits.max_nodes,
        });
    }
    memo.insert(term.clone(), result.clone());
    Ok(result)
}

fn cofactor(terms: &Terms, assignment: &Assignment, limits: ExprLimits) -> Result<Terms, Error> {
    let mut result = terms.clone();
    let mut memo = BTreeMap::new();
    for (out, term) in result.iter_mut().zip(terms) {
        *out = specialize(term, assignment, limits, &mut memo)?;
    }
    Ok(result)
}

fn constants(terms: &Terms) -> Result<Values, Error> {
    let mut values = [U256::ZERO; 9];
    for (value, term) in values.iter_mut().zip(terms) {
        *value = term
            .as_constant()
            .ok_or_else(|| Error::NotConstant(term.clone()))?;
    }
    Ok(values)
}

fn replay(
    relations: &RelationState,
    original: &Terms,
    assignment: &Assignment,
    expected: Values,
    limits: ExprLimits,
) -> Result<(), Error> {
    require_true_f(relations)?;
    let actual = constants(&cofactor(original, assignment, limits)?)?;
    if actual != expected {
        return Err(Error::Replay {
            expected: Box::new(expected),
            actual: Box::new(actual),
        });
    }
    Ok(())
}

const POSITIVES: [&str; 4] = [
    "joint expected storage and counters",
    "successful call guards have a common assignment",
    "successful guards with expected counters",
    "second call rejected with old memory retained",
];

fn controls(values: Values) -> Result<[bool; 4], Error> {
    let p = values[..3] == [1u64, 1, 2].map(U256::from);
    let c = values[3..7] == [2u64, 1, 0, 0].map(U256::from);
    let one = U256::from(1);
    // The original conjunctions use bitwise AND, so route DATA must be 0/1.
    // The quotient's nonzero 2/MAX controls apply only to admitted guards.
    let boolean_routes = values[7..].iter().all(|v| *v == U256::ZERO || *v == one);
    let q = p && values[7] == one && values[8] == one;
    for (name, valid) in [
        ("route predicates must remain Boolean", boolean_routes),
        (
            "returned values independently determine owner201 counter",
            !p || values[3] == U256::from(2),
        ),
        ("qualified wrong owner counter", !q || c),
        (
            "code/caller owner counter contamination",
            values[5] == U256::ZERO && values[6] == U256::ZERO,
        ),
    ] {
        if !valid {
            return Err(Error::Control {
                name,
                values: Box::new(values),
            });
        }
    }
    Ok([
        p && c,
        q,
        q && c,
        p && values[7] == one && values[8] == U256::ZERO && values[4] == U256::ZERO,
    ])
}

/// Both children of every residual nine-tuple are covered. Exact residual
/// equality makes memo reuse independent of discarded prefix assignments.
/// No successful result is returned until every branch and control completes.
pub(super) fn verify(
    relations: &RelationState,
    original: &Terms,
    limits: ExprLimits,
) -> Result<Cover, Error> {
    require_true_f(relations)?;
    let fresh = admit(original, limits)?;
    let all_zero: Assignment = fresh.iter().map(|id| (*id, U256::ZERO)).collect();
    replay(
        relations,
        original,
        &all_zero,
        [1u64, 1, 2, 2, 1, 0, 0, 1, 1].map(U256::from),
        limits,
    )?;
    let mut pending = vec![(original.clone(), Assignment::new())];
    let mut seen = BTreeSet::new();
    let mut positive = [false; 4];
    let mut terminal_vectors = 0;
    while let Some((tuple, choices)) = pending.pop() {
        if seen.contains(&tuple) {
            continue;
        }
        if seen.len() >= limits.max_nodes {
            return Err(Error::Cap {
                phase: "shared tuple cover",
                limit: limits.max_nodes,
            });
        }
        seen.insert(tuple.clone());
        // BTreeSet<u64> orders actual Fresh IDs, unlike ExprId's fingerprints.
        if let Some(id) = admit(&tuple, limits)?.first().copied() {
            // A decision path has its own depth, even when all component
            // expressions are shallow. Refuse before constructing that branch.
            if choices.len() >= limits.max_depth {
                return Err(Error::Cap {
                    phase: "shared tuple cover depth",
                    limit: limits.max_depth,
                });
            }
            for word in [U256::from(1), U256::ZERO] {
                let branch = cofactor(&tuple, &Assignment::from([(id, word)]), limits)?;
                let mut path = choices.clone();
                path.insert(id, word);
                pending.push((branch, path));
            }
        } else {
            let values = constants(&tuple)?;
            // Replay each terminal representative against the ORIGINAL tuple
            // with one complete shared word map, defaulting unused leaves to 0.
            // Admission proves all nonzero words are equivalent at Fresh uses.
            for nonzero in [U256::from(1), U256::from(2), U256::MAX] {
                let assignment = fresh
                    .iter()
                    .map(|id| {
                        let word = if choices.get(id).is_some_and(|v| *v != U256::ZERO) {
                            nonzero
                        } else {
                            U256::ZERO
                        };
                        (*id, word)
                    })
                    .collect();
                replay(relations, original, &assignment, values, limits)?;
            }
            for (found, holds) in positive.iter_mut().zip(controls(values)?) {
                *found |= holds;
            }
            terminal_vectors += 1;
        }
    }
    for (name, found) in POSITIVES.into_iter().zip(positive) {
        if !found {
            return Err(Error::Missing(name));
        }
    }
    Ok(Cover {
        proof_nodes: seen.len(),
        terminal_vectors,
    })
}
