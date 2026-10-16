//! Demand-local reads of immutable snapshots. Unknown aliases retain both
//! possibilities; index arithmetic checks mathematical bounds before wrapping.

use super::{Engine, LocalResult};
use crate::domain::{
    NumericValue,
    symbolic::{ByteStateId, ByteStateKind, ByteView, ExprId, SlotStateId, SlotStateKind},
};
use alloy_primitives::{U256, keccak256};
use revm_bytecode::opcode;

impl Engine<'_, '_> {
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
                        self.same(&key, &stored)
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
                length, bindings, ..
            } => {
                let at = self.exact(index)?;
                let length = self.eval(length)?;
                let Some(at) = at else {
                    return Ok(NumericValue::unknown_byte());
                };
                let (lower, upper) = length.interval().unsigned_bounds();
                if at >= upper {
                    return Ok(NumericValue::constant(U256::ZERO));
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
                    _ => self.same(index, offset),
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
                    _ if self.same(index, offset) == Some(true) => Some(U256::ZERO),
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
                && self.same(written, &view.offset) == Some(true)
            {
                return self.eval(value);
            }
        }
        let mut word = NumericValue::constant(U256::ZERO);
        for i in 0..32usize {
            let byte = match relative.checked_add(U256::from(i)) {
                Some(at) => self.view_byte(view, at)?,
                None => NumericValue::constant(U256::ZERO),
            };
            let shifted = self.apply(
                opcode::SHL,
                &[NumericValue::constant(U256::from((31 - i) * 8)), byte],
            )?;
            word = self.apply(opcode::OR, &[word, shifted])?;
        }
        Ok(word)
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
