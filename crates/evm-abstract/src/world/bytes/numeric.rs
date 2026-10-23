//! Sound numeric evaluation of the shared foundation content graph.
//!
//! This is an upper-bound evaluator, not a second content DAG or alias solver.
//! Unknown aliasing joins both alternatives; reads retain their original terms.

use super::conditional::Context;
#[cfg(test)]
use super::{constant, zero};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{ByteStateId, ByteStateKind, ByteView, ExprId, ExprKind},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

mod full_image;
mod packing;
mod slot;
pub(in crate::world) use slot::{slot, slot_with_payment};
mod word;
pub(in crate::world) use word::{read_word_with_payment, word_expression_with_payment};
pub(in crate::world) mod payment;
use super::RangeError;
use crate::resource::WorkBudget;
use payment::{ReadPayment, ReadResult};

type Facts<N = crate::domain::NumericValue> = BTreeMap<ExprId, EvmValue<N>>;

pub(in crate::world) fn scalar<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    facts: &Facts<N>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    scalar_in(
        expression,
        &Context::<N, D>::new(facts, None, domain),
        domain,
    )
}

pub(in crate::world) fn scalar_in<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    scalar_in_paid(expression, context, domain, &mut ReadPayment::unpaid())
        .unwrap_or_else(|_| EvmValue::top())
}

pub(in crate::world) fn scalar_with_payment<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    facts: &Facts<N>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> ReadResult<EvmValue<N>> {
    let context = Context::new_with_payment(facts, None, domain, budget)?;
    scalar_in_with_payment(expression, &context, domain, budget)
}
pub(in crate::world) fn scalar_in_with_payment<
    N: WordElement,
    D: EvmNumericAlgebra<Element = N>,
>(
    expression: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> ReadResult<EvmValue<N>> {
    scalar_in_paid(expression, context, domain, &mut ReadPayment::new(budget))
}
pub(in crate::world) fn scalar_in_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    expression: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<EvmValue<N>> {
    payer.context(context)?;
    payer.reserve(expression.work_size().saturating_add(1))?;
    if let Some(v) = context.value_paid(expression, domain, payer)? {
        if let Some(word) = expression.as_constant() {
            let literal = payer.literal(word, domain)?;
            let joined = payer.join(&v, &literal, context, domain)?;
            let numeric = payer.literal_numeric(word, domain)?;
            return Ok(joined.with_numeric(numeric));
        }
        if let ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } = expression.kind()
        {
            let c = scalar_in_paid(condition, context, domain, payer)?;
            let selected = if !payer.may_nonzero(&c)? {
                Some((else_value, false))
            } else if !payer.may_zero(&c)? {
                Some((then_value, true))
            } else {
                None
            };
            if let Some((value, nonzero)) = selected {
                let branch = context.branch_paid(condition, nonzero, payer)?;
                let selected = scalar_in_paid(value, &branch, domain, payer)?;
                if let Some(exact) = payer.singleton(&selected)?
                    && payer.contains(&v, exact)?
                {
                    let out = payer.copy(&v)?;
                    return Ok(out.with_numeric(payer.literal_numeric(exact, domain)?));
                }
            }
        }
        return payer.copy(&v);
    }
    if let Some(word) = expression.as_constant() {
        return payer.literal(word, domain);
    }
    match expression.kind() {
        ExprKind::Operation { opcode, args } => {
            let mut values = Vec::new();
            for arg in args {
                values.push(scalar_in_paid(arg, context, domain, payer)?);
            }
            payer.apply(*opcode, &values, context, domain, false)
        }
        ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } => {
            let c = scalar_in_paid(condition, context, domain, payer)?;
            if !payer.may_nonzero(&c)? {
                let branch = context.branch_paid(condition, false, payer)?;
                scalar_in_paid(else_value, &branch, domain, payer)
            } else if !payer.may_zero(&c)? {
                let branch = context.branch_paid(condition, true, payer)?;
                scalar_in_paid(then_value, &branch, domain, payer)
            } else {
                let branch = context.branch_paid(condition, true, payer)?;
                let left = scalar_in_paid(then_value, &branch, domain, payer)?;
                let branch = context.branch_paid(condition, false, payer)?;
                let right = scalar_in_paid(else_value, &branch, domain, payer)?;
                payer.join(&left, &right, context, domain)
            }
        }
        _ => payer.top(domain),
    }
}

