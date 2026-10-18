//! Local read-over-write and same-content rewrites. Every range shortcut needs
//! mathematical non-overflow; unknown overlap keeps the original expression.

use super::{Engine, LocalResult, QueryReason, Stop};
use crate::domain::symbolic::{
    ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprKind, SlotStateId, SlotStateKind,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) fn denied(error: ExprError) -> Stop {
    Stop::Pending(if error == ExprError::Cancelled {
        QueryReason::Cancelled
    } else {
        QueryReason::ExpressionLimit
    })
}

impl Engine<'_, '_> {
    fn operation(&mut self, code: u8, args: &[ExprId]) -> LocalResult<ExprId> {
        self.work.charge(args.len().saturating_add(1))?;
        self.work.prepay(args.iter().fold(1usize, |n, expression| {
            n.saturating_add(expression.work_size())
        }))?;
        ExprId::operation(code, args, self.limits.expressions()).map_err(denied)
    }

    pub(super) fn term(&mut self, expression: &ExprId) -> LocalResult<ExprId> {
        self.work.charge(1)?;
        if let Some(exact) = self.known(expression)?.singleton() {
            return Ok(ExprId::constant(exact));
        }
        if let Some(term) = self.terms.get(expression, self.work)? {
            return Ok(term.clone());
        }
        let term = match expression.kind() {
            ExprKind::Operation { opcode: code, args } => {
                let args = args
                    .iter()
                    .map(|arg| self.term(arg))
                    .collect::<LocalResult<Vec<_>>>()?;
                if *code == opcode::EQ && self.same(&args[0], &args[1])? == Some(true) {
                    ExprId::constant(U256::ONE)
                } else {
                    self.operation(*code, &args)?
                }
            }
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => match self.choose(condition)? {
                Some(true) => self.term(then_value)?,
                Some(false) => self.term(else_value)?,
                None => {
                    let yes = self.term(then_value)?;
                    let no = self.term(else_value)?;
                    ExprId::ite(condition.clone(), yes, no, self.limits.expressions())
                        .map_err(denied)?
                }
            },
            ExprKind::WordRead { view, offset } => self
                .word_term(view, offset)?
                .unwrap_or_else(|| expression.clone()),
            ExprKind::SlotRead { state, key } => {
                let forwarded = self.slot_term(state, key)?;
                if self.work.same_expression(expression, &forwarded)? {
                    forwarded
                } else {
                    self.term(&forwarded)?
                }
            }
            ExprKind::Keccak { input } => {
                let view = self.hash_view(input)?;
                if let Some(hash) = self.literal_hash(&view)? {
                    ExprId::constant(hash)
                } else {
                    if matches!(view.state.kind(), ByteStateKind::Exact(_))
                        && let Some(length) = view
                            .length
                            .as_constant()
                            .and_then(|length| usize::try_from(length).ok())
                            .filter(|length| {
                                length.saturating_mul(view.nodes().saturating_add(1))
                                    <= self.limits.max_nodes
                            })
                    {
                        // Only an unresolved offset remains: known_bytes will
                        // reserve capacity, then stop at its first view check,
                        // before reading a cell or hashing. Pay that attempt
                        // while preserving the normalized symbolic hash identity.
                        self.work.charge(1)?;
                        self.work.prepay(
                            length
                                .checked_add(1)
                                .ok_or(Stop::Pending(QueryReason::ExpressionLimit))?,
                        )?;
                    }
                    // hash_view has already visited every forwarding edge.
                    // Its terminal has no structural COPY/SLICE forwarding
                    // either: those tests are weaker than the same-F rules.
                    // Prepay canonicalize's one final set of EQ checks using
                    // cached comparison bounds, including fingerprint collisions.
                    let comparisons = match view.state.kind() {
                        ByteStateKind::Copy {
                            destination,
                            source,
                            length,
                            ..
                        } => view
                            .offset
                            .comparison_work(destination)
                            .saturating_add(view.length.comparison_work(length))
                            .saturating_add(length.comparison_work(&source.length)),
                        ByteStateKind::Slice { source } => {
                            view.length.comparison_work(&source.length)
                        }
                        _ => 0,
                    };
                    self.work.charge(comparisons.saturating_add(1))?;
                    // Successful materialization was handled by literal_hash;
                    // any failed literal attempt was paid above. No Keccak
                    // computation remains in this constructor call.
                    self.work.prepay(view.nodes())?;
                    ExprId::keccak(view, self.limits.expressions()).map_err(denied)?
                }
            }
            _ => expression.clone(),
        };
        self.terms
            .insert(expression.clone(), term.clone(), self.work)?;
        Ok(term)
    }

