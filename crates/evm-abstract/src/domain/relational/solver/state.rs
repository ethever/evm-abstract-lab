//! Exact demanded reads for lazy ranges and native initial/storage arrays.
//!
//! There is no unconstrained COPY function. Hashing a general lazy range has a
//! conservative value; canonical/guarded whole-copy and bounded packed sequence
//! arguments supply actual same-content relations without assuming injectivity.

use super::{Array, BV, Bool, Encoder, ExprId, FunctionSort, QueryReason, literal, word};
use crate::domain::{
    NumericValue,
    symbolic::{ByteStateId, ByteStateKind, ByteView, SlotStateId, SlotStateKind, StateBinding},
};
use alloy_primitives::{U256, keccak256};
use embedded_smt::FunctionArg;

mod word;

fn byte_zero() -> BV {
    BV::from_u64(0, 8)
}
/// Both branches have already passed normal EVM encoding, including its
/// fallible expression work, charges and accumulated definitions/numeric facts.
/// Generic SMT ITEs cannot assume this boundary and keep arbitrary subgraphs.
fn choose(guard: &Bool, yes: &BV, no: &BV) -> BV {
    assert_eq!(
        yes.width(),
        no.width(),
        "conditional branches must have equal widths"
    );
    match guard.as_bool() {
        Some(true) => yes.clone(),
        Some(false) => no.clone(),
        None => guard.ite(yes, no),
    }
}
fn valid_sum(a: &BV, b: &BV) -> (BV, Bool) {
    if let (Some(a), Some(b)) = (literal(a), literal(b)) {
        let (sum, overflow) = a.overflowing_add(b);
        return (word(sum), Bool::from_bool(!overflow));
    }
    if literal(a) == Some(U256::ZERO) {
        return (b.clone(), Bool::from_bool(true));
    }
    if literal(b) == Some(U256::ZERO) {
        return (a.clone(), Bool::from_bool(true));
    }
    let sum = a.bvadd(b);
    let valid = sum.bvuge(a);
    (sum, valid)
}