pub(in crate::world) fn numeric_domain<'a, D: EvmNumericAlgebra>(
    domain: AlgebraRef<'a, D>,
) -> AlgebraRef<'a, D> {
    domain.without_relations()
}

pub(in crate::world) fn attach<N: WordElement>(
    value: EvmValue<N>,
    expression: ExprId,
) -> EvmValue<N> {
    if let Some(v) = expression.as_constant() {
        // Constant folding in the content model is a proof. Keep source metadata.
        value.with_numeric(N::constant(v))
    } else {
        value.with_expression(expression)
    }
}

// Only memoize pure evaluations in one immutable operand environment. These
// limits bound temporary retention independently of logical array length.
const WORD_CACHE_ENTRIES: usize = 32;
const WORD_CACHE_WORK: usize = 8 * 1024;

// Continuations borrow the admitted content graph. Values move between tasks;
// no numeric carrier or branch cache is copied to retain a host stack frame.
enum ReadTask<'s, N: WordElement> {
    Byte(&'s ByteStateId, ExprId),
    Word(&'s ByteStateId, ExprId),
    View(&'s ByteView, ExprId),
    WordByte(&'s ExprId, U256),
    WordBytes(&'s ByteStateId, U256),
    DiscardThen(&'s ByteStateId, ExprId),
    Pad(Box<EvmValue<N>>, U256),
    Value(Box<EvmValue<N>>),
    Zero,
    UnknownByte,
    Choices(ReadChoices<'s>, bool),
    Join,
    First,
    Pack,
    Branch {
        condition: &'s ExprId,
        nonzero: bool,
        state: &'s ByteStateId,
        index: ExprId,
        word: bool,
    },
    EndBranch,
}

enum ChoiceKind<'s> {
    Word(&'s ByteStateId, &'s ExprId),
    WholeWord(&'s ByteStateId),
    Copy(&'s ByteStateId, &'s ByteView),
    View(&'s ByteView),
}

// Keep only the three finite operands, never their Cartesian product. Each
// next alternative is scheduled after the previous paid evaluation and join.
struct ReadChoices<'s> {
    kind: ChoiceKind<'s>,
    operands: [Vec<U256>; 3],
    position: [usize; 3],
}

impl<'s> ReadChoices<'s> {
    fn task<N: WordElement>(
        kind: ChoiceKind<'s>,
        operands: &[crate::world::Candidates<'_>],
    ) -> ReadTask<'s, N> {
        ReadTask::Choices(
            Self {
                kind,
                // Unused axes have one neutral alternative. Keep only the
                // finite operands; next() streams their ordered product.
                operands: std::array::from_fn(|i| {
                    operands.get(i).map_or_else(
                        || vec![U256::ZERO],
                        |values| values.iter().copied().collect(),
                    )
                }),
                position: [0; 3],
            },
            false,
        )
    }

    fn next<N: WordElement>(&mut self) -> Option<ReadTask<'s, N>> {
        let [a, b, c] = std::array::from_fn(|i| self.operands[i].get(self.position[i]).copied());
        let (a, b, c) = (a?, b?, c?);
        for i in (0..3).rev() {
            self.position[i] += 1;
            if self.position[i] < self.operands[i].len() || i == 0 {
                break;
            }
            self.position[i] = 0;
        }
        Some(match self.kind {
            ChoiceKind::Word(previous, value) => {
                if a >= b && a - b < U256::from(32) {
                    ReadTask::WordByte(value, a - b)
                } else {
                    ReadTask::Byte(previous, ExprId::constant(a))
                }
            }
            ChoiceKind::WholeWord(state) => ReadTask::WordBytes(state, a),
            ChoiceKind::Copy(previous, source) => {
                if a >= b && a - b < c {
                    ReadTask::View(source, ExprId::constant(a - b))
                } else {
                    ReadTask::Byte(previous, ExprId::constant(a))
                }
            }
            ChoiceKind::View(view) => {
                if a >= c {
                    ReadTask::Zero
                } else {
                    b.checked_add(a).map_or(ReadTask::Zero, |index| {
                        ReadTask::Byte(&view.state, ExprId::constant(index))
                    })
                }
            }
        })
    }
}

/// A batch of reads using exactly the same immutable facts and numeric policy.
/// Cached values keep all metadata and never escape this evaluator's lifetime.
pub(super) struct Reader<
    'a,
    N: WordElement = crate::domain::NumericValue,
    D: EvmNumericAlgebra<Element = N> = crate::domain::ProductAlgebra,
> {
    context: Context<'a, N, D>,
    domain: AlgebraRef<'a, D>,
    word_bytes: BTreeMap<ExprId, EvmValue<N>>,
    retained_work: usize,
}

impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Reader<'a, N, D> {
    pub(super) fn new(facts: &'a Facts<N>, domain: AlgebraRef<'a, D>) -> Self {
        Self::with_context(Context::<N, D>::new(facts, None, domain), domain)
    }

    pub(super) fn with_context(context: Context<'a, N, D>, domain: AlgebraRef<'a, D>) -> Self {
        Self {
            context,
            domain: numeric_domain(domain),
            word_bytes: BTreeMap::new(),
            retained_work: 0,
        }
    }

    /// Inspect this Reader's bounded cache without changing its evaluation.
    #[cfg(test)]
    pub(super) fn cache_stats(&self) -> (usize, usize) {
        (self.word_bytes.len(), self.retained_work)
    }

    pub(in crate::world) fn with_payment(
        facts: &'a Facts<N>,
        conditional: Option<&'a super::conditional::ConditionalFacts<N>>,
        domain: AlgebraRef<'a, D>,
        budget: &mut WorkBudget,
    ) -> ReadResult<Self> {
        Ok(Self::with_context(
            Context::new_with_payment(facts, conditional, domain, budget)?,
            domain,
        ))
    }
    #[cfg(test)]
    fn word_byte_union(&mut self, expression: &ExprId) -> EvmValue<N> {
        self.word_byte_union_paid(expression, &mut ReadPayment::unpaid())
            .unwrap_or_else(|_| EvmValue::top())
    }
    fn word_byte_union_paid(
        &mut self,
        expression: &ExprId,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<EvmValue<N>> {
        payer.context(&self.context)?;
        payer.reserve(
            expression
                .work_size()
                .saturating_add(self.word_bytes.len())
                .saturating_add(1),
        )?;
        payer.expression_lookup(self.word_bytes.keys(), expression)?;
        if let Some(value) = self.word_bytes.get(expression) {
            return payer.copy(value);
        }
        let scalar = scalar_in_paid(expression, &self.context, self.domain, payer)?;
        let top_image = self.domain.byte_image_candidate(scalar.numeric())
            && payer.is_top(&scalar, self.domain)?
            && !self.domain.spec().relations().enabled;
        let complete = if top_image {
            true
        } else if full_image::candidate(scalar.numeric(), self.domain) {
            if payer.is_paid() {
                payer.reserve(32 * 256)?;
            } else if !self.context.pay_byte_image(self.domain) {
                payer.context(&self.context)?;
                return Err(crate::resource::CloneError::Work.into());
            }
            let complete = full_image::complete(scalar.numeric(), self.domain);
            payer.context(&self.context)?;
            complete
        } else {
            false
        };
        let value = if complete {
            let scalar = if top_image {
                scalar
            } else {
                scalar.with_numeric(payer.top_numeric(self.domain)?)
            };
            let literal = payer.literal(U256::ZERO, self.domain)?;
            let value = payer.apply(
                opcode::BYTE,
                &[literal, scalar],
                &self.context,
                self.domain,
                false,
            )?;
            #[cfg(test)]
            tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.0 += 1);
            value
        } else {
            let mut out = None;
            for j in 0..32 {
                let literal = payer.literal(U256::from(j), self.domain)?;
                let operand = payer.copy(&scalar)?;
                let byte = payer.apply(
                    opcode::BYTE,
                    &[literal, operand],
                    &self.context,
                    self.domain,
                    false,
                )?;
                #[cfg(test)]
                tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.0 += 1);
                #[cfg(test)]
                let joins = out.is_some();
                out = Some(join_paid(out, byte, self.domain, &self.context, payer)?);
                #[cfg(test)]
                if joins {
                    tests::WORD_BYTE_UNION_CALLS.update(|calls| calls.1 += 1);
                }
            }
            out.expect("32 bytes")
        };
        payer.context(&self.context)?;
        let work = expression.work_size().saturating_add(value.work_size());
        if self.word_bytes.len() < WORD_CACHE_ENTRIES
            && work <= WORD_CACHE_WORK.saturating_sub(self.retained_work)
        {
            payer.reserve(work.saturating_add(self.word_bytes.len()).saturating_add(1))?;
            let cached = payer.copy(&value)?;
            self.word_bytes.insert(expression.clone(), cached);
            self.retained_work += work;
        }
        Ok(value)
    }
    pub(super) fn read(&mut self, state: &ByteStateId, index: &ExprId) -> EvmValue<N> {
        self.evaluate(ReadTask::Byte(state, index.clone()))
    }
    pub(in crate::world) fn read_with_payment(
        &mut self,
        state: &ByteStateId,
        index: &ExprId,
        budget: &mut WorkBudget,
    ) -> ReadResult<EvmValue<N>> {
        let mut payer = ReadPayment::new(budget);
        payer.reserve(index.work_size().saturating_add(1))?;
        self.evaluate_paid(ReadTask::Byte(state, index.clone()), &mut payer)
    }
    fn evaluate(&mut self, task: ReadTask<'_, N>) -> EvmValue<N> {
        self.evaluate_paid(task, &mut ReadPayment::unpaid())
            .unwrap_or_else(|_| EvmValue::top())
    }
    fn evaluate_paid(
        &mut self,
        task: ReadTask<'_, N>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<EvmValue<N>> {
        let mut tasks = vec![task];
        let mut values = Vec::new();
        let mut branches: Vec<Self> = Vec::new();
        while let Some(task) = tasks.pop() {
            let reader = branches.last_mut().unwrap_or(&mut *self);
            payer.context(&reader.context)?;
            payer.reserve(16)?;
            let result = match task {
                ReadTask::Byte(state, index) => {
                    reader.read_step(state, &index, false, &mut tasks, payer)?
                }
                ReadTask::Word(state, index) => {
                    reader.read_step(state, &index, true, &mut tasks, payer)?
                }
                ReadTask::View(view, index) => reader.view_step(view, &index, &mut tasks, payer)?,
                ReadTask::WordByte(value, index) => {
                    let literal = payer.literal(index, reader.domain)?;
                    let value = scalar_in_paid(value, &reader.context, reader.domain, payer)?;
                    Some(payer.apply(
                        opcode::BYTE,
                        &[literal, value],
                        &reader.context,
                        reader.domain,
                        true,
                    )?)
                }
                ReadTask::WordBytes(state, offset) => {
                    tasks.push(ReadTask::Pack);
                    for j in (0..32).rev() {
                        tasks.push(
                            offset
                                .checked_add(U256::from(j))
                                .map_or(ReadTask::Zero, |i| {
                                    ReadTask::Byte(state, ExprId::constant(i))
                                }),
                        );
                    }
                    None
                }
                ReadTask::DiscardThen(state, index) => {
                    values.pop().expect("discarded copy candidate");
                    tasks.push(ReadTask::Byte(state, index));
                    None
                }
                ReadTask::Pad(length, relative) => {
                    let value = values.pop().expect("view byte");
                    Some(if relative < payer.bounds(&length)?.0 {
                        value
                    } else {
                        let zero = payer.literal(U256::ZERO, reader.domain)?;
                        payer.join(&value, &zero, &reader.context, reader.domain)?
                    })
                }
                ReadTask::Value(value) => Some(*value),
                ReadTask::Zero => Some(payer.literal(U256::ZERO, reader.domain)?),
                ReadTask::UnknownByte => Some(payer.unknown_byte(reader.domain)?),
                ReadTask::Choices(mut choices, seen) => {
                    if let Some(next) = choices.next() {
                        tasks.push(ReadTask::Choices(choices, true));
                        tasks.push(if seen {
                            ReadTask::Join
                        } else {
                            ReadTask::First
                        });
                        tasks.push(next);
                        None
                    } else if seen {
                        None
                    } else if matches!(choices.kind, ChoiceKind::WholeWord(_)) {
                        Some(payer.top(reader.domain)?)
                    } else {
                        Some(payer.unknown_byte(reader.domain)?)
                    }
                }
                ReadTask::First => {
                    let value = values.pop().expect("first alternative");
                    Some(payer.copy(&value)?)
                }
                ReadTask::Join => {
                    let right = values.pop().expect("right alternative");
                    let left = values.pop().expect("left alternative");
                    Some(payer.join(&left, &right, &reader.context, reader.domain)?)
                }
                ReadTask::Pack => {
                    let bytes = values.split_off(values.len() - 32);
                    Some(reader.pack_bytes_paid(bytes, payer)?)
                }
                ReadTask::Branch {
                    condition,
                    nonzero,
                    state,
                    index,
                    word,
                } => {
                    let branch = Self::with_context(
                        reader.context.branch_paid(condition, nonzero, payer)?,
                        reader.domain,
                    );
                    branches.push(branch);
                    tasks.push(ReadTask::EndBranch);
                    tasks.push(if word {
                        ReadTask::Word(state, index)
                    } else {
                        ReadTask::Byte(state, index)
                    });
                    None
                }
                ReadTask::EndBranch => {
                    branches.pop().expect("active branch");
                    None
                }
            };
            if let Some(value) = result {
                values.push(value);
            }
        }
        payer.context(&self.context)?;
        Ok(values.pop().expect("completed read"))
    }

    fn read_step<'s>(
        &mut self,
        state: &'s ByteStateId,
        index: &ExprId,
        word: bool,
        tasks: &mut Vec<ReadTask<'s, N>>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<EvmValue<N>>> {
        let domain = self.domain;
        let context = self.context.clone();
        let state = demanded_state_paid(
            state,
            index,
            if word { 32 } else { 1 },
            &context,
            domain,
            payer,
        )?;
        let value = match state.kind() {
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let c = scalar_in_paid(condition, &context, domain, payer)?;
                let branch = |nonzero, state| ReadTask::Branch {
                    condition,
                    nonzero,
                    state,
                    index: index.clone(),
                    word,
                };
                if !payer.may_zero(&c)? {
                    tasks.push(branch(true, then_state));
                } else if !payer.may_nonzero(&c)? {
                    tasks.push(branch(false, else_state));
                } else {
                    tasks.push(ReadTask::Join);
                    tasks.push(branch(false, else_state));
                    tasks.push(branch(true, then_state));
                }
                return Ok(None);
            }
            _ if word => return self.word_step(state, index, &context, tasks, payer),
            ByteStateKind::Zero => {
                scalar_in_paid(&ExprId::constant(U256::ZERO), &context, domain, payer)?
            }
            ByteStateKind::Exact(bytes) => {
                let index = scalar_in_paid(index, &context, domain, payer)?;
                if let Some(indices) = payer.candidates(&index)? {
                    let mut out = None;
                    for i in indices.iter() {
                        let byte = usize::try_from(*i)
                            .ok()
                            .and_then(|i| bytes.get(i))
                            .copied()
                            .unwrap_or(0);
                        let value = scalar_in_paid(
                            &ExprId::constant(U256::from(byte)),
                            &context,
                            domain,
                            payer,
                        )?;
                        out = Some(join_paid(out, value, domain, &context, payer)?);
                    }
                    match out {
                        Some(value) => value,
                        None => payer.literal(U256::ZERO, domain)?,
                    }
                } else if payer.bounds(&index)?.0 >= U256::from(bytes.len()) {
                    payer.literal(U256::ZERO, domain)?
                } else {
                    payer.unknown_byte(domain)?
                }
            }
            ByteStateKind::Input {
                length, bindings, ..
            } => {
                let i = scalar_in_paid(index, &context, domain, payer)?;
                let len = scalar_in_paid(length, &context, domain, payer)?;
                if payer.bounds(&i)?.0 >= payer.bounds(&len)?.1 {
                    return Ok(Some(payer.literal(U256::ZERO, domain)?));
                }
                let cell = if let Some(indices) = payer.candidates(&i)? {
                    let mut out = None;
                    for index in indices.iter() {
                        payer.reserve(bindings.len().saturating_mul(8).saturating_add(1))?;
                        let cell = if let Some(binding) =
                            bindings.iter().find(|b| b.index == *index)
                        {
                            let retained = context.has_binding_value_paid(&binding.value, payer)?;
                            let value = scalar_in_paid(&binding.value, &context, domain, payer)?;
                            payer.bind(value, retained, &binding.numeric, &context, domain)?
                        } else {
                            payer.unknown_byte(domain)?
                        };
                        out = Some(join_paid(out, cell, domain, &context, payer)?);
                    }
                    match out {
                        Some(value) => value,
                        None => payer.unknown_byte(domain)?,
                    }
                } else {
                    payer.unknown_byte(domain)?
                };
                if payer.bounds(&i)?.1 < payer.bounds(&len)?.0 {
                    cell
                } else {
                    let zero = payer.literal(U256::ZERO, domain)?;
                    payer.join(&cell, &zero, &context, domain)?
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                let value = scalar_in_paid(value, &context, domain, payer)?;
                let mask = payer.literal(U256::from(255), domain)?;
                let val = payer.apply(opcode::AND, &[value, mask], &context, domain, true)?;
                if payer.expression_equal(index, offset)? {
                    return Ok(Some(val));
                }
                let i = scalar_in_paid(index, &context, domain, payer)?;
                let at = scalar_in_paid(offset, &context, domain, payer)?;
                if let (Some(i), Some(at)) = (payer.singleton(&i)?, payer.singleton(&at)?) {
                    if i == at {
                        return Ok(Some(val));
                    }
                    tasks.push(ReadTask::Byte(previous, index.clone()));
                    return Ok(None);
                }
                if disjoint_paid(&i, &at, payer)? {
                    tasks.push(ReadTask::Byte(previous, index.clone()));
                } else {
                    tasks.push(ReadTask::Join);
                    tasks.push(ReadTask::Value(Box::new(val)));
                    tasks.push(ReadTask::Byte(previous, index.clone()));
                }
                return Ok(None);
            }
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                let i = scalar_in_paid(index, &context, domain, payer)?;
                let at = scalar_in_paid(offset, &context, domain, payer)?;
                if let (Some(indices), Some(offsets)) =
                    (payer.candidates(&i)?, payer.candidates(&at)?)
                {
                    payer.reserve(i.work_size().saturating_add(at.work_size()))?;
                    tasks.push(ReadChoices::task(
                        ChoiceKind::Word(previous, value),
                        &[indices, offsets],
                    ));
                    return Ok(None);
                }
                let (ilo, ihi) = payer.bounds(&i)?;
                let (alo, ahi) = payer.bounds(&at)?;
                if ihi < alo
                    || ahi
                        .checked_add(U256::from(32))
                        .is_some_and(|end| ilo >= end)
                {
                    tasks.push(ReadTask::Byte(previous, index.clone()));
                    return Ok(None);
                }
                let val = self.word_byte_union_paid(value, payer)?;
                tasks.push(ReadTask::Join);
                tasks.push(ReadTask::Value(Box::new(val)));
                tasks.push(ReadTask::Byte(previous, index.clone()));
                return Ok(None);
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                let i = scalar_in_paid(index, &context, domain, payer)?;
                let dst = scalar_in_paid(destination, &context, domain, payer)?;
                let len = scalar_in_paid(length, &context, domain, payer)?;
                if let (Some(indices), Some(destinations), Some(lengths)) = (
                    payer.candidates(&i)?,
                    payer.candidates(&dst)?,
                    payer.candidates(&len)?,
                ) {
                    payer.reserve(
                        i.work_size()
                            .saturating_add(dst.work_size())
                            .saturating_add(len.work_size()),
                    )?;
                    tasks.push(ReadChoices::task(
                        ChoiceKind::Copy(previous, source),
                        &[indices, destinations, lengths],
                    ));
                    return Ok(None);
                }
                if payer.expression_equal(index, destination)? && !payer.may_zero(&len)? {
                    tasks.push(ReadTask::View(source, ExprId::constant(U256::ZERO)));
                    return Ok(None);
                }
                let (ilo, ihi) = payer.bounds(&i)?;
                let (dlo, dhi) = payer.bounds(&dst)?;
                let (llo, lhi) = payer.bounds(&len)?;
                if ihi < dlo || dhi.checked_add(lhi).is_some_and(|end| ilo >= end) {
                    tasks.push(ReadTask::Byte(previous, index.clone()));
                    return Ok(None);
                }
                if let (Some(i), Some(dst)) = (payer.singleton(&i)?, payer.singleton(&dst)?)
                    && i >= dst
                {
                    let delta = i - dst;
                    if delta < llo {
                        tasks.push(ReadTask::View(source, ExprId::constant(delta)));
                    } else if delta >= lhi {
                        tasks.push(ReadTask::DiscardThen(previous, index.clone()));
                        tasks.push(ReadTask::View(source, ExprId::constant(delta)));
                    } else {
                        tasks.push(ReadTask::Join);
                        tasks.push(ReadTask::Byte(previous, index.clone()));
                        tasks.push(ReadTask::View(source, ExprId::constant(delta)));
                    }
                    return Ok(None);
                }
                tasks.push(ReadTask::Join);
                tasks.push(ReadTask::UnknownByte);
                tasks.push(ReadTask::Byte(previous, index.clone()));
                return Ok(None);
            }
            ByteStateKind::Slice { source } => {
                tasks.push(ReadTask::View(source, index.clone()));
                return Ok(None);
            }
        };
        payer.context(&context)?;
        Ok(Some(value))
    }
    fn view_step<'s>(
        &mut self,
        view: &'s ByteView,
        relative: &ExprId,
        tasks: &mut Vec<ReadTask<'s, N>>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<EvmValue<N>>> {
        let context = self.context.clone();
        let domain = self.domain;
        let rel = scalar_in_paid(relative, &context, domain, payer)?;
        let off = scalar_in_paid(&view.offset, &context, domain, payer)?;
        let len = scalar_in_paid(&view.length, &context, domain, payer)?;
        if let (Some(relatives), Some(offsets), Some(lengths)) = (
            payer.candidates(&rel)?,
            payer.candidates(&off)?,
            payer.candidates(&len)?,
        ) {
            payer.reserve(
                rel.work_size()
                    .saturating_add(off.work_size())
                    .saturating_add(len.work_size()),
            )?;
            tasks.push(ReadChoices::task(
                ChoiceKind::View(view),
                &[relatives, offsets, lengths],
            ));
            return Ok(None);
        }
        if payer.bounds(&rel)?.0 >= payer.bounds(&len)?.1 {
            return Ok(Some(payer.literal(U256::ZERO, domain)?));
        }
        if let (Some(rel), Some(off)) = (payer.singleton(&rel)?, payer.singleton(&off)?) {
            let Some(i) = off.checked_add(rel) else {
                return Ok(Some(payer.literal(U256::ZERO, domain)?));
            };
            tasks.push(ReadTask::Pad(Box::new(len), rel));
            tasks.push(ReadTask::Byte(&view.state, ExprId::constant(i)));
            return Ok(None);
        }
        Ok(Some(payer.unknown_byte(domain)?))
    }
}

