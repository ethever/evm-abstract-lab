//! Demand-local reads of immutable snapshots. Unknown aliases retain both
//! possibilities; index arithmetic checks mathematical bounds before wrapping.

use super::{Engine, LocalResult, QueryReason, Stop, terms::denied};
use crate::domain::{
    NumericValue,
    congruence::Congruence,
    interval::Interval,
    known_bits::KnownBits,
    symbolic::{
        ByteStateId, ByteStateKind, ByteView, ExprId, ExprKind, SlotStateId, SlotStateKind,
        SourceId,
    },
};
use alloy_primitives::{U256, keccak256};
use revm_bytecode::opcode;

#[cfg(test)]
mod tests;

/// A directly guaranteed initial word, retained only for this query. The prefix
/// excludes view padding; checked subtraction matches its nonwrapping window.
pub(super) struct SourceWord {
    source: SourceId,
    input_length: ExprId,
    length: ExprId,
    start: U256,
    prefix: U256,
    value: U256,
}

/// Metadata shared by the bytes of one demanded, nonwrapping word window.
/// Only fully covered copies forward here; other histories use the scalar read.
struct ByteWindow<'a> {
    start: U256,
    content: WindowContent<'a>,
}

enum WindowContent<'a> {
    Zero,
    Exact(&'a [u8]),
    Input { upper: U256, words: Vec<usize> },
}

impl Engine<'_, '_> {
    fn initial_byte(&mut self, state: &ByteStateId, index: &ExprId) -> LocalResult<ExprId> {
        let ByteStateKind::Input { source, length, .. } = state.kind() else {
            unreachable!("initial byte identity requires an input state");
        };
        let length = self.term(length)?;
        self.work.charge(
            source
                .token()
                .work_size()
                .saturating_add(length.work_size()),
        )?;
        let initial = ByteStateId::input(source.clone(), length);
        self.work
            .charge(initial.nodes().saturating_add(index.work_size()))?;
        ExprId::byte_read(initial, index.clone(), self.limits.expressions()).map_err(denied)
    }

    // Observations constrain initial content, not the written state above it.
    // Visit only active expressions/states; unknown ITE arms keep their facts.
    pub(super) fn source_facts(&mut self, expression: &ExprId) -> LocalResult<()> {
        self.work.charge(1)?;
        if !expression.has_state_semantics() {
            return Ok(());
        }
        match expression.kind() {
            ExprKind::Operation { args, .. } => {
                for arg in args {
                    self.source_facts(arg)?;
                }
            }
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => {
                self.source_facts(condition)?;
                match self.choose(condition)? {
                    Some(true) => self.source_facts(then_value)?,
                    Some(false) => self.source_facts(else_value)?,
                    None => {}
                }
            }
            ExprKind::ByteRead { state, index } => {
                self.source_facts(index)?;
                self.byte_sources(state)?;
            }
            ExprKind::WordRead { view, offset } => {
                self.source_facts(offset)?;
                self.view_sources(view)?;
                self.word_source_facts(expression, view, offset)?;
            }
            ExprKind::Keccak { input } => self.view_sources(input)?,
            _ => {}
        }
        Ok(())
    }

    fn word_source_facts(
        &mut self,
        expression: &ExprId,
        view: &ByteView,
        relative: &ExprId,
    ) -> LocalResult<()> {
        let ByteStateKind::Input { source, length, .. } = view.state.kind() else {
            return Ok(());
        };
        let Some(word) = self.known(expression)?.singleton() else {
            return Ok(());
        };
        let (Some(relative), Some(start)) = (self.exact(relative)?, self.exact(&view.offset)?)
        else {
            return Ok(());
        };
        let lower = self.eval(&view.length)?.interval().unsigned_bounds().0;
        // Only bytes inside the guaranteed view prefix constrain its backing
        // source. Short-view padding and mathematical overflow bind no cell.
        let Some(prefix) = lower.checked_sub(relative).filter(|n| *n != U256::ZERO) else {
            return Ok(());
        };
        let Some(start) = start.checked_add(relative) else {
            return Ok(());
        };
        let canonical_length = self.term(length)?;
        let growth = if self.source_words.len() == self.source_words.capacity() {
            self.source_words.len()
        } else {
            0
        };
        self.work.charge(growth.saturating_add(1))?;
        self.source_words.push(SourceWord {
            source: source.clone(),
            input_length: length.clone(),
            length: canonical_length,
            start,
            prefix: prefix.min(U256::from(32)),
            value: word,
        });
        Ok(())
    }

