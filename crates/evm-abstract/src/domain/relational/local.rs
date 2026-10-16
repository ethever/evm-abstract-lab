//! Bounded consequence propagation over retained guarantees.
//!
//! A successful pass means only that these rules found no contradiction. It
//! supplies neither a satisfying valuation nor evidence of non-uniqueness.
//! Unknown conditional paths contribute only common guarantees. No valuation
//! or path alternatives are enumerated; determined selectors select one branch.

use super::{
    Constraint, QueryReason, RelationLimits, RelationPath, RelationPathKind, RelationState,
};
use crate::domain::{
    AbstractValue, Domain, NumericValue,
    facts::{Fact, FactError, FactLattice},
    symbolic::{ExprId, ExprKind},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::{BTreeMap, BTreeSet};

mod state;
mod terms;
#[cfg(test)]
mod tests;

#[derive(Debug)]
enum Stop {
    Bottom,
    Pending(QueryReason),
}
impl From<FactError> for Stop {
    fn from(error: FactError) -> Self {
        match error {
            FactError::Contradiction { .. }
            | FactError::EmptyFiniteSet
            | FactError::ConflictingBits
            | FactError::InvalidBounds => Self::Bottom,
            FactError::Capacity { .. } => Self::Pending(QueryReason::ScalarFactLimit),
            other => Self::Pending(QueryReason::ScalarFactError(other)),
        }
    }
}
type LocalResult<T> = Result<T, Stop>;

struct Work<'a> {
    remaining: usize,
    cancellation: analysis_control::Cancellation,
    ledger: Option<&'a mut crate::resource::WorkBudget>,
    key_work: usize,
}
impl Work<'_> {
    fn charge(&mut self, amount: usize) -> LocalResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(Stop::Pending(QueryReason::Cancelled));
        }
        self.prepay(amount.saturating_mul(self.key_work))?;
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or(Stop::Pending(QueryReason::ExpressionLimit))?;
        Ok(())
    }
    fn prepay(&mut self, amount: usize) -> LocalResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(Stop::Pending(QueryReason::Cancelled));
        }
        if self
            .ledger
            .as_mut()
            .is_some_and(|ledger| !ledger.charge(amount))
        {
            return Err(Stop::Pending(QueryReason::ExpressionLimit));
        }
        Ok(())
    }
}

/// `None` is proved bottom; a numeric result covers every remaining alternative.
pub(super) fn project(
    state: &RelationState,
    expression: Option<&ExprId>,
    extra: Option<Constraint>,
    limits: &RelationLimits,
    ledger: Option<&mut crate::resource::WorkBudget>,
) -> Result<Option<NumericValue>, QueryReason> {
    let cancellation = analysis_control::current_cancellation();
    if cancellation.is_cancelled() {
        return Err(QueryReason::Cancelled);
    }
    if !limits.enabled {
        return Err(QueryReason::Disabled);
    }
    if !limits.valid() {
        return Err(QueryReason::Configuration);
    }
    if state.is_bottom() {
        return Ok(None);
    }
    if state.len() > limits.max_constraints {
        return Err(QueryReason::ConstraintLimit);
    }
    if state.work_size() > limits.max_nodes
        || state.path.depth() > limits.max_depth
        || state
            .constraints
            .iter()
            .flat_map(Constraint::expressions)
            .any(|e| e.depth() > limits.max_depth)
        || expression.is_some_and(|e| e.nodes() > limits.max_nodes || e.depth() > limits.max_depth)
    {
        return Err(QueryReason::ExpressionLimit);
    }
    // One structural step prepays a worst-case bounded key scan/comparison.
    // Numeric reduction and finite scans pay their own domain cost separately.
    // An alias comparison may scan all disequalities and follow a decreasing
    // parent chain for each endpoint. BTree lookup depth also grows with the
    // bounded local memo. Reserve both rather than treating a key check as O(1).
    let graph_work = state
        .work_size()
        .saturating_add(expression.map_or(0, ExprId::work_size))
        .saturating_add(1);
    let key_steps = state
        .len()
        .saturating_add(2)
        .max((usize::BITS - graph_work.leading_zeros()) as usize + 2);
    let key_work = graph_work.saturating_mul(key_steps.saturating_mul(key_steps));
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation,
        ledger,
        key_work,
    };
    let mut engine = Engine::new(&mut work, limits, Domain::default());
    let run = (|| {
        for atom in &state.constraints {
            engine.atom(atom)?;
        }
        if let Some(extra) = &extra {
            engine.atom(extra)?;
        }
        engine.path(&state.path)?;
        // At most one bounded inverse pass; incompleteness only loses precision.
        for atom in &state.constraints {
            engine.validate(atom)?;
        }
        if let Some(extra) = &extra {
            engine.validate(extra)?;
        }
        engine.memo.clear();
        engine.terms.clear();
        expression.map_or_else(
            || Ok(NumericValue::top()),
            |expression| engine.eval(expression),
        )
    })();
    match run {
        Ok(value) => Ok(Some(value)),
        Err(Stop::Bottom) => Ok(None),
        Err(Stop::Pending(reason)) => Err(reason),
    }
}

