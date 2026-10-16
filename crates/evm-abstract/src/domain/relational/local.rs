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
    AbstractValue, Domain, FiniteConstantSet, NumericValue,
    facts::{Fact, FactError, FactLattice},
    symbolic::{ExprId, ExprKind},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

use keys::KeyMap;

mod keys;
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
}
impl Work<'_> {
    fn charge(&mut self, amount: usize) -> LocalResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(Stop::Pending(QueryReason::Cancelled));
        }
        let remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or(Stop::Pending(QueryReason::ExpressionLimit))?;
        self.prepay(amount)?;
        self.remaining = remaining;
        Ok(())
    }
    fn prepay(&mut self, amount: usize) -> LocalResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(Stop::Pending(QueryReason::Cancelled));
        }
        if let Some(ledger) = self.ledger.as_mut() {
            // Optional precision may stop without exhausting the shared ledger.
            // The later charge is still transactional if cancellation races.
            if !ledger.can_afford(amount) || !ledger.charge(amount) {
                return Err(Stop::Pending(if self.cancellation.is_cancelled() {
                    QueryReason::Cancelled
                } else {
                    QueryReason::ExpressionLimit
                }));
            }
        }
        Ok(())
    }
    fn same_expression(&mut self, left: &ExprId, right: &ExprId) -> LocalResult<bool> {
        self.charge(left.comparison_work(right))?;
        Ok(left == right)
    }
    fn order_expression(
        &mut self,
        left: &ExprId,
        right: &ExprId,
    ) -> LocalResult<std::cmp::Ordering> {
        self.charge(left.comparison_work(right))?;
        Ok(left.cmp(right))
    }
}

