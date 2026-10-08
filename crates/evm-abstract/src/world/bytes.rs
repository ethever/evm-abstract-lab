//! Sparse abstract byte sequences. Missing bytes and missing length are separate.

use crate::domain::{AbstractValue, Domain};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Sparse bytes plus an abstract length. Reads beyond a possible length are zero.
///
/// Memory starts with zero bytes and rounds expansion to 32-byte words. Calldata
/// and returndata carry their actual byte lengths. Every allocating operation
/// checks its explicit bound before iterating or allocating a range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ByteArray {
    length: AbstractValue,
    bytes: BTreeMap<usize, AbstractValue>,
    default: AbstractValue,
    memory: bool,
}

/// A byte range whose concrete footprint cannot fit the caller's model budget.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RangeError {
    /// An unbounded length cannot be expanded into an allocated range.
    #[error("byte range has unknown size")]
    UnknownSize,
    /// A 256-bit byte offset does not fit the host index representation.
    #[error("byte offset {0} does not fit a host index")]
    OffsetTooLarge(U256),
    /// Offset plus size overflowed the host representation.
    #[error("byte range at {offset} of size {size} overflows")]
    Overflow {
        /// Host-representable start offset.
        offset: usize,
        /// Host-representable requested size.
        size: usize,
    },
    /// The allocated output or expanded memory would exceed its explicit cap.
    #[error("byte range requires {requested} bytes, exceeding limit {limit}")]
    TooLarge {
        /// Required footprint, or the offending 256-bit size.
        requested: U256,
        /// Maximum allowed footprint.
        limit: usize,
    },
}

impl ByteArray {
    /// Some retained byte/length lost an expression to the symbolic budget.
    pub(crate) fn symbolic_limit_reached(&self) -> bool {
        self.bytes
            .values()
            .chain([&self.length, &self.default])
            .any(AbstractValue::symbolic_limit_reached)
    }
    /// Visit explicit values without interpreting unknown/default bytes as one identity.
    pub(crate) fn visit_values(&self, visit: &mut impl FnMut(&AbstractValue)) {
        visit(&self.length);
        visit(&self.default);
        for value in self.bytes.values() {
            visit(value);
        }
    }
    /// Capture-avoiding transformations preserve sparse byte positions and lengths.
    pub(crate) fn update_values(&mut self, update: &mut impl FnMut(&mut AbstractValue)) {
        update(&mut self.length);
        update(&mut self.default);
        for value in self.bytes.values_mut() {
            update(value);
        }
    }
    // 完整小集合组装 word 时走有限枚举快路径；开放字节才预留通用交换。
    pub(crate) fn word_numeric_work(&self, offset: &AbstractValue, domain: Domain) -> usize {
        let Some(offsets) = offset.constants() else {
            return 1;
        };
        offsets.iter().fold(0usize, |work, offset| {
            let Some(start) = usize::try_from(*offset).ok() else {
                return work.saturating_add(1);
            };
            let combinations = (0..32).try_fold(1usize, |n, i| {
                let index = start.checked_add(i)?;
                let byte = self.byte_at(index, domain);
                let size = byte.constants()?.len();
                let next = n.saturating_mul(size);
                (next <= domain.capacity()).then_some(next)
            });
            let operation = combinations.map_or_else(
                || domain.operation_work(&[]),
                |n| n.saturating_mul(256).saturating_add(32),
            );
            let symbolic = if domain.spec().relations().enabled {
                // Mirror the balanced assembly's shape using cached sizes;
                // reserve traversal work before creating any expression nodes.
                let mut parts = (0..32)
                    .map(|i| {
                        start
                            .checked_add(i)
                            .map_or(1, |index| {
                                let value = self.byte_at(index, domain);
                                if value.singleton().is_some() {
                                    1
                                } else {
                                    value
                                        .expression()
                                        .map_or(1, |expression| expression.nodes())
                                }
                            })
                            .saturating_add(2)
                    })
                    .collect::<Vec<_>>();
                let mut nodes = parts.iter().copied().fold(0usize, usize::saturating_add);
                while parts.len() > 1 {
                    parts = parts
                        .chunks(2)
                        .map(|pair| {
                            let size = pair.iter().copied().fold(1usize, usize::saturating_add);
                            nodes = nodes.saturating_add(size);
                            size
                        })
                        .collect();
                }
                nodes.saturating_mul(256)
            } else {
                0
            };
            work.saturating_add(operation.saturating_mul(64))
                .saturating_add(symbolic)
        })
    }
    pub(crate) fn widen(&mut self, old: &Self, domain: Domain) {
        self.length = domain.widen(&old.length, &self.length);
        self.default = domain.widen(&old.default, &self.default);
        for (offset, value) in &mut self.bytes {
            *value = domain.widen(old.bytes.get(offset).unwrap_or(&old.default), value);
        }
    }
    pub(crate) fn project(&self, domain: Domain) -> Self {
        let mut out = self.clone();
        out.length = domain.project(&out.length);
        let byte = |value: &AbstractValue| {
            if value == &AbstractValue::top() {
                domain.project(&AbstractValue::unknown_byte())
            } else {
                domain.project(value)
            }
        };
        out.default = byte(&out.default);
        for value in out.bytes.values_mut() {
            *value = byte(value);
        }
        out
    }
    /// 覆盖投影时由开放字节生成的候选，不先执行投影来估算。
    pub(crate) fn projection_work(&self, domain: Domain) -> usize {
        self.bytes.values().chain([&self.default]).fold(
            domain.projection_work(&self.length),
            |work, value| {
                let projected = if value == &AbstractValue::top() {
                    domain.projection_work(&AbstractValue::unknown_byte())
                } else {
                    domain.projection_work(value)
                };
                work.saturating_add(projected)
            },
        )
    }
    /// A known empty calldata/returndata sequence.
    pub fn empty() -> Self {
        Self::exact(&[])
    }