struct Engine<'a, 'ledger> {
    work: &'a mut Work<'ledger>,
    limits: &'a RelationLimits,
    domain: Domain,
    parents: BTreeMap<ExprId, ExprId>,
    unequal: BTreeSet<(ExprId, ExprId)>,
    facts: BTreeMap<ExprId, NumericValue>,
    memo: BTreeMap<ExprId, NumericValue>,
    terms: BTreeMap<ExprId, ExprId>,
}
impl<'a, 'ledger> Engine<'a, 'ledger> {
    fn new(work: &'a mut Work<'ledger>, limits: &'a RelationLimits, domain: Domain) -> Self {
        Self {
            work,
            limits,
            domain,
            parents: BTreeMap::new(),
            unequal: BTreeSet::new(),
            facts: BTreeMap::new(),
            memo: BTreeMap::new(),
            terms: BTreeMap::new(),
        }
    }

    fn root(&self, expression: &ExprId) -> ExprId {
        let mut root = expression;
        while let Some(parent) = self.parents.get(root) {
            root = parent;
        }
        root.clone()
    }

    fn meet(&mut self, left: &NumericValue, right: &NumericValue) -> LocalResult<NumericValue> {
        self.work
            .prepay(left.work_size().saturating_add(right.work_size()))?;
        if left == right || right.is_top() {
            return Ok(left.clone());
        }
        if left.is_top() {
            return Ok(right.clone());
        }
        let cost = self
            .domain
            .refinement_work(&AbstractValue::from_numeric(left.clone()), 1)
            .saturating_add(right.work_size());
        self.work.prepay(cost)?;
        super::intersect_numeric(left, right, self.domain).ok_or(Stop::Bottom)
    }

    fn known(&self, expression: &ExprId) -> NumericValue {
        let root = self.root(expression);
        root.as_constant()
            .map(NumericValue::constant)
            .or_else(|| self.facts.get(&root).cloned())
            .unwrap_or_else(NumericValue::top)
    }

    fn learn(&mut self, expression: &ExprId, value: &NumericValue) -> LocalResult<()> {
        self.work.charge(1)?;
        let root = self.root(expression);
        let old = self.known(&root);
        let next = self.meet(&old, value)?;
        if old != next {
            self.facts.insert(root, next);
        }
        Ok(())
    }

    fn equal(&mut self, left: &ExprId, right: &ExprId) -> LocalResult<()> {
        self.work.charge(1)?;
        let a = self.root(left);
        let b = self.root(right);
        if a == b {
            return Ok(());
        }
        if self.same(&a, &b) == Some(false) {
            return Err(Stop::Bottom);
        }
        let av = self.known(&a);
        let bv = self.known(&b);
        let value = self.meet(&av, &bv)?;
        // Roots strictly decrease, including for x == f(x); no recursive rewrite.
        let (root, child) = if a < b { (a, b) } else { (b, a) };
        self.facts.remove(&child);
        self.parents.insert(child, root.clone());
        self.facts.insert(root, value);
        Ok(())
    }

    fn same(&self, left: &ExprId, right: &ExprId) -> Option<bool> {
        let a = self.root(left);
        let b = self.root(right);
        if a == b {
            return Some(true);
        }
        if self.unequal.iter().any(|(x, y)| {
            let x = self.root(x);
            let y = self.root(y);
            (x == a && y == b) || (x == b && y == a)
        }) {
            return Some(false);
        }
        let av = self.known(&a);
        let bv = self.known(&b);
        if let (Some(a), Some(b)) = (av.singleton(), bv.singleton()) {
            return Some(a == b);
        }
        let (al, au) = av.interval().unsigned_bounds();
        let (bl, bu) = bv.interval().unsigned_bounds();
        if au < bl || bu < al || av.bits.meet(&bv.bits).is_none() {
            return Some(false);
        }
        None
    }

    fn import(&mut self, expression: &ExprId, fact: Fact) -> LocalResult<()> {
        self.work.prepay(
            self.domain
                .refinement_work(&AbstractValue::from_numeric(self.known(expression)), 1),
        )?;
        let mut lattice = FactLattice::new(self.domain.spec().fact_limit());
        lattice.insert(fact)?;
        let value = crate::domain::reduce::import(
            &AbstractValue::from_numeric(self.known(expression)),
            &lattice,
        )?;
        self.learn(expression, value.numeric())
    }