    fn slot_term(&mut self, state: &SlotStateId, key: &ExprId) -> LocalResult<ExprId> {
        self.work.charge(1)?;
        let key = self.term(key)?;
        if let SlotStateKind::Store {
            previous,
            key: written,
            value,
        } = state.kind()
        {
            let written = self.term(written)?;
            match self.same(&key, &written)? {
                Some(true) => return self.term(value),
                Some(false) => return self.slot_term(previous, &key),
                None => {}
            }
        }
        self.work
            .charge(state.nodes().saturating_add(key.work_size()))?;
        ExprId::slot_read(state.clone(), key, self.limits.expressions()).map_err(denied)
    }

    fn fits(&mut self, offset: &ExprId, length: &ExprId) -> LocalResult<bool> {
        let (_, maximum) = self.eval(length)?.interval().unsigned_bounds();
        if maximum == U256::ZERO {
            return Ok(true);
        }
        let (_, start) = self.eval(offset)?.interval().unsigned_bounds();
        Ok(start.checked_add(maximum - U256::ONE).is_some())
    }

    fn disjoint(
        &mut self,
        left: &ExprId,
        left_length: &ExprId,
        right: &ExprId,
        right_length: &ExprId,
    ) -> LocalResult<bool> {
        self.work.charge(1)?;
        let (_, ll) = self.eval(left_length)?.interval().unsigned_bounds();
        let (_, rl) = self.eval(right_length)?.interval().unsigned_bounds();
        if ll == U256::ZERO || rl == U256::ZERO {
            return Ok(true);
        }
        let (lo, hi) = self.eval(left)?.interval().unsigned_bounds();
        let (ro, rh) = self.eval(right)?.interval().unsigned_bounds();
        Ok(hi.checked_add(ll - U256::ONE).is_some_and(|end| end < ro)
            || rh.checked_add(rl - U256::ONE).is_some_and(|end| end < lo))
    }

