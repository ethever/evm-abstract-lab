//! Whole-word read-over-write lowering with exact mathematical overflow padding.
//!
//! Forwarding avoids 32 variable byte shifts when the captured write and read
//! start at the same expression. Partial byte updates retain their alias guards;
//! Checked disjoint concrete words leave earlier observations intact. Possible
//! aliases and overlapping word writes use the general demanded-byte encoder.

use super::{BV, Bool, ByteStateId, ByteStateKind, Encoder, ExprId, QueryReason, valid_sum, word};
use crate::domain::symbolic::ExprKind;
use alloy_primitives::U256;
use revm_bytecode::opcode;

#[cfg(test)]
mod tests;

fn disjoint_word(start: &ExprId, destination: &ExprId, length: U256) -> bool {
    if length == U256::ZERO {
        return true;
    }
    let (Some(read), Some(written)) = (start.as_constant(), destination.as_constant()) else {
        return false;
    };
    let (Some(read_end), Some(written_end)) = (
        read.checked_add(U256::from(31)),
        written.checked_add(length - U256::from(1)),
    ) else {
        return false;
    };
    read_end < written || written_end < read
}

impl Encoder<'_> {
    /// A selector observes only the high four bytes of the captured word. Keep
    /// their original view/source padding and both mathematical address checks;
    /// the discarded suffix cannot affect unsigned division by 2^224.
    pub(in crate::domain::relational::solver) fn selector_prefix(
        &mut self,
        op: u8,
        args: &[ExprId],
    ) -> Result<Option<BV>, QueryReason> {
        let value = match op {
            opcode::SHR if args[0].as_constant() == Some(U256::from(224)) => &args[1],
            opcode::DIV if args[1].as_constant() == Some(U256::ONE << 224) => &args[0],
            _ => return Ok(None),
        };
        let ExprKind::WordRead { view, offset } = value.kind() else {
            return Ok(None);
        };
        let offset = self.expression(offset)?;
        let mut bytes = Vec::with_capacity(4);
        for j in 0..4 {
            let (index, valid) = valid_sum(&offset, &word(U256::from(j)));
            let byte = self.view_byte(view, &index)?;
            self.charge(4)?;
            bytes.push(valid.ite(&byte, &BV::from_u64(0, 8)));
        }
        self.charge(1)?;
        self.word_from_bytes(bytes)
            .map(|prefix| Some(prefix.zero_ext(224)))
    }

    fn pad_written_word(&mut self, at: BV, value: BV) -> Result<BV, QueryReason> {
        // Near MAX only the leading bytes have mathematical addresses. Keep
        // them and zero the suffix, without assuming caller range validity.
        let missing = word(U256::from(31)).bvsub(word(U256::MAX).bvsub(&at));
        let mask = word(U256::MAX).bvshl(missing.bvmul(word(U256::from(8))));
        self.charge(7)?;
        Ok(at
            .bvule(word(U256::MAX - U256::from(31)))
            .ite(&value, &value.bvand(mask)))
    }

    fn previous_word(
        &mut self,
        state: &ByteStateId,
        start: &ExprId,
    ) -> Result<Option<BV>, QueryReason> {
        // The checked disjoint update leaves the whole read intact, including
        // any earlier ITE/source boundary. Read it with the existing encoder.
        self.word_read(
            &super::ByteView::new(
                state.clone(),
                start.clone(),
                ExprId::constant(U256::from(32)),
            ),
            &ExprId::constant(U256::ZERO),
        )
        .map(Some)
    }

    pub(super) fn word_from_bytes(&mut self, mut bytes: Vec<BV>) -> Result<BV, QueryReason> {
        // Build 16/32/64/128/256-bit pairs rather than 32 full-width shifts.
        // Zero extension followed by a constant shift is exact concatenation.
        while bytes.len() > 1 {
            let mut words = Vec::with_capacity(bytes.len() / 2);
            for pair in bytes.as_chunks::<2>().0 {
                self.charge(5)?;
                let high = &pair[0];
                let low = &pair[1];
                let width = high.width() + low.width();
                words.push(
                    high.zero_ext(low.width())
                        .bvshl(BV::from_u64(u64::from(low.width()), width))
                        .bvor(low.zero_ext(high.width())),
                );
            }
            bytes = words;
        }
        Ok(bytes.pop().expect("32 bytes form one word"))
    }

    pub(super) fn input_word(&mut self, view: &super::ByteView) -> Result<Option<BV>, QueryReason> {
        let ByteStateKind::Input {
            source,
            length,
            bindings,
        } = view.state.kind()
        else {
            return Ok(None);
        };
        if view.length != *length
            && !view
                .length
                .as_constant()
                .is_some_and(|len| len >= U256::from(32))
        {
            return Ok(None);
        }
        let constant_start = view.offset.as_constant();
        let start = match constant_start {
            Some(at) => word(at),
            None => self.expression(&view.offset)?,
        };
        self.charge(1)?;
        let array = self
            .byte_sources
            .entry(source.clone())
            .or_insert_with(|| self.symbols.fresh_array("evm_bytes", 256, 8))
            .clone();
        if self.observed_bytes.insert(view.state.clone()) {
            self.bindings(&array, bindings, true)?;
        }
        let mut bytes = Vec::with_capacity(32);
        for j in 0..32 {
            self.charge(if constant_start.is_some() { 2 } else { 3 })?;
            let index = match constant_start {
                Some(at) => word(at.wrapping_add(U256::from(j))),
                None => start.bvadd(word(U256::from(j))),
            };
            bytes.push(array.select(&index));
        }
        let raw = self.word_from_bytes(bytes)?;
        self.charge(1)?;
        if constant_start
            .filter(|at| *at <= U256::MAX - U256::from(31))
            .is_some_and(|at| {
                self.guarantees
                    .minimum(length)
                    .is_some_and(|lower| lower > at + U256::from(31))
            })
        {
            // The already asserted source bound covers every byte, including
            // the strict source-length boundary. No padding shift is needed.
            return Ok(Some(raw));
        }
        let length = self.expression(length)?;
        // A retained byte has mathematical start+j < source length <= MAX,
        // so it cannot overflow. Wrapped raw suffix reads are discarded by the
        // same prefix mask that enforces the source's strict length boundary.
        let available = length.bvsub(&start);
        // Below 32, six bits suffice for 32-available (including 32 at zero).
        // This avoids a full-width variable subtraction/multiply for padding.
        let missing = BV::from_u64(32, 6).bvsub(available.extract(5, 0));
        let shift = missing.zero_ext(250).bvshl(word(U256::from(3)));
        let prefix = raw.bvand(word(U256::MAX).bvshl(shift));
        self.charge(12)?;
        Ok(Some(length.bvugt(&start).ite(
            &available.bvuge(word(U256::from(32))).ite(&raw, &prefix),
            &word(U256::ZERO),
        )))
    }

    pub(super) fn forwarded_word(
        &mut self,
        state: &ByteStateId,
        start: &ExprId,
    ) -> Result<Option<BV>, QueryReason> {
        self.charge(1)?;
        match state.kind() {
            ByteStateKind::Zero => Ok(Some(word(U256::ZERO))),
            ByteStateKind::Ite { .. } => {
                // A factored byte update can sit above the selected previous
                // word. Lift that old read once, retaining the same guard,
                // rather than revisiting both states separately for 32 bytes.
                self.previous_word(state, start)
            }
            ByteStateKind::WriteWord { offset, value, .. } if offset == start => {
                let at = self.expression(start)?;
                let value = self.expression(value)?;
                self.pad_written_word(at, value).map(Some)
            }
            ByteStateKind::WriteWord {
                previous, offset, ..
            } => {
                if disjoint_word(start, offset, U256::from(32)) {
                    // No byte in these mathematical intervals can alias. Do
                    // not use wrapping ends or symbolic expression inequality.
                    return self.previous_word(previous, start);
                }
                Ok(None)
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                if let Some(copied) = length.as_constant() {
                    if copied >= U256::from(32) && destination == start {
                        // A complete word comes from the captured source view;
                        // its own padding and the destination overflow both hold.
                        let at = self.expression(start)?;
                        let value =
                            self.word_read(&source.canonicalize(), &ExprId::constant(U256::ZERO))?;
                        return self.pad_written_word(at, value).map(Some);
                    }
                    if disjoint_word(start, destination, copied) {
                        return self.previous_word(previous, start);
                    }
                }
                Ok(None)
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                let Some(old) = self.forwarded_word(previous, start)? else {
                    return Ok(None);
                };
                let at = self.expression(offset)?;
                let base = self.expression(start)?;
                let delta = if offset == start {
                    word(U256::ZERO)
                } else if let ExprKind::Operation {
                    opcode: opcode::ADD,
                    args,
                } = offset.kind()
                    && let Some(constant) = if args[0] == *start {
                        args[1].as_constant()
                    } else if args[1] == *start {
                        args[0].as_constant()
                    } else {
                        None
                    }
                {
                    word(constant)
                } else {
                    at.bvsub(&base)
                };
                let inside = Bool::and(&[at.bvuge(&base), delta.bvult(word(U256::from(32)))]);
                let shift = word(U256::from(31))
                    .bvsub(&delta)
                    .bvmul(word(U256::from(8)));
                let mask = word(U256::from(255)).bvshl(&shift);
                let byte = self
                    .expression(value)?
                    .bvand(word(U256::from(255)))
                    .bvshl(shift);
                let updated = old.bvand(mask.bvnot()).bvor(byte);
                self.charge(10)?;
                Ok(Some(inside.ite(&updated, &old)))
            }
            _ => Ok(None),
        }
    }
}