pub(super) fn read<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    state: &ByteStateId,
    index: &ExprId,
    facts: &Facts<N>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    Reader::<N, D>::new(facts, domain).read(state, index)
}

impl<N: WordElement, D: EvmNumericAlgebra<Element = N>> Reader<'_, N, D> {
    pub(super) fn word(&mut self, state: &ByteStateId, offset: &ExprId) -> EvmValue<N> {
        self.evaluate(ReadTask::Word(state, offset.clone()))
    }

    pub(in crate::world) fn word_with_payment(
        &mut self,
        state: &ByteStateId,
        offset: &ExprId,
        budget: &mut WorkBudget,
    ) -> ReadResult<EvmValue<N>> {
        let mut payer = ReadPayment::new(budget);
        payer.reserve(offset.work_size().saturating_add(1))?;
        self.evaluate_paid(ReadTask::Word(state, offset.clone()), &mut payer)
    }
    fn word_step<'s>(
        &self,
        state: &'s ByteStateId,
        offset: &ExprId,
        context: &Context<'_, N, D>,
        tasks: &mut Vec<ReadTask<'s, N>>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<Option<EvmValue<N>>> {
        let domain = self.domain;
        if let ByteStateKind::WriteWord {
            offset: written,
            value,
            ..
        } = state.kind()
            && super::reads::same_offset_paid(written, offset, context, domain, payer)?
            && super::reads::nonwrapping_word_paid(offset, context, domain, payer)?
        {
            return Ok(Some(scalar_in_paid(value, context, domain, payer)?));
        }
        let forwarded = match state.kind() {
            ByteStateKind::Slice { source } => {
                super::reads::view_word_offset_paid(source, offset, context, domain, payer)?
                    .map(|at| (&source.state, at))
            }
            ByteStateKind::Copy {
                destination,
                source,
                length,
                ..
            } => super::reads::copy_word_offset_paid(
                destination,
                source,
                length,
                offset,
                context,
                domain,
                payer,
            )?
            .map(|at| (&source.state, at)),
            _ => None,
        };
        if let Some((source, at)) = forwarded {
            tasks.push(ReadTask::Word(source, at));
            return Ok(None);
        }
        let offsets = scalar_in_paid(offset, context, domain, payer)?;
        let Some(candidates) = payer.candidates(&offsets)? else {
            return Ok(Some(if matches!(state.kind(), ByteStateKind::Zero) {
                scalar_in_paid(&ExprId::constant(U256::ZERO), context, domain, payer)?
            } else {
                payer.top(domain)?
            }));
        };
        payer.reserve(offsets.work_size())?;
        tasks.push(ReadChoices::task(
            ChoiceKind::WholeWord(state),
            &[candidates],
        ));
        Ok(None)
    }
    fn pack_bytes_paid(
        &self,
        bytes: Vec<EvmValue<N>>,
        payer: &mut ReadPayment<'_>,
    ) -> ReadResult<EvmValue<N>> {
        let context = &self.context;
        let domain = self.domain;
        if let Some(value) = packing::pack_paid(&bytes, domain, context, payer)? {
            return Ok(value);
        }
        payer.context(context)?;
        let mut parts = Vec::new();
        for (j, b) in bytes.into_iter().enumerate() {
            let literal = payer.literal(U256::from((31 - j) * 8), domain)?;
            parts.push(payer.apply(opcode::SHL, &[literal, b], context, domain, false)?);
        }
        while parts.len() > 1 {
            let mut next = Vec::new();
            for pair in parts.chunks(2) {
                next.push(if pair.len() == 2 {
                    payer.apply(opcode::OR, pair, context, domain, false)?
                } else {
                    payer.copy(&pair[0])?
                });
            }
            parts = next;
        }
        Ok(parts.pop().expect("word"))
    }
}