/// `None` is proved bottom; a numeric result covers every remaining alternative.
/// A candidate is checked against the original facts, never imported as a fact.
pub(super) fn project(
    state: &RelationState,
    expression: Option<&ExprId>,
    candidate: Option<U256>,
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
    // Local steps and key operations pay their actual bounded traversal.
    // Numeric operations retain their separate scalar-domain reservations.
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation,
        ledger,
    };
    let mut engine = Engine::new(&mut work, limits, Domain::default());
    // Value projection trusts retained guarantees. Feasibility additionally
    // compares their immediate definitions without rebuilding the shared F.
    engine.check_definitions = expression.is_none();
    let run = (|| {
        for atom in &state.constraints {
            engine.atom(atom)?;
        }
        engine.path(&state.path)?;
        // At most one bounded inverse pass; incompleteness only loses precision.
        for atom in &state.constraints {
            engine.validate(atom)?;
        }
        engine.memo.clear(engine.work)?;
        engine.terms.clear(engine.work)?;
        let Some(expression) = expression else {
            return Ok(NumericValue::top());
        };
        let value = engine.eval(expression)?;
        if let Some(candidate) = candidate
            && (!value.contains(candidate)
                || engine.same(expression, &ExprId::constant(candidate))? == Some(false))
        {
            return Err(Stop::Bottom);
        }
        Ok(value)
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
    parents: KeyMap<ExprId>,
    unequal: Vec<(ExprId, ExprId)>,
    facts: KeyMap<NumericValue>,
    memo: KeyMap<NumericValue>,
    terms: KeyMap<ExprId>,
    check_definitions: bool,
}
impl<'a, 'ledger> Engine<'a, 'ledger> {
    fn new(work: &'a mut Work<'ledger>, limits: &'a RelationLimits, domain: Domain) -> Self {
        Self {
            work,
            limits,
            domain,
            parents: KeyMap::default(),
            unequal: Vec::new(),
            facts: KeyMap::default(),
            memo: KeyMap::default(),
            terms: KeyMap::default(),
            check_definitions: false,
        }
    }

    fn root(&mut self, expression: &ExprId) -> LocalResult<ExprId> {
        self.work.charge(1)?;
        let mut root = expression.clone();
        while let Some(parent) = self.parents.get(&root, self.work)? {
            self.work.charge(1)?;
            root = parent.clone();
        }
        Ok(root)
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

    fn known(&mut self, expression: &ExprId) -> LocalResult<NumericValue> {
        let root = self.root(expression)?;
        if let Some(value) = root.as_constant() {
            return Ok(NumericValue::constant(value));
        }
        if let Some(value) = self.facts.get(&root, self.work)? {
            self.work.prepay(value.work_size())?;
            return Ok(value.clone());
        }
        Ok(match root.kind() {
            ExprKind::Input(input)
                if matches!(
                    input.symbol,
                    crate::domain::identity::Symbol::To
                        | crate::domain::identity::Symbol::Caller
                        | crate::domain::identity::Symbol::Origin
                        | crate::domain::identity::Symbol::Coinbase
                ) =>
            {
                NumericValue::unknown_address()
            }
            ExprKind::Operation { opcode, .. }
                if matches!(
                    *opcode,
                    opcode::EQ
                        | opcode::ISZERO
                        | opcode::LT
                        | opcode::GT
                        | opcode::SLT
                        | opcode::SGT
                ) =>
            {
                NumericValue::unsigned_range(U256::ZERO, U256::ONE).unwrap()
            }
            _ => NumericValue::top(),
        })
    }

    fn learn(&mut self, expression: &ExprId, value: &NumericValue) -> LocalResult<()> {
        self.work.charge(1)?;
        let root = self.root(expression)?;
        let old = self.known(&root)?;
        let next = self.meet(&old, value)?;
        if old != next {
            self.facts.insert(root, next, self.work)?;
        }
        Ok(())
    }

    fn equal(&mut self, left: &ExprId, right: &ExprId) -> LocalResult<()> {
        self.work.charge(1)?;
        let a = self.root(left)?;
        let b = self.root(right)?;
        if self.work.same_expression(&a, &b)? {
            return Ok(());
        }
        if self.same(&a, &b)? == Some(false) {
            return Err(Stop::Bottom);
        }
        let av = self.known(&a)?;
        let bv = self.known(&b)?;
        let value = self.meet(&av, &bv)?;
        // Roots strictly decrease, including for x == f(x); no recursive rewrite.
        let (root, child) = if self.work.order_expression(&a, &b)? == std::cmp::Ordering::Less {
            (a, b)
        } else {
            (b, a)
        };
        self.facts.remove(&child, self.work)?;
        self.parents.insert(child, root.clone(), self.work)?;
        self.facts.insert(root, value, self.work)?;
        Ok(())
    }

    fn same(&mut self, left: &ExprId, right: &ExprId) -> LocalResult<Option<bool>> {
        let a = self.root(left)?;
        let b = self.root(right)?;
        if self.work.same_expression(&a, &b)? {
            return Ok(Some(true));
        }
        for index in 0..self.unequal.len() {
            self.work.charge(1)?;
            let (x, y) = self.unequal[index].clone();
            let x = self.root(&x)?;
            let y = self.root(&y)?;
            if (self.work.same_expression(&x, &a)? && self.work.same_expression(&y, &b)?)
                || (self.work.same_expression(&x, &b)? && self.work.same_expression(&y, &a)?)
            {
                return Ok(Some(false));
            }
        }
        let av = self.known(&a)?;
        let bv = self.known(&b)?;
        if let (Some(a), Some(b)) = (av.singleton(), bv.singleton()) {
            return Ok(Some(a == b));
        }
        if (av.nonzero && bv.singleton() == Some(U256::ZERO))
            || (bv.nonzero && av.singleton() == Some(U256::ZERO))
        {
            return Ok(Some(false));
        }
        let (al, au) = av.interval().unsigned_bounds();
        let (bl, bu) = bv.interval().unsigned_bounds();
        if au < bl || bu < al || av.bits.meet(&bv.bits).is_none() {
            return Ok(Some(false));
        }
        Ok(None)
    }

    fn import(&mut self, expression: &ExprId, fact: Fact) -> LocalResult<()> {
        let known = self.known(expression)?;
        let input = AbstractValue::from_numeric(known);
        self.work.prepay(self.domain.refinement_work(&input, 1))?;
        let mut lattice = FactLattice::new(self.domain.spec().fact_limit());
        lattice.insert(fact)?;
        let value = crate::domain::reduce::import(&input, &lattice)?;
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
                } else if self.same(&args[0], &args[1])? == Some(true) {
                    Err(Stop::Bottom)
                } else {
                    self.work.charge(1)?;
                    self.unequal.push((args[0].clone(), args[1].clone()));
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
        if self.check_definitions {
            for expression in atom.expressions() {
                self.validate_definition(expression)?;
                if let ExprKind::Operation { args, .. } = expression.kind() {
                    for arg in args {
                        self.validate_definition(arg)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_definition(&mut self, expression: &ExprId) -> LocalResult<()> {
        self.work.charge(1)?;
        let known = self.known(expression)?;
        if known.is_top() {
            return Ok(());
        }
        // A proof check must inspect the immediate definition. In particular,
        // neither an imported result nor EQ's own unequal pair can establish
        // that definition's value. Operands still use the shared original F.
        let defined = match expression.kind() {
            ExprKind::Operation { opcode, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.eval(arg))
                    .collect::<LocalResult<Vec<_>>>()?;
                self.apply(*opcode, &args)?
            }
            ExprKind::SlotRead { state, key } => self.slot(state, key)?,
            ExprKind::ByteRead { state, index } => self.byte(state, index)?,
            // word_term() may forward its own imported exact guarantee. The
            // direct byte read checks concrete content independently instead.
            ExprKind::WordRead { view, offset } => self.word(view, offset)?,
            ExprKind::Keccak { input } => self.hash(input)?,
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => match self.choose(condition)? {
                Some(true) => self.eval(then_value)?,
                Some(false) => self.eval(else_value)?,
                None => {
                    let yes = self.eval(then_value)?;
                    let no = self.eval(else_value)?;
                    self.join(yes, no)?
                }
            },
            ExprKind::Constant(_) | ExprKind::Input(_) | ExprKind::Fresh(_) => return Ok(()),
        };
        self.meet(&known, &defined)?;
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
            } if matches!(then_path.kind(), RelationPathKind::False) => self.common(else_path)?,
            RelationPathKind::Ite {
                then_path,
                else_path,
                ..
            } if matches!(else_path.kind(), RelationPathKind::False) => self.common(then_path)?,
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
            } if matches!(then_path.kind(), RelationPathKind::False) => {
                self.direct_truth(selector, false)?;
                self.path(else_path)
            }
            RelationPathKind::Ite {
                selector,
                then_path,
                else_path,
            } if matches!(else_path.kind(), RelationPathKind::False) => {
                self.direct_truth(selector, true)?;
                self.path(then_path)
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
        self.memo.clear(self.work)?;
        self.terms.clear(self.work)?;
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
                    if self.same(&args[0], &args[1])? == Some(true) {
                        return Err(Stop::Bottom);
                    }
                    self.work.charge(1)?;
                    self.unequal.push((args[0].clone(), args[1].clone()));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn inverse(&mut self, expression: &ExprId) -> LocalResult<()> {
        let Some(exact) = self.known(expression)?.singleton() else {
            return Ok(());
        };
        let ExprKind::Operation { opcode: op, args } = expression.kind() else {
            return Ok(());
        };
        // Observed comparison results carry the same direct guard guarantee
        // as assume(). In particular EQ=0 must exclude a later equal target.
        if matches!(*op, opcode::EQ | opcode::ISZERO) {
            if exact > U256::ONE {
                return Err(Stop::Bottom);
            }
            return self.direct_truth(expression, exact != U256::ZERO);
        }
        let a = self.known(&args[0])?.singleton();
        let b = if let Some(arg) = args.get(1) {
            self.known(arg)?.singleton()
        } else {
            None
        };
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

    fn exclude_finite(
        &mut self,
        expression: &ExprId,
        mut value: NumericValue,
    ) -> LocalResult<NumericValue> {
        let Some(constants) = value.constants() else {
            return Ok(value);
        };
        if self.unequal.is_empty() {
            return Ok(value);
        }
        self.work.charge(constants.len())?;
        let mut retained = constants.clone();
        let subject = self.root(expression)?;
        for index in 0..self.unequal.len() {
            self.work.charge(1)?;
            let (left, right) = self.unequal[index].clone();
            let left = self.root(&left)?;
            let right = self.root(&right)?;
            let excluded = if self.work.same_expression(&subject, &left)? {
                self.known(&right)?.singleton()
            } else if self.work.same_expression(&subject, &right)? {
                self.known(&left)?.singleton()
            } else {
                None
            };
            if let Some(excluded) = excluded {
                self.work.charge(retained.len().saturating_add(1))?;
                retained.remove(&excluded);
            }
        }
        // This is set difference over an already bounded numeric component.
        // No candidate is assumed, and no conditional path is selected.
        value.finite = FiniteConstantSet::try_from_values(retained).map_err(|_| Stop::Bottom)?;
        Ok(value)
    }

    fn eval(&mut self, expression: &ExprId) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        let known = self.known(expression)?;
        // Imported exact guarantees also cover compound terms. Reconstructing
        // a whole known word cannot add information; feasibility stays unproved.
        if known.singleton().is_some() {
            return self.exclude_finite(expression, known);
        }
        if let Some(value) = self.memo.get(expression, self.work)? {
            self.work.prepay(value.work_size())?;
            let value = value.clone();
            let value = self.meet(&known, &value)?;
            return self.exclude_finite(expression, value);
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
                    && let Some(equal) = self.same(&left, right.as_ref().expect("binary EQ"))?
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
            ExprKind::WordRead { view, offset } => match self.word_term(view, offset)? {
                Some(forwarded) if !self.work.same_expression(&forwarded, expression)? => {
                    self.eval(&forwarded)?
                }
                _ => self.word(view, offset)?,
            },
            // Equality of hashes follows expression identity; no injectivity or
            // equality of different unknown byte ranges is assumed.
            ExprKind::Keccak { input } => self.hash(input)?,
        };
        let value = self.meet(&known, &numeric)?;
        let value = self.exclude_finite(expression, value)?;
        self.work.prepay(value.work_size())?;
        self.memo
            .insert(expression.clone(), value.clone(), self.work)?;
        Ok(value)
    }
}