    /// A fully observed byte sequence. Out-of-range reads still produce zero.
    pub fn exact(input: &[u8]) -> Self {
        Self {
            length: AbstractValue::constant(U256::from(input.len())),
            bytes: input
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte != 0)
                .map(|(index, byte)| (index, AbstractValue::constant(U256::from(*byte))))
                .collect(),
            default: zero(),
            memory: false,
        }
    }

    /// Unobserved bytes with unobserved length.
    pub fn unknown() -> Self {
        Self {
            length: AbstractValue::top(),
            bytes: BTreeMap::new(),
            default: AbstractValue::top(),
            memory: false,
        }
    }

    /// Fresh EVM memory: all bytes zero, current allocation zero.
    pub fn memory() -> Self {
        Self {
            memory: true,
            ..Self::empty()
        }
    }

    /// Possible byte lengths; memory lengths are word-aligned when finite.
    pub fn len(&self) -> &AbstractValue {
        &self.length
    }

    /// Explicit sparse byte facts, in offset order, without padding or joining.
    /// Rendering these facts must not change them through [`Self::byte_at`].
    pub(crate) fn stored_bytes(&self) -> impl Iterator<Item = (usize, &AbstractValue)> {
        self.bytes.iter().map(|(offset, value)| (*offset, value))
    }

    /// Value used at offsets without an explicit sparse fact.
    pub(crate) fn default_byte(&self) -> &AbstractValue {
        &self.default
    }

    /// Whether expansion follows EVM memory's 32-byte allocation rule.
    pub(crate) fn is_memory(&self) -> bool {
        self.memory
    }

    /// Cost estimate covering explicit byte values, length, and default joins.
    /// Saturation makes very large inputs exceed a finite work budget safely.
    pub fn work_size(&self) -> usize {
        self.bytes
            .values()
            .chain([&self.length, &self.default])
            .fold(0_usize, |work, value| {
                work.saturating_add(value.work_size())
            })
    }

    /// One byte with zero padding beyond every possible sequence length.
    pub fn byte_at(&self, index: usize, domain: Domain) -> AbstractValue {
        let byte = self.bytes.get(&index).unwrap_or(&self.default);
        match self.length.constants() {
            Some(lengths) => {
                let position = U256::from(index);
                let in_range = lengths.iter().any(|length| position < *length);
                let out_of_range = lengths.iter().any(|length| position >= *length);
                match (in_range, out_of_range) {
                    (false, _) => zero(),
                    (true, false) => byte.clone(),
                    (true, true) => domain.join(byte, &zero()),
                }
            }
            // With unknown length, a known stored byte is still possibly beyond
            // one of the paths' lengths. Retain zero as well.
            None => domain.join(byte, &zero()),
        }
    }

    /// Read a big-endian 32-byte EVM word at finite possible offsets.
    pub fn read_word(&self, offset: &AbstractValue, domain: Domain) -> AbstractValue {
        let Some(offsets) = offset.constants() else {
            return if self.bytes.is_empty() && self.default == zero() {
                zero()
            } else {
                AbstractValue::top()
            };
        };
        let mut result = None;
        for offset in offsets {
            if self
                .length
                .constants()
                .is_some_and(|lengths| lengths.iter().all(|length| *length <= *offset))
            {
                result = Some(join_optional(result, zero(), domain));
                continue;
            }
            let Some(offset) = host_index(*offset) else {
                // Every finite sequence is shorter than an unrepresentable
                // index, but an unknown-length sequence can contain bytes here.
                let word = if self
                    .length
                    .constants()
                    .is_some_and(|lengths| lengths.iter().all(|length| *length <= *offset))
                {
                    zero()
                } else {
                    AbstractValue::top()
                };
                result = Some(join_optional(result, word, domain));
                continue;
            };
            // A balanced concatenation keeps symbolic depth logarithmic in
            // word width. Left-folding SHL/OR adds two levels for every byte
            // and loses the word's expression at an ordinary branch afterward.
            let bytes = (0..32)
                .map(|index| {
                    offset
                        .checked_add(index)
                        .map_or_else(AbstractValue::top, |index| self.byte_at(index, domain))
                })
                .collect::<Vec<_>>();
            let reconstructed = if domain.spec().relations().enabled
                && bytes.iter().all(|value| !value.symbolic_limit_reached())
            {
                bytes
                    .iter()
                    .map(AbstractValue::symbolic_expression)
                    .collect::<Option<Vec<_>>>()
                    .and_then(|bytes| crate::domain::symbolic::ExprId::reassemble_word(&bytes))
            } else {
                None
            };
            // Reconstruct the numeric summary as before, but do not expand a
            // large symbolic tree when exact byte extraction proves the word.
            let assembly_domain = if reconstructed.is_some() {
                Domain::from_spec(domain.spec().with_relations(
                    crate::domain::relational::RelationLimits {
                        enabled: false,
                        ..domain.spec().relations()
                    },
                ))
            } else {
                domain
            };
            let mut parts = Vec::with_capacity(32);
            for (index, byte) in bytes.into_iter().enumerate() {
                parts.push(assembly_domain.apply(opcode::SHL, &[constant((31 - index) * 8), byte]));
            }
            while parts.len() > 1 {
                parts = parts
                    .chunks(2)
                    .map(|pair| {
                        if pair.len() == 2 {
                            assembly_domain.apply(opcode::OR, pair)
                        } else {
                            pair[0].clone()
                        }
                    })
                    .collect();
            }
            let mut word = parts.pop().expect("a word contains 32 bytes");
            if let Some(expression) = reconstructed {
                word = word.with_expression(expression);
            }
            result = Some(join_optional(result, word, domain));
        }
        result.expect("AbstractValue constants are nonempty")
    }

    /// Extract a zero-padded range, bounded by its output size.
    ///
    /// An unknown source offset yields unknown bytes of the requested length;
    /// no unbounded allocation is attempted.
    pub fn slice(
        &self,
        offset: &AbstractValue,
        size: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<Self, RangeError> {
        let sizes = bounded_sizes(size, max_bytes)?;
        let mut result = None;
        for size in sizes {
            let mut sliced = Self {
                length: constant(size),
                bytes: BTreeMap::new(),
                default: zero(),
                memory: false,
            };
            for index in 0..size {
                let byte = self.read_offset_byte(offset, index, domain)?;
                if byte != zero() {
                    sliced.bytes.insert(index, byte);
                }
            }
            result = Some(join_array_optional(result, sliced, domain));
        }
        Ok(result.expect("AbstractValue constants are nonempty"))
    }

    /// Expand memory for an access, or sequence length for a write.
    /// Unknown offsets conservatively make allocation length unknown.
    pub fn expand(
        &mut self,
        offset: &AbstractValue,
        size: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let sizes = bounded_sizes(size, max_bytes)?;
        if sizes.iter().all(|size| *size == 0) {
            return Ok(());
        }
        let Some(offsets) = offset.constants() else {
            self.length = AbstractValue::top();
            return Ok(());
        };
        let mut end = None;
        for offset in offsets {
            for size in &sizes {
                if *size == 0 {
                    end = Some(join_optional(end, self.length.clone(), domain));
                    continue;
                }
                let offset = host_index(*offset).ok_or(RangeError::OffsetTooLarge(*offset))?;
                let required = bounded_end(offset, *size, max_bytes, self.memory)?;
                let candidate = match self.length.constants() {
                    Some(lengths) => lengths.iter().fold(None, |candidate, length| {
                        Some(join_optional(
                            candidate,
                            AbstractValue::constant((*length).max(U256::from(required))),
                            domain,
                        ))
                    }),
                    None => Some(AbstractValue::top()),
                };
                end = Some(join_optional(
                    end,
                    candidate.expect("AbstractValue constants are nonempty"),
                    domain,
                ));
            }
        }
        self.length = end.expect("AbstractValue constants are nonempty");
        Ok(())
    }

    /// Store an EVM word, preserving its byte order and bounding memory growth.
    pub fn write_word(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let bytes: Vec<_> = (0..32)
            .map(|index| domain.apply(opcode::BYTE, &[constant(index), value.clone()]))
            .collect();
        self.write_values(offset, &bytes, max_bytes, domain)
    }

    /// Store the low byte of an EVM word.
    pub fn write_byte(
        &mut self,
        offset: &AbstractValue,
        value: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let byte = domain.apply(opcode::AND, &[value.clone(), constant(255)]);
        self.write_values(offset, &[byte], max_bytes, domain)
    }

    /// Copy exactly `size` zero-padded source bytes, including overlapping memory.
    /// The caller supplies a source snapshot for MCOPY's memmove semantics.
    pub fn copy_from(
        &mut self,
        target_offset: &AbstractValue,
        source: &Self,
        source_offset: &AbstractValue,
        size: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let sizes = bounded_sizes(size, max_bytes)?;
        let original = self.clone();
        let mut result = None;
        for size in sizes {
            let bytes = (0..size)
                .map(|index| source.read_offset_byte(source_offset, index, domain))
                .collect::<Result<Vec<_>, _>>()?;
            let mut branch = original.clone();
            branch.write_values(target_offset, &bytes, max_bytes, domain)?;
            result = Some(join_array_optional(result, branch, domain));
        }
        *self = result.expect("AbstractValue constants are nonempty");
        Ok(())
    }

    /// CALL output copying: only `min(requested, returndata.len)` bytes change.
    /// Memory expands for the full requested range; the uncopied suffix remains.
    pub fn copy_return_data(
        &mut self,
        target_offset: &AbstractValue,
        returndata: &Self,
        requested: &AbstractValue,
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        let requests = bounded_sizes(requested, max_bytes)?;
        let original = self.clone();
        let mut result = None;
        for requested in requests {
            let mut expanded = original.clone();
            expanded.expand(target_offset, &constant(requested), max_bytes, domain)?;
            if let Some(lengths) = returndata.length.constants() {
                for length in lengths {
                    let copy_size = (*length).min(U256::from(requested)).to::<usize>();
                    let bytes: Vec<_> = (0..copy_size)
                        .map(|index| returndata.byte_at(index, domain))
                        .collect();
                    let mut branch = expanded.clone();
                    branch.write_values(target_offset, &bytes, max_bytes, domain)?;
                    result = Some(join_array_optional(result, branch, domain));
                }
            } else {
                // Each byte may be outside an unknown returndata length, so
                // join the copied and unchanged possibilities independently.
                let mut copied = expanded.clone();
                let bytes: Vec<_> = (0..requested)
                    .map(|index| returndata.byte_at(index, domain))
                    .collect();
                copied.write_values(target_offset, &bytes, max_bytes, domain)?;
                result = Some(join_array_optional(
                    result,
                    expanded.join(&copied, domain),
                    domain,
                ));
            }
        }
        *self = result.expect("AbstractValue constants are nonempty");
        Ok(())
    }

    /// Join both sequences, including defaults at keys missing on one path.
    pub fn join(&self, other: &Self, domain: Domain) -> Self {
        let keys: BTreeSet<_> = self
            .bytes
            .keys()
            .chain(other.bytes.keys())
            .copied()
            .collect();
        Self {
            length: domain.join(&self.length, &other.length),
            bytes: keys
                .into_iter()
                .map(|index| {
                    (
                        index,
                        domain.join(&self.byte_at(index, domain), &other.byte_at(index, domain)),
                    )
                })
                .collect(),
            default: domain.join(&self.default, &other.default),
            memory: self.memory && other.memory,
        }
    }

    /// Recover bytes only when length and every byte are exact.
    pub fn exact_bytes(&self) -> Option<Vec<u8>> {
        // Keep this convenience extraction bounded even when a library caller
        // deliberately supplies an enormous memory cap to sparse writes.
        self.exact_bytes_bounded(1024 * 1024)
    }

    /// Recover exact bytes while respecting a caller-supplied allocation cap.
    pub fn exact_bytes_bounded(&self, max_bytes: usize) -> Option<Vec<u8>> {
        let lengths = self.length.constants()?;
        if lengths.len() != 1 {
            return None;
        }
        let length = host_index(*lengths.first()?)?;
        if length > max_bytes {
            return None;
        }
        let mut output = Vec::new();
        output.try_reserve_exact(length).ok()?;
        for index in 0..length {
            let byte = self
                .bytes
                .get(&index)
                .unwrap_or(&self.default)
                .constants()?;
            if byte.len() != 1 {
                return None;
            }
            let byte = *byte.first()?;
            if byte > U256::from(255) {
                return None;
            }
            output.push(byte.to::<u8>());
        }
        Some(output)
    }

    fn read_offset_byte(
        &self,
        offset: &AbstractValue,
        increment: usize,
        domain: Domain,
    ) -> Result<AbstractValue, RangeError> {
        let Some(offsets) = offset.constants() else {
            return Ok(if self.bytes.is_empty() && self.default == zero() {
                zero()
            } else {
                AbstractValue::top()
            });
        };
        let mut result = None;
        for offset in offsets {
            if self
                .length
                .constants()
                .is_some_and(|lengths| lengths.iter().all(|length| *length <= *offset))
            {
                result = Some(join_optional(result, zero(), domain));
                continue;
            }
            let Some(base) = host_index(*offset) else {
                let byte = if self
                    .length
                    .constants()
                    .is_some_and(|lengths| lengths.iter().all(|length| *length <= *offset))
                {
                    zero()
                } else {
                    AbstractValue::top()
                };
                result = Some(join_optional(result, byte, domain));
                continue;
            };
            let index = base.checked_add(increment).ok_or(RangeError::Overflow {
                offset: base,
                size: increment,
            })?;
            result = Some(join_optional(result, self.byte_at(index, domain), domain));
        }
        Ok(result.expect("AbstractValue constants are nonempty"))
    }

    fn write_values(
        &mut self,
        offset: &AbstractValue,
        bytes: &[AbstractValue],
        max_bytes: usize,
        domain: Domain,
    ) -> Result<(), RangeError> {
        if bytes.is_empty() {
            return Ok(());
        }
        // Validate every branch before changing this state.
        let mut expanded = self.clone();
        expanded.expand(offset, &constant(bytes.len()), max_bytes, domain)?;
        let Some(offsets) = offset.constants() else {
            expanded.bytes.clear();
            expanded.default = AbstractValue::top();
            *self = expanded;
            return Ok(());
        };
        let mut result = None;
        for offset in offsets {
            let base = host_index(*offset).ok_or(RangeError::OffsetTooLarge(*offset))?;
            let mut branch = expanded.clone();
            for (index, byte) in bytes.iter().enumerate() {
                // expand verified this sum for every offset before mutation.
                branch.bytes.insert(base + index, byte.clone());
            }
            result = Some(join_array_optional(result, branch, domain));
        }
        *self = result.expect("AbstractValue constants are nonempty");
        Ok(())
    }
}