    pub(super) fn hash_view(&mut self, input: &ByteView) -> LocalResult<ByteView> {
        let mut view = input.clone();
        loop {
            self.work.charge(1)?;
            view.offset = self.term(&view.offset)?;
            view.length = self.term(&view.length)?;
            let next = match view.state.kind() {
                ByteStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => self.choose(condition)?.map(|nonzero| {
                    // The scalar operands above were selected under this same
                    // F already; no separate unmetered branch_view walk is needed.
                    ByteView::new(
                        if nonzero {
                            then_state.clone()
                        } else {
                            else_state.clone()
                        },
                        view.offset.clone(),
                        view.length.clone(),
                    )
                }),
                ByteStateKind::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => {
                    // A determined selector may occur independently in these
                    // fields. Compare their selected terms under the same F.
                    let destination = self.term(destination)?;
                    let length = self.term(length)?;
                    let source_length = self.term(&source.length)?;
                    if self.same(&view.offset, &destination)? == Some(true)
                        && self.same(&view.length, &length)? == Some(true)
                        && self.same(&length, &source_length)? == Some(true)
                        && self.fits(&destination, &length)?
                    {
                        Some(source.clone())
                    } else if self.disjoint(&view.offset, &view.length, &destination, &length)? {
                        Some(ByteView::new(
                            previous.clone(),
                            view.offset.clone(),
                            view.length.clone(),
                        ))
                    } else {
                        None
                    }
                }
                ByteStateKind::Slice { source } => {
                    // Like COPY lengths, the saved slice boundary can retain
                    // an ITE whose selected term is exact only under this F.
                    let source_length = self.term(&source.length)?;
                    if self.same(&view.offset, &ExprId::constant(U256::ZERO))? == Some(true)
                        && self.same(&view.length, &source_length)? == Some(true)
                    {
                        Some(source.clone())
                    } else {
                        None
                    }
                }
                ByteStateKind::WriteByte {
                    previous, offset, ..
                }
                | ByteStateKind::WriteWord {
                    previous, offset, ..
                } => {
                    let length = ExprId::constant(U256::from(
                        if matches!(view.state.kind(), ByteStateKind::WriteByte { .. }) {
                            1
                        } else {
                            32
                        },
                    ));
                    if self.disjoint(&view.offset, &view.length, offset, &length)? {
                        Some(ByteView::new(
                            previous.clone(),
                            view.offset.clone(),
                            view.length.clone(),
                        ))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            match next {
                Some(next) => view = next,
                // The loop covers canonical copy/slice forwarding itself, with
                // a charge on every edge. Do not walk the history again here.
                None => return Ok(view),
            }
        }
    }

    pub(super) fn word_term(
        &mut self,
        view: &ByteView,
        relative: &ExprId,
    ) -> LocalResult<Option<ExprId>> {
        self.work.charge(1)?;
        self.work
            .prepay(view.nodes().saturating_add(relative.work_size()))?;
        let original = ExprId::word_read(view.clone(), relative.clone(), self.limits.expressions())
            .map_err(denied)?;
        if let Some(value) = self.known(&original)?.singleton() {
            return Ok(Some(ExprId::constant(value)));
        }
        let Some(relative) = self.exact(relative)? else {
            return Ok(None);
        };
        let length = self.eval(&view.length)?;
        if relative
            .checked_add(U256::from(32))
            .is_none_or(|end| end > length.interval().unsigned_bounds().0)
        {
            return Ok(None);
        }
        let at = if relative == U256::ZERO {
            self.term(&view.offset)?
        } else if let Some(start) = self
            .exact(&view.offset)?
            .and_then(|start| start.checked_add(relative))
        {
            ExprId::constant(start)
        } else {
            return Ok(None);
        };
        if let Some(forwarded) = self.forward_word(&view.state, &at)? {
            return Ok(Some(forwarded));
        }
        // The full requested word fits this view. A canonical 32-byte window
        // keeps direct source-word facts shared with longer copied views.
        ExprId::word_read(
            ByteView::new(view.state.clone(), at, ExprId::constant(U256::from(32))),
            ExprId::constant(U256::ZERO),
            self.limits.expressions(),
        )
        .map(Some)
        .map_err(denied)
    }

    fn relative_byte(&mut self, at: &ExprId, written: &ExprId) -> LocalResult<Option<usize>> {
        if let (Some(at), Some(written)) = (self.exact(at)?, self.exact(written)?) {
            return Ok(written
                .checked_sub(at)
                .filter(|n| *n < U256::from(32))
                .map(|n| n.to::<usize>()));
        }
        if self.eval(at)?.interval().unsigned_bounds().1 > U256::MAX - U256::from(31) {
            return Ok(None);
        }
        if self.same(at, written)? == Some(true) {
            return Ok(Some(0));
        }
        if let ExprKind::Operation {
            opcode: opcode::ADD,
            args,
        } = written.kind()
        {
            for (base, offset) in [(&args[0], &args[1]), (&args[1], &args[0])] {
                if self.same(base, at)? == Some(true)
                    && let Some(offset) = self.exact(offset)?.filter(|n| *n < U256::from(32))
                {
                    return Ok(Some(offset.to::<usize>()));
                }
            }
        }
        Ok(None)
    }

    fn forward_word(&mut self, state: &ByteStateId, at: &ExprId) -> LocalResult<Option<ExprId>> {
        self.work.charge(1)?;
        let word_length = ExprId::constant(U256::from(32));
        match state.kind() {
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                if self.same(at, offset)? == Some(true) {
                    let value = self.term(value)?;
                    if let Some(at) = self.exact(at)? {
                        let available = U256::MAX - at;
                        if available < U256::from(31) {
                            let missing = 31 - available.to::<usize>();
                            return self
                                .operation(
                                    opcode::AND,
                                    &[value, ExprId::constant(U256::MAX << (missing * 8))],
                                )
                                .map(Some);
                        }
                        return Ok(Some(value));
                    }
                    if self.fits(at, &word_length)? {
                        return Ok(Some(value));
                    }
                }
                if self.disjoint(at, &word_length, offset, &word_length)? {
                    self.forward_word(previous, at)
                } else {
                    Ok(None)
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                if let Some(relative) = self.relative_byte(at, offset)?
                    && let Some(old) = self.forward_word(previous, at)?
                {
                    let value = self.term(value)?;
                    return self.overlay_byte(&old, &value, relative).map(Some);
                }
                if self.disjoint(at, &word_length, offset, &ExprId::constant(U256::ONE))? {
                    self.forward_word(previous, at)
                } else {
                    Ok(None)
                }
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                if self.same(at, destination)? == Some(true)
                    && self.eval(length)?.interval().unsigned_bounds().0 >= U256::from(32)
                    && self.fits(at, &word_length)?
                {
                    let offset = ExprId::constant(U256::ZERO);
                    if let Some(term) = self.word_term(source, &offset)? {
                        return Ok(Some(term));
                    }
                    return ExprId::word_read(source.clone(), offset, self.limits.expressions())
                        .map(Some)
                        .map_err(denied);
                }
                if self.disjoint(at, &word_length, destination, length)? {
                    self.forward_word(previous, at)
                } else {
                    Ok(None)
                }
            }
            ByteStateKind::Slice { source } => self.word_term(source, at),
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => match self.choose(condition)? {
                Some(true) => self.forward_word(then_state, at),
                Some(false) => self.forward_word(else_state, at),
                None => {
                    let Some(yes) = self.forward_word(then_state, at)? else {
                        return Ok(None);
                    };
                    let Some(no) = self.forward_word(else_state, at)? else {
                        return Ok(None);
                    };
                    self.work.charge(
                        yes.work_size()
                            .saturating_add(no.work_size())
                            .saturating_add(condition.work_size()),
                    )?;
                    ExprId::ite(condition.clone(), yes, no, self.limits.expressions())
                        .map(Some)
                        .map_err(denied)
                }
            },
            _ => Ok(None),
        }
    }

    fn overlay_byte(
        &mut self,
        old: &ExprId,
        value: &ExprId,
        relative: usize,
    ) -> LocalResult<ExprId> {
        self.work.charge(1)?;
        if let ExprKind::Ite {
            condition,
            then_value,
            else_value,
        } = old.kind()
        {
            // Map this one shared write over the retained word selection. No
            // branch assumption or branch-local fact enters the current F.
            let yes = self.overlay_byte(then_value, value, relative)?;
            let no = self.overlay_byte(else_value, value, relative)?;
            self.work.charge(
                yes.work_size()
                    .saturating_add(no.work_size())
                    .saturating_add(condition.work_size()),
            )?;
            return ExprId::ite(condition.clone(), yes, no, self.limits.expressions())
                .map_err(denied);
        }
        let shift = (31 - relative) * 8;
        let byte = self.operation(
            opcode::AND,
            &[value.clone(), ExprId::constant(U256::from(255))],
        )?;
        let byte = self.operation(opcode::SHL, &[ExprId::constant(U256::from(shift)), byte])?;
        let mask = U256::MAX ^ (U256::from(255) << shift);
        let old = self.operation(opcode::AND, &[old.clone(), ExprId::constant(mask)])?;
        self.operation(opcode::OR, &[old, byte])
    }
}