    fn atom(&mut self, atom: &Constraint) -> LocalResult<()> {
        self.work.charge(
            atom.expressions()
                .iter()
                .fold(1usize, |n, e| n.saturating_add(e.work_size())),
        )?;
        match atom {
            Constraint::Equal { left, right } => self.equal(left, right)?,
            Constraint::Truth {
                expression,
                nonzero,
            } => self.direct_truth(expression, *nonzero)?,
            _ => {}
        }
        // Direct scalar translation touches only the atom's immediate subjects,
        // rather than crossing every retained atom with every query subterm.
        let mut subjects = atom.expressions();
        let mut expanded = Vec::new();
        while let Some(subject) = subjects.pop() {
            self.work.charge(1)?;
            expanded.push(subject);
            if let ExprKind::Operation { args, .. } = subject.kind() {
                subjects.extend(args);
            }
        }
        for subject in expanded {
            if let Some(fact) = super::scalar::direct(atom, subject)? {
                self.import(subject, fact)?;
            }
        }
        Ok(())
    }

    fn direct_truth(&mut self, expression: &ExprId, nonzero: bool) -> LocalResult<()> {
        self.work.charge(1)?;
        let mut value = NumericValue::top();
        if nonzero {
            value.nonzero = true;
        } else {
            value = NumericValue::constant(U256::ZERO);
        }
        self.learn(expression, &value)?;
        match expression.kind() {
            ExprKind::Operation {
                opcode: opcode::ISZERO,
                args,
            } => self.direct_truth(&args[0], !nonzero),
            ExprKind::Operation {
                opcode: opcode::EQ,
                args,
            } => {
                if nonzero {
                    self.equal(&args[0], &args[1])
                } else if self.same(&args[0], &args[1]) == Some(true) {
                    Err(Stop::Bottom)
                } else {
                    self.unequal.insert((args[0].clone(), args[1].clone()));
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    fn validate(&mut self, atom: &Constraint) -> LocalResult<()> {
        for expression in atom.expressions() {
            self.work.charge(1)?;
            self.inverse(expression)?;
            // Equality-to-a-constant imports a fact for an operation's result;
            // its operands may be invertible even when the atom itself is EQ.
            if let ExprKind::Operation { args, .. } = expression.kind() {
                for arg in args {
                    self.inverse(arg)?;
                }
            }
        }
        if let Constraint::Truth {
            expression,
            nonzero,
        } = atom
        {
            self.truth(expression, *nonzero)?;
        }
        Ok(())
    }

    fn common(&mut self, path: &RelationPath) -> LocalResult<BTreeSet<Constraint>> {
        self.work.charge(path.nodes().saturating_add(1))?;
        Ok(match path.kind() {
            RelationPathKind::Atoms(atoms) => atoms.iter().cloned().collect(),
            RelationPathKind::And(paths) => {
                let mut out = BTreeSet::new();
                for path in paths {
                    out.extend(self.common(path)?);
                }
                out
            }
            RelationPathKind::Ite {
                then_path,
                else_path,
                ..
            } => {
                let a = self.common(then_path)?;
                let b = self.common(else_path)?;
                a.intersection(&b).cloned().collect()
            }
            _ => BTreeSet::new(),
        })
    }

    fn path(&mut self, path: &RelationPath) -> LocalResult<()> {
        self.work.charge(1)?;
        match path.kind() {
            RelationPathKind::True => Ok(()),
            RelationPathKind::False => Err(Stop::Bottom),
            RelationPathKind::Atoms(atoms) => {
                for atom in atoms {
                    self.atom(atom)?;
                }
                for atom in atoms {
                    self.validate(atom)?;
                }
                Ok(())
            }
            RelationPathKind::And(paths) => {
                for path in paths {
                    self.path(path)?;
                }
                Ok(())
            }
            RelationPathKind::Ite {
                selector,
                then_path,
                else_path,
            } => match self.choose(selector)? {
                Some(true) => self.path(then_path),
                Some(false) => self.path(else_path),
                None => {
                    // This is a linear common-fact intersection, not a split or
                    // a search. Branch-specific facts stay under their guards.
                    for atom in self.common(path)? {
                        self.atom(&atom)?;
                        self.validate(&atom)?;
                    }
                    Ok(())
                }
            },
        }
    }

    fn truth(&mut self, expression: &ExprId, nonzero: bool) -> LocalResult<()> {
        self.work.charge(1)?;
        self.memo.clear();
        self.terms.clear();
        let value = self.eval(expression)?;
        if (nonzero && !value.may_be_nonzero()) || (!nonzero && !value.may_be_zero()) {
            return Err(Stop::Bottom);
        }
        let mut numeric = NumericValue::top();
        if nonzero {
            numeric.nonzero = true;
        } else {
            numeric = NumericValue::constant(U256::ZERO);
        }
        self.learn(expression, &numeric)?;
        if let ExprKind::Operation { opcode: op, args } = expression.kind() {
            match *op {
                opcode::ISZERO => self.truth(&args[0], !nonzero)?,
                opcode::EQ if nonzero => self.equal(&args[0], &args[1])?,
                opcode::EQ => {
                    if self.same(&args[0], &args[1]) == Some(true) {
                        return Err(Stop::Bottom);
                    }
                    self.unequal.insert((args[0].clone(), args[1].clone()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn inverse(&mut self, expression: &ExprId) -> LocalResult<()> {
        let Some(exact) = self.known(expression).singleton() else {
            return Ok(());
        };
        let ExprKind::Operation { opcode: op, args } = expression.kind() else {
            return Ok(());
        };
        let a = self.known(&args[0]).singleton();
        let b = args.get(1).and_then(|arg| self.known(arg).singleton());
        let inferred = match (*op, a, b) {
            (opcode::NOT, _, _) => Some((&args[0], !exact)),
            (opcode::ADD, _, Some(b)) => Some((&args[0], exact.wrapping_sub(b))),
            (opcode::ADD, Some(a), _) => Some((&args[1], exact.wrapping_sub(a))),
            (opcode::SUB, _, Some(b)) => Some((&args[0], exact.wrapping_add(b))),
            (opcode::SUB, Some(a), _) => Some((&args[1], a.wrapping_sub(exact))),
            (opcode::XOR, _, Some(b)) => Some((&args[0], exact ^ b)),
            (opcode::XOR, Some(a), _) => Some((&args[1], exact ^ a)),
            _ => None,
        };
        if let Some((target, value)) = inferred {
            self.learn(target, &NumericValue::constant(value))?;
        }
        Ok(())
    }

    fn apply(&mut self, op: u8, args: &[NumericValue]) -> LocalResult<NumericValue> {
        self.work.prepay(
            args.iter()
                .fold(1usize, |n, value| n.saturating_add(value.work_size())),
        )?;
        let args = args
            .iter()
            .cloned()
            .map(AbstractValue::from_numeric)
            .collect::<Vec<_>>();
        self.work.prepay(self.domain.operation_work(&args))?;
        Ok(self.domain.apply(op, &args).numeric().clone())
    }

    fn join(&mut self, a: NumericValue, b: NumericValue) -> LocalResult<NumericValue> {
        self.work.prepay(
            a.work_size()
                .saturating_add(b.work_size())
                .saturating_mul(256),
        )?;
        Ok(self
            .domain
            .join(
                &AbstractValue::from_numeric(a),
                &AbstractValue::from_numeric(b),
            )
            .numeric()
            .clone())
    }

    fn eval(&mut self, expression: &ExprId) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        let known = self.known(expression);
        if known.singleton().is_some()
            && matches!(
                expression.kind(),
                ExprKind::Constant(_) | ExprKind::Input(_) | ExprKind::Fresh(_)
            )
        {
            return Ok(known);
        }
        if let Some(value) = self.memo.get(expression).cloned() {
            return self.meet(&known, &value);
        }
        let numeric = match expression.kind() {
            ExprKind::Constant(value) => NumericValue::constant(*value),
            ExprKind::Input(_) | ExprKind::Fresh(_) => known.clone(),
            ExprKind::Operation { opcode: op, args } => {
                let left = self.term(&args[0])?;
                let right = if args.len() > 1 {
                    Some(self.term(&args[1])?)
                } else {
                    None
                };
                if *op == opcode::EQ
                    && let Some(equal) = self.same(&left, right.as_ref().expect("binary EQ"))
                {
                    NumericValue::constant(U256::from(u8::from(equal)))
                } else {
                    let args = args
                        .iter()
                        .map(|arg| self.eval(arg))
                        .collect::<LocalResult<Vec<_>>>()?;
                    self.apply(*op, &args)?
                }
            }
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => {
                let condition = self.eval(condition)?;
                if !condition.may_be_zero() {
                    self.eval(then_value)?
                } else if !condition.may_be_nonzero() {
                    self.eval(else_value)?
                } else {
                    let a = self.eval(then_value)?;
                    let b = self.eval(else_value)?;
                    self.join(a, b)?
                }
            }
            ExprKind::SlotRead { state, key } => self.slot(state, key)?,
            ExprKind::ByteRead { state, index } => self.byte(state, index)?,
            ExprKind::WordRead { view, offset } => self.word(view, offset)?,
            // Equality of hashes follows expression identity; no injectivity or
            // equality of different unknown byte ranges is assumed.
            ExprKind::Keccak { input } => self.hash(input)?,
        };
        let value = self.meet(&known, &numeric)?;
        self.memo.insert(expression.clone(), value.clone());
        Ok(value)
    }
}