fn zero() -> AbstractValue {
    AbstractValue::constant(U256::ZERO)
}

fn constant(value: usize) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn host_index(value: U256) -> Option<usize> {
    (value <= U256::from(usize::MAX)).then(|| value.to::<usize>())
}

fn bounded_sizes(size: &AbstractValue, max_bytes: usize) -> Result<Vec<usize>, RangeError> {
    size.constants()
        .ok_or(RangeError::UnknownSize)?
        .iter()
        .map(|size| {
            if *size > U256::from(max_bytes) {
                Err(RangeError::TooLarge {
                    requested: *size,
                    limit: max_bytes,
                })
            } else {
                Ok(size.to::<usize>())
            }
        })
        .collect()
}

fn bounded_end(
    offset: usize,
    size: usize,
    max_bytes: usize,
    memory: bool,
) -> Result<usize, RangeError> {
    let mut end = offset
        .checked_add(size)
        .ok_or(RangeError::Overflow { offset, size })?;
    if memory {
        end = end
            .checked_add(31)
            .ok_or(RangeError::Overflow { offset, size })?
            / 32
            * 32;
    }
    if end > max_bytes {
        return Err(RangeError::TooLarge {
            requested: U256::from(end),
            limit: max_bytes,
        });
    }
    Ok(end)
}

fn join_optional(
    current: Option<AbstractValue>,
    value: AbstractValue,
    domain: Domain,
) -> AbstractValue {
    current.map_or(value.clone(), |current| domain.join(&current, &value))
}

fn join_array_optional(current: Option<ByteArray>, value: ByteArray, domain: Domain) -> ByteArray {
    current.map_or_else(|| value.clone(), |current| current.join(&value, domain))
}

#[cfg(test)]
mod tests;