    fn source_word_byte(
        &mut self,
        source: &SourceId,
        length: &ExprId,
        at: U256,
    ) -> LocalResult<Option<U256>> {
        let words = self.matching_source_words(source, length, at, at)?;
        self.window_source_byte(&words, at)
    }

    fn matching_source_words(
        &mut self,
        source: &SourceId,
        length: &ExprId,
        start: U256,
        end: U256,
    ) -> LocalResult<Vec<usize>> {
        let mut words = Vec::new();
        if self.source_words.is_empty() {
            return Ok(words);
        }
        let mut canonical = None;
        for index in 0..self.source_words.len() {
            self.work.charge(1)?;
            let word = &self.source_words[index];
            if word.start > end
                || start
                    .checked_sub(word.start)
                    .is_some_and(|relative| relative >= word.prefix)
            {
                continue;
            }
            if source.namespace() != word.source.namespace()
                || !self
                    .work
                    .same_expression(source.token(), word.source.token())?
            {
                continue;
            }
            let same_length = self.work.same_expression(length, &word.input_length)?;
            let canonical_length = word.length.clone();
            if !same_length {
                if canonical.is_none() {
                    canonical = Some(self.term(length)?);
                }
                if !self
                    .work
                    .same_expression(canonical.as_ref().unwrap(), &canonical_length)?
                {
                    continue;
                }
            }
            let growth = if words.len() == words.capacity() {
                words.len()
            } else {
                0
            };
            self.work.charge(growth.saturating_add(1))?;
            words.push(index);
        }
        Ok(words)
    }

    fn window_source_byte(&mut self, words: &[usize], at: U256) -> LocalResult<Option<U256>> {
        let mut found = None;
        for &index in words {
            self.work.charge(1)?;
            let word = &self.source_words[index];
            let Some(relative) = at.checked_sub(word.start).filter(|n| *n < word.prefix) else {
                continue;
            };
            self.work.charge(1)?;
            let byte = (word.value >> ((31 - relative.to::<usize>()) * 8)) & U256::from(255);
            if found.is_some_and(|previous| previous != byte) {
                return Err(super::Stop::Bottom);
            }
            found = Some(byte);
        }
        Ok(found)
    }