impl Encoder<'_> {
    fn numeric(&mut self, value: &BV, numeric: &NumericValue) -> Result<Vec<Bool>, QueryReason> {
        self.charge(numeric.work_size())?;
        if let Some(exact) = numeric.singleton() {
            return Ok(vec![value.eq(word(exact))]);
        }
        if let Some(values) = numeric.constants() {
            // Membership in the complete filtered set already implies every
            // other numeric component. Encoding its redundant congruence can
            // otherwise bit-blast a full Word256 remainder for a tiny set.
            return Ok(vec![Bool::or(
                &values
                    .iter()
                    .filter(|candidate| numeric.contains(**candidate))
                    .map(|candidate| value.eq(word(*candidate)))
                    .collect::<Vec<_>>(),
            )]);
        }
        let mut facts = Vec::new();
        let (lo, hi) = numeric.interval().unsigned_bounds();
        if lo != U256::ZERO {
            facts.push(value.bvuge(word(lo)));
        }
        if hi != U256::MAX {
            facts.push(value.bvule(word(hi)));
        }
        let (lo, hi) = numeric.interval().signed_bounds();
        let biased = value.bvxor(word(U256::from(1) << 255));
        if lo != U256::ZERO {
            facts.push(biased.bvuge(word(lo)));
        }
        if hi != U256::MAX {
            facts.push(biased.bvule(word(hi)));
        }
        let zero = numeric.known_bits().zero();
        let one = numeric.known_bits().one();
        if zero != U256::ZERO {
            facts.push(value.bvand(word(zero)).eq(word(U256::ZERO)));
        }
        if one != U256::ZERO {
            facts.push(value.bvand(word(one)).eq(word(one)));
        }
        if let Some((modulus, residue)) = numeric.congruence().modulus_residue() {
            if modulus != U256::from(1) {
                facts.push(value.bvurem(word(modulus)).eq(word(residue)));
            }
        } else if let Some(exact) = numeric.congruence().singleton() {
            facts.push(value.eq(word(exact)));
        }
        if !numeric.may_be_zero() {
            facts.push(value.eq(word(U256::ZERO)).not());
        }
        Ok(facts)
    }
    fn bindings(
        &mut self,
        array: &Array,
        bindings: &[StateBinding],
        bytes: bool,
    ) -> Result<(), QueryReason> {
        for binding in bindings {
            self.charge(2)?;
            let value = self.expression(&binding.value)?;
            let selected = array.select(&word(binding.index));
            if bytes {
                // Equality to the zero-extended cell enforces its width, rather
                // than silently truncating an inconsistent observed scalar.
                self.definitions.push(selected.zero_ext(248).eq(&value));
            } else {
                self.definitions.push(selected.eq(&value));
            }
            let facts = self.numeric(&value, &binding.numeric)?;
            self.definitions.extend(facts);
        }
        Ok(())
    }
    pub(super) fn slots(&mut self, state: &SlotStateId) -> Result<Array, QueryReason> {
        if let Some(array) = self.slot_arrays.get(state) {
            return Ok(array.clone());
        }
        self.charge(1)?;
        let array = match state.kind() {
            SlotStateKind::Zero => Array::constant(256, &word(U256::ZERO)),
            SlotStateKind::Input { source, bindings } => {
                let array = self
                    .slot_sources
                    .entry(source.clone())
                    .or_insert_with(|| self.symbols.fresh_array("evm_storage", 256, 256))
                    .clone();
                self.bindings(&array, bindings, false)?;
                array
            }
            SlotStateKind::Store {
                previous,
                key,
                value,
            } => {
                let previous = self.slots(previous)?;
                let key = self.expression(key)?;
                let value = self.expression(value)?;
                previous.store(&key, &value)
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let guard = self.truth(condition)?;
                let yes = self.slots(then_state)?;
                let no = self.slots(else_state)?;
                guard.ite_array(&yes, &no)
            }
        };
        self.slot_arrays.insert(state.clone(), array.clone());
        Ok(array)
    }
    pub(super) fn slot_read(
        &mut self,
        state: &SlotStateId,
        key: &ExprId,
    ) -> Result<BV, QueryReason> {
        self.charge(1)?;
        match state.kind() {
            SlotStateKind::Zero => Ok(word(U256::ZERO)),
            SlotStateKind::Input { .. } => {
                let index = self.expression(key)?;
                Ok(self.slots(state)?.select(&index))
            }
            SlotStateKind::Store {
                previous,
                key: written,
                value,
            } => {
                if written == key {
                    return self.expression(value);
                }
                if let Some(equal) = self.guarantees.equality(written, key) {
                    return if equal {
                        self.expression(value)
                    } else {
                        self.slot_read(previous, key)
                    };
                }
                let index = self.expression(key)?;
                let written = self.expression(written)?;
                let value = self.expression(value)?;
                let old = self.slot_read(previous, key)?;
                self.charge(2)?;
                Ok(index.eq(&written).ite(&value, &old))
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let guard = self.truth(condition)?;
                let yes = self.slot_read(then_state, key)?;
                let no = self.slot_read(else_state, key)?;
                Ok(guard.ite(&yes, &no))
            }
        }
    }
    /// Native full-content arrays are used only where padding/range semantics
    /// can actually be expressed with finite stores and conditional arrays.
    fn byte_array(&mut self, state: &ByteStateId) -> Result<Option<Array>, QueryReason> {
        if let Some(array) = self.byte_arrays.get(state) {
            return Ok(array.clone());
        }
        self.charge(1)?;
        let array = match state.kind() {
            ByteStateKind::Zero => Some(Array::constant(256, &byte_zero())),
            ByteStateKind::Exact(bytes) => {
                self.charge(bytes.len())?;
                let mut array = Array::constant(256, &byte_zero());
                for (i, byte) in bytes.iter().copied().enumerate() {
                    if byte != 0 {
                        array =
                            array.store(&word(U256::from(i)), &BV::from_u64(u64::from(byte), 8));
                    }
                }
                Some(array)
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                if let Some(array) = self.byte_array(previous)? {
                    let offset = self.expression(offset)?;
                    let value = self.expression(value)?.extract(7, 0);
                    Some(array.store(&offset, &value))
                } else {
                    None
                }
            }
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                if let Some(mut array) = self.byte_array(previous)? {
                    self.charge(32 * 4)?;
                    let offset = self.expression(offset)?;
                    let value = self.expression(value)?;
                    for j in 0..32 {
                        let (index, valid) = valid_sum(&offset, &word(U256::from(j)));
                        let byte = value.extract(255 - j * 8, 248 - j * 8);
                        array = valid.ite_array(&array.store(&index, &byte), &array);
                    }
                    Some(array)
                } else {
                    None
                }
            }
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => match (self.byte_array(then_state)?, self.byte_array(else_state)?) {
                (Some(yes), Some(no)) => Some(self.truth(condition)?.ite_array(&yes, &no)),
                _ => None,
            },
            ByteStateKind::Input {
                source, bindings, ..
            } => {
                let array = self
                    .byte_sources
                    .entry(source.clone())
                    .or_insert_with(|| self.symbols.fresh_array("evm_bytes", 256, 8))
                    .clone();
                if self.observed_bytes.insert(state.clone()) {
                    self.bindings(&array, bindings, true)?;
                }
                None
            }
            ByteStateKind::Slice { .. } | ByteStateKind::Copy { .. } => None,
        };
        self.byte_arrays.insert(state.clone(), array.clone());
        Ok(array)
    }
    pub(super) fn byte(&mut self, state: &ByteStateId, index: &BV) -> Result<BV, QueryReason> {
        self.charge(1)?;
        match state.kind() {
            ByteStateKind::Zero => Ok(byte_zero()),
            ByteStateKind::Exact(bytes) => {
                if let Some(index) = literal(index) {
                    let byte = usize::try_from(index)
                        .ok()
                        .and_then(|i| bytes.get(i))
                        .copied()
                        .unwrap_or(0);
                    return Ok(BV::from_u64(u64::from(byte), 8));
                }
                Ok(self
                    .byte_array(state)?
                    .expect("exact native array")
                    .select(index))
            }
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } => {
                let array = self
                    .byte_sources
                    .entry(source.clone())
                    .or_insert_with(|| self.symbols.fresh_array("evm_bytes", 256, 8))
                    .clone();
                // The state memo avoids binding the same observations for every
                // byte in a word; source-array identity is shared across views.
                if self.observed_bytes.insert(state.clone()) {
                    self.bindings(&array, bindings, true)?;
                }
                let length = self.expression(length)?;
                self.charge(2)?;
                Ok(choose(
                    &index.bvult(&length),
                    &array.select(index),
                    &byte_zero(),
                ))
            }
            ByteStateKind::WriteByte {
                previous,
                offset,
                value,
            } => {
                let offset = self.expression(offset)?;
                let value = self.expression(value)?.extract(7, 0);
                let old = self.byte(previous, index)?;
                self.charge(2)?;
                Ok(choose(&index.eq(&offset), &value, &old))
            }
            ByteStateKind::WriteWord {
                previous,
                offset,
                value,
            } => {
                let offset = self.expression(offset)?;
                let delta = index.bvsub(&offset);
                let value = self.expression(value)?;
                // The >= test prevents subtraction wrap from manufacturing an alias.
                let inside = Bool::and(&[index.bvuge(&offset), delta.bvult(word(U256::from(32)))]);
                let shift = word(U256::from(31))
                    .bvsub(&delta)
                    .bvmul(word(U256::from(8)));
                let byte = value.bvlshr(&shift).extract(7, 0);
                let old = self.byte(previous, index)?;
                self.charge(7)?;
                Ok(choose(&inside, &byte, &old))
            }
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => {
                let destination = self.expression(destination)?;
                let length = self.expression(length)?;
                let delta = index.bvsub(&destination);
                let inside = Bool::and(&[index.bvuge(&destination), delta.bvult(&length)]);
                let new = self.view_byte(source, &delta)?;
                let old = self.byte(previous, index)?;
                self.charge(4)?;
                Ok(choose(&inside, &new, &old))
            }
            ByteStateKind::Slice { source } => self.view_byte(source, index),
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let guard = self.truth(condition)?;
                let yes = self.byte(then_state, index)?;
                let no = self.byte(else_state, index)?;
                Ok(choose(&guard, &yes, &no))
            }
        }
    }
    fn view_byte(&mut self, view: &ByteView, index: &BV) -> Result<BV, QueryReason> {
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = view.state.kind()
        {
            let guard = self.truth(condition)?;
            let (yes_view, yes_work) = view.branch_view(then_state.clone(), condition, true);
            let (no_view, no_work) = view.branch_view(else_state.clone(), condition, false);
            self.charge(yes_work.saturating_add(no_work))?;
            let yes_view = yes_view.canonicalize();
            let no_view = no_view.canonicalize();
            let yes = self.view_byte(&yes_view, index)?;
            let no = self.view_byte(&no_view, index)?;
            self.charge(1)?;
            return Ok(choose(&guard, &yes, &no));
        }
        if let ByteStateKind::Copy {
            destination,
            source,
            length,
            ..
        } = view.state.kind()
            && view.offset == *destination
            && view.length == *length
            && *length == source.length
        {
            // A read through a complete copy is the corresponding source read.
            // Preserve the destination's mathematical overflow guard even when
            // the whole range has no structural nonoverflow proof yet.
            let destination = self.expression(destination)?;
            let (_, valid) = valid_sum(&destination, index);
            let byte = self.view_byte(source, index)?;
            self.charge(2)?;
            return Ok(choose(&valid, &byte, &byte_zero()));
        }
        let offset = self.expression(&view.offset)?;
        let length = self.expression(&view.length)?;
        let (backing, valid) = valid_sum(&offset, index);
        let byte = self.byte(&view.state, &backing)?;
        self.charge(5)?;
        Ok(choose(
            &Bool::and(&[valid, index.bvult(length)]),
            &byte,
            &byte_zero(),
        ))
    }
    pub(super) fn word_read(
        &mut self,
        view: &ByteView,
        offset: &ExprId,
    ) -> Result<BV, QueryReason> {
        if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = view.state.kind()
        {
            // Lift the read through the state selector once per word, instead
            // of traversing both alternatives separately for every byte.
            let guard = self.truth(condition)?;
            let (yes_view, yes_work) = view.branch_view(then_state.clone(), condition, true);
            let (no_view, no_work) = view.branch_view(else_state.clone(), condition, false);
            self.charge(1usize.saturating_add(yes_work).saturating_add(no_work))?;
            let yes = self.word_read(&yes_view.canonicalize(), offset)?;
            let no = self.word_read(&no_view.canonicalize(), offset)?;
            return Ok(guard.ite(&yes, &no));
        }
        if offset.as_constant() == Some(U256::ZERO)
            && view
                .length
                .as_constant()
                .is_some_and(|len| len >= U256::from(32))
            && let Some(value) = self.forwarded_word(&view.state, &view.offset)?
        {
            return Ok(value);
        }
        if offset.as_constant() == Some(U256::ZERO)
            && let Some(value) = self.input_word(view)?
        {
            return Ok(value);
        }
        let offset = self.expression(offset)?;
        let mut bytes = Vec::with_capacity(32);
        for j in 0..32 {
            let (index, valid) = valid_sum(&offset, &word(U256::from(j)));
            let byte = self.view_byte(view, &index)?;
            let byte = choose(&valid, &byte, &byte_zero());
            self.charge(4)?;
            bytes.push(byte);
        }
        self.word_from_bytes(bytes)
    }
    /// Encode the first `len <= 64` view bytes, with a zero suffix, as BV512.
    /// The former normalized array was zero outside these 64 fixed indices;
    /// its extensional equality is exactly equality of this packed content.
    fn normalized_hash_content(&mut self, input: &ByteView, len: usize) -> Result<BV, QueryReason> {
        if len == 0 {
            self.charge(1)?;
            return Ok(BV::from_u64(0, 512));
        }
        let zero = ExprId::constant(U256::ZERO);
        let high = self.word_read(input, &zero)?;
        let low = if len <= 32 {
            word(U256::ZERO)
        } else if let Some(offset) = input.offset.as_constant() {
            if let Some(start) = offset.checked_add(U256::from(32)) {
                // The shorter view keeps exactly the original remaining bytes.
                // A checked concrete start lets aligned MSTORE words forward.
                self.word_read(
                    &ByteView::new(
                        input.state.clone(),
                        ExprId::constant(start),
                        ExprId::constant(U256::from(len - 32)),
                    ),
                    &zero,
                )?
            } else {
                // Every byte in the second word is beyond the mathematical
                // address space. It must not read a wrapped low address.
                word(U256::ZERO)
            }
        } else {
            // Preserve the original boundary and valid_sum overflow guards for
            // a symbolic start; wrapping ADD cannot construct a backing view.
            self.word_read(input, &ExprId::constant(U256::from(32)))?
        };
        self.charge(5)?;
        Ok(high
            .zero_ext(256)
            .bvshl(BV::from_u64(256, 512))
            .bvor(low.zero_ext(256)))
    }
    fn function(&mut self, signature: usize) -> embedded_smt::Function {
        self.hash_functions
            .entry(signature)
            .or_insert_with(|| {
                let sorts = if signature == 0 {
                    vec![FunctionSort::Bv(512), FunctionSort::Bv(256)]
                } else {
                    vec![
                        FunctionSort::Array {
                            index_width: 256,
                            element_width: 8,
                        },
                        FunctionSort::Bv(256),
                        FunctionSort::Bv(256),
                        FunctionSort::Bv(256),
                    ]
                };
                self.symbols.fresh_function("evm_keccak", &sorts, 256)
            })
            .clone()
    }
    pub(super) fn hash(&mut self, input: &ByteView) -> Result<BV, QueryReason> {
        let input = input.canonicalize();
        if let Some(hash) = self.hashes.get(&input) {
            return Ok(hash.clone());
        }
        self.charge(1)?;
        let length = self.expression(&input.length)?;
        let offset = self.expression(&input.offset)?;
        let empty = word(U256::from_be_bytes(keccak256([]).0));
        // Preserve whole-copy equality conditionally when destination+len can
        // overflow; legal symbolic ranges need no byte enumeration.
        let mut whole_copy = None;
        if let ByteStateKind::Copy {
            destination,
            source,
            length: copied,
            ..
        } = input.state.kind()
            && input.offset == *destination
            && input.length == *copied
            && *copied == source.length
        {
            let destination = self.expression(destination)?;
            let valid = match literal(&destination) {
                Some(at) if at == U256::ZERO => Bool::from_bool(true),
                Some(at) => {
                    self.unsigned_le(&input.length, &length, U256::ZERO.wrapping_sub(at))?
                }
                None => Bool::or(&[
                    length.eq(word(U256::ZERO)),
                    length
                        .bvsub(word(U256::from(1)))
                        .bvule(word(U256::MAX).bvsub(&destination)),
                ]),
            };
            whole_copy = Some((valid, self.hash(source)?));
        }
        let value = if let Some((valid, copied)) = whole_copy {
            valid.ite(&copied, &self.symbols.fresh_bv("evm_hash_uncompared", 256))
        } else if let ByteStateKind::Ite {
            condition,
            then_state,
            else_state,
        } = input.state.kind()
        {
            let guard = self.truth(condition)?;
            if let Some(nonzero) = self.guarantees.truth(condition) {
                let state = if nonzero { then_state } else { else_state };
                let (view, work) = input.branch_view(state.clone(), condition, nonzero);
                self.charge(work)?;
                let value = self.hash(&view)?;
                self.hashes.insert(input, value.clone());
                return Ok(value);
            }
            let (yes_view, yes_work) = input.branch_view(then_state.clone(), condition, true);
            let (no_view, no_work) = input.branch_view(else_state.clone(), condition, false);
            self.charge(yes_work.saturating_add(no_work))?;
            // These operand substitutions hold only inside the matching state
            // guard. Keep the outer selector rather than assuming either path.
            let yes = self.hash(&yes_view)?;
            let no = self.hash(&no_view)?;
            self.charge(1)?;
            guard.ite(&yes, &no)
        } else if let Some(len) = input
            .length
            .as_constant()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|len| *len <= crate::domain::symbolic::MAX_NORMALIZED_HASH_BYTES)
        {
            // All backing kinds use the same packed-content/length signature.
            // Zero padding makes it equivalent to the bounded normalized array,
            // without its store chain and extensional array equality clauses.
            let content = self.normalized_hash_content(&input, len)?;
            self.function(0)
                .apply(&[FunctionArg::Bv(&content), FunctionArg::Bv(&length)])
        } else if let ByteStateKind::Input {
            source,
            length: source_length,
            bindings,
        } = input.state.kind()
        {
            let array = self
                .byte_sources
                .entry(source.clone())
                .or_insert_with(|| self.symbols.fresh_array("evm_bytes", 256, 8))
                .clone();
            if self.observed_bytes.insert(input.state.clone()) {
                self.bindings(&array, bindings, true)?;
            }
            let source_length = self.expression(source_length)?;
            self.function(1).apply(&[
                FunctionArg::Array(&array),
                FunctionArg::Bv(&source_length),
                FunctionArg::Bv(&offset),
                FunctionArg::Bv(&length),
            ])
        } else if let Some(array) = self.byte_array(&input.state)? {
            // Total content arrays use a full-domain boundary, plus offset/len.
            // This signature is distinct from padded sources at length MAX.
            self.function(2).apply(&[
                FunctionArg::Array(&array),
                FunctionArg::Bv(&word(U256::ZERO)),
                FunctionArg::Bv(&offset),
                FunctionArg::Bv(&length),
            ])
        } else if let Some((destination, copied, prior)) = input.prior_copy_hash_view() {
            // A later disjoint COPY cannot change the requested earlier view.
            // Widen both ends: these are mathematical ranges, not BV256 sums.
            let destination = self.expression(destination)?;
            let copied_expression = copied;
            let copied = self.expression(copied_expression)?;
            let (before, after) =
                if let (Some(at), Some(dst)) = (literal(&offset), literal(&destination)) {
                    (
                        if at <= dst {
                            self.unsigned_le(&input.length, &length, dst - at)?
                        } else {
                            Bool::from_bool(false)
                        },
                        if dst <= at {
                            self.unsigned_le(copied_expression, &copied, at - dst)?
                        } else {
                            Bool::from_bool(false)
                        },
                    )
                } else {
                    (
                        offset
                            .zero_ext(1)
                            .bvadd(length.zero_ext(1))
                            .bvule(destination.zero_ext(1)),
                        destination
                            .zero_ext(1)
                            .bvadd(copied.zero_ext(1))
                            .bvule(offset.zero_ext(1)),
                    )
                };
            let disjoint = Bool::or(&[
                length.eq(word(U256::ZERO)),
                copied.eq(word(U256::ZERO)),
                before,
                after,
            ]);
            let preserved = self.hash(&prior)?;
            self.charge(7)?;
            disjoint.ite(
                &preserved,
                &self.symbols.fresh_bv("evm_hash_uncompared", 256),
            )
        } else {
            self.symbols.fresh_bv("evm_hash_uncompared", 256)
        };
        let value = length.eq(word(U256::ZERO)).ite(&empty, &value);
        // Known snapshot bytes must constrain the SAME hash application, so
        // an older view of this source also gains its real digest when the
        // initial observations become available within this query.
        if input
            .length
            .as_constant()
            .and_then(|n| usize::try_from(n).ok())
            .is_some_and(|n| {
                n.saturating_mul(input.nodes().saturating_add(1))
                    <= self.limits.max_nodes.saturating_sub(self.work)
            })
            && let Some(bytes) = input
                .known_bytes(self.limits.expressions())
                .map_err(|error| match error {
                    crate::domain::symbolic::ExprError::Cancelled => QueryReason::Cancelled,
                    _ => QueryReason::ExpressionLimit,
                })?
        {
            self.charge(bytes.len().saturating_mul(input.nodes().saturating_add(1)))?;
            self.definitions
                .push(value.eq(word(U256::from_be_bytes(keccak256(bytes).0))));
        }
        self.hashes.insert(input, value.clone());
        Ok(value)
    }
}

#[cfg(test)]
mod tests;