// The symbolic constructor has already checked retention/cancellation. Numeric
// evaluation follows the same concrete proof iteratively, so a long disjoint
// prefix does not consume the host stack or reconstruct the word byte by byte.
pub(super) fn demanded_state_paid<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    state: &'a ByteStateId,
    index: &ExprId,
    width: usize,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<&'a ByteStateId> {
    let Some(start) = index.as_constant() else {
        return Ok(state);
    };
    let end = start.saturating_add(U256::from(width - 1));
    let mut cursor = state;
    payer.context(context)?;
    loop {
        payer.reserve(1)?;
        let (previous, failure) = {
            let payment = std::cell::RefCell::new(&mut *payer);
            let failure = std::cell::RefCell::new(None);
            let previous = cursor.prior_to_disjoint_write_with_bound(start, end, &|destination| {
                if failure.borrow().is_some() {
                    return None;
                }
                let mut payment = payment.borrow_mut();
                match context.write_lower_bound_paid(destination, domain, &mut payment) {
                    Ok(bound) => bound,
                    Err(error) => {
                        *failure.borrow_mut() = Some(error);
                        None
                    }
                }
            });
            (previous, failure.into_inner())
        };
        if let Some(error) = failure {
            return Err(error);
        }
        payer.context(context)?;
        let Some(previous) = previous else { break };
        cursor = previous;
    }
    Ok(cursor)
}
fn join_paid<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    out: Option<EvmValue<N>>,
    value: EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    context: &Context<'_, N, D>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<EvmValue<N>> {
    payer.context(context)?;
    match out {
        Some(old) => payer.join(&old, &value, context, domain),
        None => payer.copy(&value),
    }
}
pub(in crate::world) fn disjoint_paid<N: WordElement>(
    a: &EvmValue<N>,
    b: &EvmValue<N>,
    payer: &mut ReadPayment<'_>,
) -> ReadResult<bool> {
    let (alo, ahi) = payer.bounds(a)?;
    let (blo, bhi) = payer.bounds(b)?;
    if ahi < blo || bhi < alo {
        return Ok(true);
    }
    if payer.is_paid() {
        payer.reserve(
            a.numeric()
                .work_size()
                .saturating_add(b.numeric().work_size()),
        )?;
    }
    let out = match (a.constants(), b.constants()) {
        (Some(a), Some(b)) => {
            payer.reserve(a.len().saturating_add(b.len()).saturating_mul(64))?;
            a.is_disjoint(b)
        }
        _ => false,
    };
    payer.check()?;
    Ok(out)
}
pub(in crate::world) fn candidates_with_payment<'a, N: WordElement>(
    value: &'a EvmValue<N>,
    budget: &mut WorkBudget,
) -> ReadResult<Option<crate::world::Candidates<'a>>> {
    ReadPayment::new(budget).candidates(value)
}
pub(in crate::world) fn contains_with_payment<N: WordElement>(
    value: &EvmValue<N>,
    word: U256,
    budget: &mut WorkBudget,
) -> ReadResult<bool> {
    ReadPayment::new(budget).contains(value, word)
}
pub(in crate::world) fn nonzero_with_payment<N: WordElement>(
    value: &EvmValue<N>,
    budget: &mut WorkBudget,
) -> ReadResult<bool> {
    ReadPayment::new(budget).may_nonzero(value)
}

#[cfg(test)]
mod payer_tests;
#[cfg(test)]
mod payment_tests;
#[cfg(test)]
mod stack_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod windows;