    fn view_window<'a>(
        &mut self,
        view: &'a ByteView,
        relative: U256,
    ) -> LocalResult<Option<ByteWindow<'a>>> {
        self.work.charge(1)?;
        let lower = self.eval(&view.length)?.interval().unsigned_bounds().0;
        if relative
            .checked_add(U256::from(32))
            .is_none_or(|end| end > lower)
        {
            return Ok(None);
        }
        let Some(start) = self
            .exact(&view.offset)?
            .and_then(|offset| offset.checked_add(relative))
            .filter(|start| start.checked_add(U256::from(31)).is_some())
        else {
            return Ok(None);
        };
        self.byte_window(&view.state, start)
    }

    fn byte_window<'a>(
        &mut self,
        state: &'a ByteStateId,
        start: U256,
    ) -> LocalResult<Option<ByteWindow<'a>>> {
        self.work.charge(1)?;
        let content = match state.kind() {
            ByteStateKind::Zero => WindowContent::Zero,
            ByteStateKind::Exact(bytes) => WindowContent::Exact(bytes),
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } if bindings.is_empty() && !self.has_source_bytes => {
                let upper = self.eval(length)?.interval().unsigned_bounds().1;
                let end = start
                    .checked_add(U256::from(31))
                    .expect("nonwrapping window");
                let words = self.matching_source_words(source, length, start, end)?;
                WindowContent::Input { upper, words }
            }
            ByteStateKind::Copy {
                destination,
                source,
                length,
                ..
            } => {
                let Some(relative) = self
                    .exact(destination)?
                    .and_then(|destination| start.checked_sub(destination))
                else {
                    return Ok(None);
                };
                let lower = self.eval(length)?.interval().unsigned_bounds().0;
                if relative
                    .checked_add(U256::from(32))
                    .is_none_or(|end| end > lower)
                {
                    return Ok(None);
                }
                return self.view_window(source, relative);
            }
            ByteStateKind::Slice { source } => return self.view_window(source, start),
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                return match self.choose(condition)? {
                    Some(true) => self.byte_window(then_state, start),
                    Some(false) => self.byte_window(else_state, start),
                    None => Ok(None),
                };
            }
            _ => return Ok(None),
        };
        Ok(Some(ByteWindow { start, content }))
    }

    fn window_byte(&mut self, window: &ByteWindow<'_>, index: usize) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        let at = window
            .start
            .checked_add(U256::from(index))
            .expect("nonwrapping window");
        Ok(match &window.content {
            WindowContent::Zero => NumericValue::constant(U256::ZERO),
            WindowContent::Exact(bytes) => NumericValue::constant(U256::from(
                usize::try_from(at)
                    .ok()
                    .and_then(|at| bytes.get(at))
                    .copied()
                    .unwrap_or(0),
            )),
            WindowContent::Input { upper, words } if at < *upper => self
                .window_source_byte(words, at)?
                .map_or_else(NumericValue::unknown_byte, NumericValue::constant),
            WindowContent::Input { .. } => NumericValue::constant(U256::ZERO),
        })
    }

    fn view_sources(&mut self, view: &ByteView) -> LocalResult<()> {
        self.source_facts(&view.offset)?;
        self.source_facts(&view.length)?;
        self.byte_sources(&view.state)
    }

    fn byte_sources(&mut self, state: &ByteStateId) -> LocalResult<()> {
        self.work.charge(1)?;
        match state.kind() {
            ByteStateKind::Input {
                length, bindings, ..
            } => {
                self.source_facts(length)?;
                if bindings.is_empty() {
                    return Ok(());
                }
                let length = self.eval(length)?;
                let (lower, upper) = length.interval().unsigned_bounds();
                for binding in bindings {
                    self.work.charge(1)?;
                    self.source_facts(&binding.value)?;
                    let value = self.eval(&binding.value)?;
                    let value = self.meet(&value, &binding.numeric)?;
                    let byte = if let Some(value) = value.singleton() {
                        NumericValue::constant(value & U256::from(255))
                    } else {
                        self.apply(
                            opcode::AND,
                            &[value, NumericValue::constant(U256::from(255))],
                        )?
                    };
                    let byte = if binding.index >= upper {
                        NumericValue::constant(U256::ZERO)
                    } else if binding.index < lower {
                        byte
                    } else {
                        self.join(byte, NumericValue::constant(U256::ZERO))?
                    };
                    let read = self.initial_byte(state, &ExprId::constant(binding.index))?;
                    self.learn(&read, &byte)?;
                    self.has_source_bytes = true;
                }
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            }
            | ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                self.byte_sources(previous)?;
                self.source_facts(offset)?;
                self.source_facts(value)?;
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                self.byte_sources(previous)?;
                self.source_facts(destination)?;
                self.source_facts(length)?;
                self.view_sources(source)?;
            }
            ByteStateKind::Slice { source } => self.view_sources(source)?,
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                self.source_facts(condition)?;
                match self.choose(condition)? {
                    Some(true) => self.byte_sources(then_state)?,
                    Some(false) => self.byte_sources(else_state)?,
                    None => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn choose(&mut self, condition: &ExprId) -> LocalResult<Option<bool>> {
        let value = self.eval(condition)?;
        Ok(if !value.may_be_zero() {
            Some(true)
        } else if !value.may_be_nonzero() {
            Some(false)
        } else {
            None
        })
    }

    pub(super) fn exact(&mut self, expression: &ExprId) -> LocalResult<Option<U256>> {
        Ok(self.eval(expression)?.singleton())
    }

    pub(super) fn slot(&mut self, state: &SlotStateId, key: &ExprId) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        match state.kind() {
            SlotStateKind::Zero => Ok(NumericValue::constant(U256::ZERO)),
            SlotStateKind::Input { bindings, .. } => {
                let Some(key) = self.exact(key)? else {
                    return Ok(NumericValue::top());
                };
                for binding in bindings {
                    self.work.charge(1)?;
                    if binding.index == key {
                        let value = self.eval(&binding.value)?;
                        return self.meet(&value, &binding.numeric);
                    }
                }
                Ok(NumericValue::top())
            }
            SlotStateKind::Store {
                previous,
                key: stored,
                value,
            } => {
                // Evaluate operands before comparing: equal numeric singletons
                // can establish aliasing without merging their identities.
                let queried = self.eval(key)?;
                let written = self.eval(stored)?;
                let equal = match (queried.singleton(), written.singleton()) {
                    (Some(a), Some(b)) => Some(a == b),
                    _ => {
                        let key = self.term(key)?;
                        let stored = self.term(stored)?;
                        self.same(&key, &stored)?
                    }
                };
                match equal {
                    Some(true) => self.eval(value),
                    Some(false) => self.slot(previous, key),
                    None => {
                        let yes = self.eval(value)?;
                        let no = self.slot(previous, key)?;
                        self.join(yes, no)
                    }
                }
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => match self.choose(condition)? {
                Some(true) => self.slot(then_state, key),
                Some(false) => self.slot(else_state, key),
                None => {
                    let a = self.slot(then_state, key)?;
                    let b = self.slot(else_state, key)?;
                    self.join(a, b)
                }
            },
        }
    }

    pub(super) fn byte(
        &mut self,
        state: &ByteStateId,
        index: &ExprId,
    ) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        match state.kind() {
            ByteStateKind::Zero => Ok(NumericValue::constant(U256::ZERO)),
            ByteStateKind::Exact(bytes) => {
                Ok(self
                    .exact(index)?
                    .map_or_else(NumericValue::unknown_byte, |index| {
                        NumericValue::constant(U256::from(
                            usize::try_from(index)
                                .ok()
                                .and_then(|i| bytes.get(i))
                                .copied()
                                .unwrap_or(0),
                        ))
                    }))
            }
            ByteStateKind::Input {
                source,
                length: input_length,
                bindings,
            } => {
                let at = self.exact(index)?;
                let length = self.eval(input_length)?;
                let Some(at) = at else {
                    return Ok(NumericValue::unknown_byte());
                };
                let (lower, upper) = length.interval().unsigned_bounds();
                if at >= upper {
                    return Ok(NumericValue::constant(U256::ZERO));
                }
                let word_byte = self.source_word_byte(source, input_length, at)?;
                if self.has_source_bytes {
                    let initial = self.initial_byte(state, &ExprId::constant(at))?;
                    let observed = self.known(&initial)?;
                    if observed
                        .singleton()
                        .is_some_and(|byte| byte <= U256::from(255))
                        && word_byte.is_none()
                    {
                        return Ok(observed);
                    }
                    if !observed.is_top() {
                        let byte = word_byte
                            .map_or_else(NumericValue::unknown_byte, NumericValue::constant);
                        return self.meet(&observed, &byte);
                    }
                }
                if let Some(byte) = word_byte {
                    return Ok(NumericValue::constant(byte));
                }
                for binding in bindings {
                    self.work.charge(1)?;
                    if binding.index == at {
                        let value = self.eval(&binding.value)?;
                        let value = self.meet(&value, &binding.numeric)?;
                        let byte = self.apply(
                            opcode::AND,
                            &[value, NumericValue::constant(U256::from(255))],
                        )?;
                        return Ok(if at < lower {
                            byte
                        } else {
                            self.join(byte, NumericValue::constant(U256::ZERO))?
                        });
                    }
                }
                Ok(NumericValue::unknown_byte())
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                let equal = match (self.exact(index)?, self.exact(offset)?) {
                    (Some(a), Some(b)) => Some(a == b),
                    _ => self.same(index, offset)?,
                };
                match equal {
                    Some(false) => self.byte(previous, index),
                    yes => {
                        let value = self.eval(value)?;
                        let value = self.apply(
                            opcode::AND,
                            &[value, NumericValue::constant(U256::from(255))],
                        )?;
                        if yes == Some(true) {
                            Ok(value)
                        } else {
                            let old = self.byte(previous, index)?;
                            self.join(value, old)
                        }
                    }
                }
            }
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                let relative = match (self.exact(index)?, self.exact(offset)?) {
                    (Some(at), Some(start)) => {
                        if at < start || at - start >= U256::from(32) {
                            return self.byte(previous, index);
                        }
                        Some(at - start)
                    }
                    _ if self.same(index, offset)? == Some(true) => Some(U256::ZERO),
                    _ => None,
                };
                if let Some(relative) = relative {
                    let value = self.eval(value)?;
                    self.apply(opcode::BYTE, &[NumericValue::constant(relative), value])
                } else {
                    // Unproved overlap cannot pick either historical source.
                    Ok(NumericValue::unknown_byte())
                }
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                let (Some(at), Some(start)) = (self.exact(index)?, self.exact(destination)?) else {
                    return Ok(NumericValue::unknown_byte());
                };
                if at < start {
                    return self.byte(previous, index);
                }
                let relative = at - start;
                let length = self.eval(length)?;
                let (lower, upper) = length.interval().unsigned_bounds();
                if relative >= upper {
                    return self.byte(previous, index);
                }
                let copied = self.view_byte(source, relative)?;
                if relative < lower {
                    Ok(copied)
                } else {
                    let old = self.byte(previous, index)?;
                    self.join(copied, old)
                }
            }
            ByteStateKind::Slice { source } => match self.exact(index)? {
                Some(index) => self.view_byte(source, index),
                None => Ok(NumericValue::unknown_byte()),
            },
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => match self.choose(condition)? {
                Some(true) => self.byte(then_state, index),
                Some(false) => self.byte(else_state, index),
                None => {
                    let a = self.byte(then_state, index)?;
                    let b = self.byte(else_state, index)?;
                    self.join(a, b)
                }
            },
        }
    }

    fn view_byte(&mut self, view: &ByteView, relative: U256) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        let length = self.eval(&view.length)?;
        let (lower, upper) = length.interval().unsigned_bounds();
        if relative >= upper {
            return Ok(NumericValue::constant(U256::ZERO));
        }
        let offset = self.eval(&view.offset)?;
        let read = if let Some(start) = offset.singleton() {
            match start.checked_add(relative) {
                Some(at) => self.byte(&view.state, &ExprId::constant(at))?,
                None => NumericValue::constant(U256::ZERO),
            }
        } else if relative == U256::ZERO {
            self.byte(&view.state, &view.offset)?
        } else {
            NumericValue::unknown_byte()
        };
        Ok(if relative < lower {
            read
        } else {
            self.join(read, NumericValue::constant(U256::ZERO))?
        })
    }

    pub(super) fn word(&mut self, view: &ByteView, relative: &ExprId) -> LocalResult<NumericValue> {
        self.work.charge(1)?;
        let Some(relative) = self.exact(relative)? else {
            return Ok(NumericValue::top());
        };
        let length = self.eval(&view.length)?;
        let (lower, upper) = length.interval().unsigned_bounds();
        if relative >= upper {
            return Ok(NumericValue::constant(U256::ZERO));
        }
        if relative == U256::ZERO && lower >= U256::from(32) {
            // This shortcut needs the entire word in the nonwrapping address
            // range; equal symbolic offsets alone do not establish that.
            let offset = self.eval(&view.offset)?;
            if offset.interval().unsigned_bounds().1 <= U256::MAX - U256::from(31)
                && let ByteStateKind::WriteWord {
                    offset: written,
                    value,
                    ..
                } = view.state.kind()
                && self.same(written, &view.offset)? == Some(true)
            {
                return self.eval(value);
            }
        }
        // Whole-word forwarding above preserves correlations and explicit
        // source facts. Independent unconstrained bytes add no information to
        // generic SHL/OR; pack their masks directly, keeping concrete padding.
        let window = self.view_window(view, relative)?;
        self.work.prepay(32)?;
        let mut bytes = Vec::with_capacity(32);
        let mut simple = true;
        let mut zero = U256::ZERO;
        let mut one = U256::ZERO;
        for i in 0..32usize {
            self.work.charge(1)?;
            let byte = match &window {
                Some(window) => self.window_byte(window, i)?,
                None => match relative.checked_add(U256::from(i)) {
                    Some(at) => self.view_byte(view, at)?,
                    None => NumericValue::constant(U256::ZERO),
                },
            };
            self.work.prepay(byte.work_size())?;
            let shift = (31 - i) * 8;
            if let Some(value) = byte.singleton() {
                zero |= (!value & U256::from(255)) << shift;
                one |= (value & U256::from(255)) << shift;
            } else if byte != NumericValue::unknown_byte() {
                simple = false;
            }
            bytes.push(byte);
        }
        if simple {
            let bits = KnownBits::new(zero, one).expect("disjoint byte positions");
            if let Some(value) = bits.singleton() {
                return Ok(NumericValue::constant(value));
            }
            let sign = U256::ONE << 255usize;
            let biased_one = (one & !sign) | (zero & sign);
            let biased_zero = (zero & !sign) | (one & sign);
            let interval = Interval::new_unsigned(one, !zero)
                .and_then(|unsigned| {
                    unsigned.meet(&Interval::new_signed(biased_one, !biased_zero)?)
                })
                .expect("byte mask endpoints are included");
            let fixed_low = (zero | one).trailing_ones();
            let congruence = if fixed_low == 0 {
                Congruence::top()
            } else {
                let modulus = U256::ONE << fixed_low;
                Congruence::new(modulus, one & (modulus - U256::ONE))
                    .expect("nonzero byte mask modulus")
            };
            return Ok(NumericValue {
                bits,
                interval,
                congruence,
                nonzero: one != U256::ZERO,
                ..NumericValue::top()
            });
        }
        // Informative finite/range/congruence bytes retain the existing scalar
        // transfer; the direct packing rule does not discard their guarantees.
        let mut word = NumericValue::constant(U256::ZERO);
        for (i, byte) in bytes.into_iter().enumerate() {
            let shifted = self.apply(
                opcode::SHL,
                &[NumericValue::constant(U256::from((31 - i) * 8)), byte],
            )?;
            word = self.apply(opcode::OR, &[word, shifted])?;
        }
        Ok(word)
    }

    /// Only empty or terminal literal/zero views enter this bulk path. All address,
    /// boundary and selector proofs belong to the same-F hash_view rewrite.
    /// Quote the complete allocation/copy/hash envelope from cached metadata;
    /// refusal publishes no bytes, hash, or memo entry.
    pub(super) fn literal_hash(&mut self, view: &ByteView) -> LocalResult<Option<U256>> {
        self.work.charge(1)?;
        let Some(length) = view
            .length
            .as_constant()
            .and_then(|length| usize::try_from(length).ok())
        else {
            return Ok(None);
        };
        let source = if length == 0 {
            &[][..]
        } else {
            let Some(offset) = view.offset.as_constant() else {
                return Ok(None);
            };
            match view.state.kind() {
                ByteStateKind::Zero => &[][..],
                ByteStateKind::Exact(bytes) => usize::try_from(offset)
                    .ok()
                    .and_then(|start| bytes.get(start..))
                    .unwrap_or_default(),
                _ => return Ok(None),
            }
        };
        // Literal cells beyond the backing allocation, including mathematical
        // U256 address overflow, are zero. A bounded prefix needs no DAG walk.
        let copied = source.len().min(length);
        let refused = || Stop::Pending(QueryReason::ExpressionLimit);
        let steps = length
            .checked_add(copied)
            .and_then(|steps| steps.checked_add(1))
            .ok_or_else(refused)?;
        let remaining = self.work.remaining.checked_sub(steps).ok_or_else(refused)?;
        let fee = length
            .checked_mul(24)
            .and_then(|hash| hash.checked_add(24))
            .and_then(|hash| hash.checked_add(steps))
            .ok_or_else(refused)?;
        // Keep local materialization steps separate from the scalar Keccak
        // reservation, but admit the entire shared fee transactionally.
        self.work.prepay(fee)?;
        self.work.remaining = remaining;
        let mut bytes = vec![0; length];
        bytes[..copied].copy_from_slice(&source[..copied]);
        self.work.charge(0)?;
        let hash = U256::from_be_bytes(keccak256(bytes).0);
        self.work.charge(0)?;
        Ok(Some(hash))
    }

    pub(super) fn hash(&mut self, view: &ByteView) -> LocalResult<NumericValue> {
        let Some(length) = self.exact(&view.length)? else {
            return Ok(NumericValue::top());
        };
        let Ok(length) = usize::try_from(length) else {
            return Ok(NumericValue::top());
        };
        // Do not allocate or traverse an attacker-selected hash length. The
        // ordinary unknown hash is a useful retained symbolic value, not failure.
        if length > self.work.remaining {
            return Ok(NumericValue::top());
        }
        // Resolve joined lengths/selectors once under the full current F.
        // Ineligible histories still use the original demand-local byte reads.
        // Empty content needs no history/offset traversal, as in the fallback.
        if length != 0 {
            let normalized = self.hash_view(view)?;
            if let Some(hash) = self.literal_hash(&normalized)? {
                return Ok(NumericValue::constant(hash));
            }
        }
        let mut bytes = Vec::new();
        for i in 0..length {
            let Some(byte) = self.view_byte(view, U256::from(i))?.singleton() else {
                return Ok(NumericValue::top());
            };
            bytes.push(byte.to::<u8>());
        }
        self.work
            .prepay(length.saturating_mul(24).saturating_add(24))?;
        Ok(NumericValue::constant(U256::from_be_bytes(
            keccak256(bytes).0,
        )))
    }
}
